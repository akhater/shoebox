//! Private photos: photos the user keeps from being seen.
//!
//! Every password is its own private space, as a wallet's passphrase is its
//! own wallet: no password is ever wrong, a new one opens an empty space.
//! Nobody watching can tell a decoy password from the real one, nor how many
//! there are; nothing is stored that tells, only what each photo needs.
//!
//! While locked, private photos are gone from everything Shoebox shows, to
//! the window and to agents alike: views, counts, dates, folders, People,
//! search, filters, Cull, the CLI. A password shows its own photos (in their
//! usual places and all together in the Private view) and no other's. Only
//! the window opens a space for viewing; agents never see private photos.
//!
//! Shoebox's own copies of a private photo (preview, thumbnail, face
//! pictures) are sealed on disk: anyone browsing the catalog folder sees no
//! picture. The photo files themselves are never touched.
//!
//! Keys: a password gives a seed (Argon2id, with a salt kept in the
//! catalog), the seed a key pair for each photo (by its id). A private photo
//! keeps its public key (`photos.seal_to`): its copies are sealed to it
//! without the password (a scan making its preview again, a stop to finish),
//! and one photo's key says nothing about another's. Opening a space finds
//! its photos by working out their keys; only those it finds can be opened.

use crate::catalog::Catalog;
use crate::db::PREVIEW_OK;
use crate::paths::DataDir;
use anyhow::{Result, anyhow, bail};
use argon2::{Algorithm, Argon2, Params, Version};
use crypto_box::aead::OsRng;
use crypto_box::{PublicKey, SecretKey};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use zeroize::Zeroizing;

/// A password shorter than this is refused (too easy to guess, whatever it opens).
pub const MIN_PASSWORD_CHARS: usize = 6;

/// How hard a password's seed is to work out (Argon2id): 64 MB of memory,
/// 3 passes - about a third of a second, each guess as much.
const KDF_MEMORY_KIB: u32 = 64 * 1024;
const KDF_PASSES: u32 = 3;

/// The first bytes of every sealed file, then the public key it is sealed to.
pub const SEALED_MAGIC: &[u8; 4] = b"SBX1";
const HEAD: usize = 4 + 32;

/// The catalog's salt for passwords (`meta` table): one for every space.
const SALT_KEY: &str = "private_salt";

/// A password's seed: everything its space needs.
type Seed = Zeroizing<[u8; 32]>;

pub fn check_password(password: &str) -> Result<()> {
    if password.chars().count() < MIN_PASSWORD_CHARS {
        bail!("A password needs at least {MIN_PASSWORD_CHARS} characters.");
    }
    Ok(())
}

fn seed_of(password: &str, salt: &[u8], memory_kib: u32, passes: u32) -> Result<Seed> {
    check_password(password)?;
    let params = Params::new(memory_kib, passes, 1, Some(32)).map_err(|e| anyhow!("{e}"))?;
    let mut seed = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, &mut seed[..])
        .map_err(|e| anyhow!("{e}"))?;
    Ok(seed)
}

/// Photo `id`'s key pair in the space of `seed`.
fn photo_key(seed: &Seed, id: i64) -> SecretKey {
    let mut input = b"shoebox private photo ".to_vec();
    input.extend_from_slice(&id.to_le_bytes());
    SecretKey::from(*blake3::keyed_hash(seed, &input).as_bytes())
}

/// `bytes` sealed to a photo's public key: only its secret key opens them.
pub fn seal(public: &PublicKey, bytes: &[u8]) -> Result<Vec<u8>> {
    let sealed = public.seal(&mut OsRng, bytes).map_err(|_| anyhow!("Couldn't seal a private photo's copy"))?;
    let mut out = Vec::with_capacity(HEAD + sealed.len());
    out.extend_from_slice(SEALED_MAGIC);
    out.extend_from_slice(public.as_bytes());
    out.extend_from_slice(&sealed);
    Ok(out)
}

/// The public key a copy is sealed to.
fn sealed_to(sealed: &[u8]) -> Option<[u8; 32]> {
    (sealed.len() > HEAD && sealed.starts_with(SEALED_MAGIC)).then(|| sealed[4..HEAD].try_into().unwrap())
}

/// What [`seal`] sealed.
pub fn open(secret: &SecretKey, sealed: &[u8]) -> Result<Vec<u8>> {
    if sealed_to(sealed) != Some(*secret.public_key().as_bytes()) {
        bail!("A private photo's copy can't be opened with this key");
    }
    secret.unseal(&sealed[HEAD..]).map_err(|_| anyhow!("A private photo's copy is damaged"))
}

/// The space open now, in this process: its seed and its photos (by their
/// public key). Each process starts locked; only the window opens one.
#[derive(Default)]
pub struct Vault {
    open: RwLock<Option<Open>>,
}

struct Open {
    seed: Seed,
    photos: HashMap<[u8; 32], i64>,
}

impl Vault {
    pub fn is_unlocked(&self) -> bool {
        self.open.read().unwrap_or_else(|e| e.into_inner()).is_some()
    }

    /// The photos of the space open now (none while locked).
    pub fn ids(&self) -> HashSet<i64> {
        self.open
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map_or_else(HashSet::new, |o| o.photos.values().copied().collect())
    }

    pub fn lock(&self) {
        *self.open.write().unwrap_or_else(|e| e.into_inner()) = None;
    }

    fn seed(&self) -> Option<Seed> {
        self.open.read().unwrap_or_else(|e| e.into_inner()).as_ref().map(|o| o.seed.clone())
    }

    fn add(&self, id: i64, public: [u8; 32]) {
        if let Some(o) = self.open.write().unwrap_or_else(|e| e.into_inner()).as_mut() {
            o.photos.insert(public, id);
        }
    }

    fn remove(&self, ids: &[i64]) {
        if let Some(o) = self.open.write().unwrap_or_else(|e| e.into_inner()).as_mut() {
            o.photos.retain(|_, id| !ids.contains(id));
        }
    }

    /// Opens a sealed copy of a photo of the space open now; any other is locked.
    pub fn open(&self, sealed: &[u8]) -> Result<Vec<u8>> {
        let guard = self.open.read().unwrap_or_else(|e| e.into_inner());
        let found = guard.as_ref().zip(sealed_to(sealed)).and_then(|(o, to)| Some((o, *o.photos.get(&to)?)));
        match found {
            Some((o, id)) => open(&photo_key(&o.seed, id), sealed),
            None => Err(Locked.into()),
        }
    }
}

/// The photo asked for is private and not in the space open now (or none is).
#[derive(Debug)]
pub struct Locked;

impl std::fmt::Display for Locked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Private photos are locked.")
    }
}

impl std::error::Error for Locked {}

/// What the window may know: whether a space is open. Nothing about others.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub unlocked: bool,
}

pub fn status(cat: &Catalog) -> Status {
    Status { unlocked: cat.vault.is_unlocked() }
}

/// The catalog's salt, made the first time a password is used.
fn salt(cat: &Catalog) -> Result<Vec<u8>> {
    let read = || -> Result<Option<String>> {
        Ok(cat.read()?.query_row("SELECT value FROM meta WHERE key = ?1", [SALT_KEY], |r| r.get(0)).optional()?)
    };
    let hex = match read()? {
        Some(h) => h,
        None => {
            let mut salt = [0u8; 16];
            crypto_box::aead::rand_core::RngCore::try_fill_bytes(&mut OsRng, &mut salt)
                .map_err(|e| anyhow!("No randomness: {e}"))?;
            cat.db().execute(
                "INSERT OR IGNORE INTO meta (key, value) VALUES (?1, ?2)",
                params![SALT_KEY, salt.iter().map(|b| format!("{b:02x}")).collect::<String>()],
            )?;
            read()?.ok_or_else(|| anyhow!("The private photos' salt can't be kept"))?
        }
    };
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2).unwrap_or("x"), 16))
        .collect::<Result<_, _>>()
        .map_err(|_| anyhow!("The private photos' salt is damaged"))
}

fn seed_for(cat: &Catalog, password: &str) -> Result<Seed> {
    seed_of(password, &salt(cat)?, KDF_MEMORY_KIB, KDF_PASSES)
}

/// The photos of the space of `seed`: those whose key it works out.
fn photos_of(cat: &Catalog, seed: &Seed) -> Result<HashMap<[u8; 32], i64>> {
    let rows: Vec<(i64, Vec<u8>)> = cat
        .read()?
        .prepare("SELECT id, seal_to FROM photos WHERE private = 1 AND seal_to IS NOT NULL")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, to)| {
            let public = *photo_key(seed, id).public_key().as_bytes();
            (to == public).then_some((public, id))
        })
        .collect())
}

/// Opens the space of `password` in this process, until [`lock`]: its
/// photos show, in their places and in the Private view. Never refused (but
/// for a password too short): a password nobody used yet opens an empty space.
pub fn unlock(cat: &Catalog, password: &str) -> Result<Status> {
    let seed = seed_for(cat, password)?;
    let photos = photos_of(cat, &seed)?;
    *cat.vault.open.write().unwrap_or_else(|e| e.into_inner()) = Some(Open { seed, photos });
    cat.note_shown_changed();
    log::info!("private photos: unlocked");
    Ok(status(cat))
}

pub fn lock(cat: &Catalog) -> Status {
    if cat.vault.is_unlocked() {
        cat.vault.lock();
        cat.note_shown_changed();
        log::info!("private photos: locked");
    }
    status(cat)
}

/// Makes photos private in the space open now: hidden from everyone else at
/// once, their copies sealed. Its first photos need its password again
/// (`confirm`): a password mistyped opens an empty space, and photos put
/// there would be found only with the typo. Returns how many became private.
pub fn hide(cat: &Catalog, photo_ids: &[i64], confirm: Option<&str>) -> Result<usize> {
    let seed = cat.vault.seed().ok_or(Locked)?;
    if cat.vault.ids().is_empty() {
        match confirm {
            None => bail!(NeedsConfirm),
            Some(again) if *seed_for(cat, again)? != *seed => bail!("The two passwords aren't the same."),
            Some(_) => {}
        }
    }
    hide_with(cat, &seed, photo_ids)
}

/// [`hide`] headless: the space of `password`, opened only to put photos in.
/// A password with no photos yet needs `new` (a typo would start a space).
pub fn hide_with_password(cat: &Catalog, password: &str, photo_ids: &[i64], new: bool) -> Result<usize> {
    let seed = seed_for(cat, password)?;
    if !new && photos_of(cat, &seed)?.is_empty() {
        bail!("No private photos have this password yet. If it is the one you mean, add --new.");
    }
    hide_with(cat, &seed, photo_ids)
}

fn hide_with(cat: &Catalog, seed: &Seed, photo_ids: &[i64]) -> Result<usize> {
    let ids = cat.shown_ids(photo_ids)?;
    let keys: Vec<(i64, PublicKey)> = ids.iter().map(|&id| (id, photo_key(seed, id).public_key())).collect();
    let mut made = Vec::new();
    {
        let mut db = cat.db();
        let tx = db.transaction()?;
        for (id, public) in &keys {
            if tx.execute(
                "UPDATE photos SET private = 1, seal_to = ?1 WHERE id = ?2 AND private = 0",
                params![public.as_bytes().as_slice(), id],
            )? > 0
            {
                made.push((*id, public.clone()));
            }
        }
        tx.commit()?;
    }
    for (id, public) in &made {
        cat.vault.add(*id, *public.as_bytes());
        seal_copies(cat, public, *id)?;
    }
    if !made.is_empty() {
        log::info!("{} photos made private", made.len());
    }
    Ok(made.len())
}

/// Every photo of a scope (a folder) made private: those shown now.
pub fn hide_in(cat: &Catalog, scope: &crate::catalog::Scope, confirm: Option<&str>) -> Result<usize> {
    let ids = cat.list_photos(scope, Default::default(), &Default::default())?.ids;
    hide(cat, &ids, confirm)
}

/// Photos of the space open now made ordinary again: their copies opened
/// back. Returns how many.
pub fn show(cat: &Catalog, photo_ids: &[i64]) -> Result<usize> {
    let seed = cat.vault.seed().ok_or(Locked)?;
    let mine = cat.vault.ids();
    let ids: Vec<i64> = photo_ids.iter().copied().filter(|id| mine.contains(id)).collect();
    // Copies first, then the catalog: stopped in between, the photo is still
    // private and the next start seals its copies again.
    for &id in &ids {
        unseal_copies(cat, &photo_key(&seed, id), id)?;
    }
    {
        let mut db = cat.db();
        let tx = db.transaction()?;
        for id in &ids {
            tx.execute("UPDATE photos SET private = 0, seal_to = NULL WHERE id = ?1", [id])?;
        }
        tx.commit()?;
    }
    cat.vault.remove(&ids);
    if !ids.is_empty() {
        log::info!("{} photos no longer private", ids.len());
    }
    Ok(ids.len())
}

/// At start: copies of private photos a stop left in the clear are sealed
/// (no password needed). Returns how many photos had some.
pub fn settle(cat: &Catalog) -> Result<usize> {
    let mut n = 0;
    for (id, public) in private_keys(cat)? {
        if seal_copies(cat, &public, id)? > 0 {
            n += 1;
        }
    }
    if n > 0 {
        log::info!("private photos: copies of {n} photos sealed after a stop");
    }
    Ok(n)
}

/// Once unlocked: face pictures of its photos lost while locked are cut
/// again from their preview.
pub fn after_unlock(cat: &Catalog) -> Result<usize> {
    let ids = cat.vault.ids();
    if ids.is_empty() {
        return Ok(0);
    }
    let faces: Vec<(i64, i64)> = cat
        .read()?
        .prepare(&format!(
            "SELECT fa.id, fa.photo_id FROM faces fa JOIN photos p ON p.id = fa.photo_id
              WHERE p.private = 1 AND p.preview_state = {PREVIEW_OK}"
        ))?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let lost: Vec<i64> = faces
        .into_iter()
        .filter(|(f, p)| ids.contains(p) && !cat.has_copy(&cat.data.face_path(*f)))
        .map(|(f, _)| f)
        .collect();
    if lost.is_empty() {
        return Ok(0);
    }
    Ok(crate::faces::cut_pictures_again(cat, &lost)?.len())
}

/// Every private photo and the key its copies are sealed to.
fn private_keys(cat: &Catalog) -> Result<Vec<(i64, PublicKey)>> {
    let rows: Vec<(i64, Vec<u8>)> = cat
        .read()?
        .prepare("SELECT id, seal_to FROM photos WHERE private = 1 AND seal_to IS NOT NULL")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows.into_iter().filter_map(|(id, to)| Some((id, PublicKey::from(<[u8; 32]>::try_from(to).ok()?)))).collect())
}

/// Every copy Shoebox makes of a photo: preview, thumbnail, face pictures.
fn copies_of(cat: &Catalog, photo_id: i64) -> Result<Vec<PathBuf>> {
    let mut out = vec![cat.data.preview_path(photo_id), cat.data.thumb_path(photo_id)];
    let db = cat.read()?;
    let mut stmt = db.prepare("SELECT id FROM faces WHERE photo_id = ?1")?;
    for f in stmt.query_map([photo_id], |r| r.get::<_, i64>(0))? {
        out.push(cat.data.face_path(f?));
    }
    Ok(out)
}

/// Seals a photo's copies still in the clear. Returns how many.
fn seal_copies(cat: &Catalog, public: &PublicKey, photo_id: i64) -> Result<usize> {
    let mut n = 0;
    for path in copies_of(cat, photo_id)? {
        match std::fs::read(&path) {
            // The one in the clear is the latest written: sealed over any older one.
            Ok(bytes) => {
                cat.data.write(&DataDir::sealed_path(&path), &seal(public, &bytes)?)?;
                cat.data.remove(&path)?;
                n += 1;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(n)
}

fn unseal_copies(cat: &Catalog, secret: &SecretKey, photo_id: i64) -> Result<()> {
    for path in copies_of(cat, photo_id)? {
        let sealed = DataDir::sealed_path(&path);
        match std::fs::read(&sealed) {
            Ok(bytes) => match open(secret, &bytes) {
                Ok(plain) => {
                    cat.data.write(&path, &plain)?;
                    cat.data.remove(&sealed)?;
                }
                // Damaged: of no use. Missing, it is made again as a lost one would be.
                Err(e) => {
                    log::warn!("photo {photo_id}: {} can't be opened, removed: {e:#}", sealed.display());
                    cat.data.remove(&sealed)?;
                }
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// What is shown, and the copies of photos, private ones included. Shown:
/// every photo that isn't private, and those of the space open now.
impl Catalog {
    /// Whether any photo is hidden right now. Most catalogs have none: then
    /// their queries get nothing added (counted through the private photos'
    /// own index, as small as they are).
    pub fn hides_any(&self) -> bool {
        let private = self
            .read()
            .and_then(|db| {
                Ok(db.query_row("SELECT COUNT(*) FROM photos WHERE private = 1", [], |r| r.get::<_, i64>(0))?)
            })
            .unwrap_or(i64::MAX);
        private > self.vault.ids().len() as i64
    }

    /// The files of the private photos hidden right now: (disk, folder, photo).
    pub fn hidden_files(&self) -> Result<Vec<(i64, String, i64)>> {
        if !self.hides_any() {
            return Ok(Vec::new());
        }
        let db = self.read()?;
        let rows = db
            .prepare(&format!(
                "SELECT fo.volume_id, fo.rel_path, f.photo_id FROM files f JOIN folders fo ON fo.id = f.folder_id
                  WHERE f.photo_id IN {}",
                self.hidden_ids_sql()
            ))?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// SQL: the ids of the private photos hidden right now (all of them while
    /// locked; those of other spaces once one is open).
    pub fn hidden_ids_sql(&self) -> String {
        match ids_sql(&self.vault.ids()) {
            Some(mine) => format!("(SELECT id FROM photos WHERE private = 1 AND id NOT IN ({mine}))"),
            None => "(SELECT id FROM photos WHERE private = 1)".into(),
        }
    }

    /// SQL condition on photo alias `p`: shown right now.
    pub fn shown_sql(&self, p: &str) -> String {
        if !self.hides_any() {
            return "1".into();
        }
        match ids_sql(&self.vault.ids()) {
            Some(mine) => format!("({p}.private = 0 OR {p}.id IN ({mine}))"),
            None => format!("{p}.private = 0"),
        }
    }

    /// SQL condition on photo alias `p`: a photo of the space open now (the Private view).
    pub fn mine_sql(&self, p: &str) -> String {
        ids_sql(&self.vault.ids()).map_or_else(|| "0".into(), |mine| format!("{p}.id IN ({mine})"))
    }

    /// SQL condition on face alias `f`: in a photo shown right now (faces
    /// carry their photo's flag).
    pub fn shown_face_sql(&self, f: &str) -> String {
        if !self.hides_any() {
            return "1".into();
        }
        match ids_sql(&self.vault.ids()) {
            Some(mine) => format!("({f}.private = 0 OR {f}.photo_id IN ({mine}))"),
            None => format!("{f}.private = 0"),
        }
    }

    /// SQL condition on a column holding a photo id (never NULL): shown right now.
    pub fn shown_photo_sql(&self, col: &str) -> String {
        if self.hides_any() { format!("{col} NOT IN {}", self.hidden_ids_sql()) } else { "1".into() }
    }

    /// SQL condition on a column holding a photo id: hidden right now.
    pub fn hidden_photo_sql(&self, col: &str) -> String {
        if self.hides_any() { format!("{col} IN {}", self.hidden_ids_sql()) } else { "0".into() }
    }

    /// Those of `ids` shown right now, in the same order.
    pub fn shown_ids(&self, ids: &[i64]) -> Result<Vec<i64>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let hidden = self.hidden_ids()?;
        Ok(ids.iter().copied().filter(|id| !hidden.contains(id)).collect())
    }

    /// An error, the same as for a photo that doesn't exist, when photo `id`
    /// is hidden right now.
    pub fn check_shown(&self, id: i64) -> Result<()> {
        if self.is_private(id)? && !self.vault.ids().contains(&id) {
            bail!("Unknown photo {id}");
        }
        Ok(())
    }

    /// The private photos hidden right now: their copies can't be read.
    pub fn hidden_ids(&self) -> Result<HashSet<i64>> {
        let mine = self.vault.ids();
        Ok(private_keys(self)?.into_iter().map(|(id, _)| id).filter(|id| !mine.contains(id)).collect())
    }

    pub fn is_private(&self, photo_id: i64) -> Result<bool> {
        Ok(self.seal_to(photo_id)?.is_some())
    }

    /// The key a private photo's copies are sealed to; None when it isn't private.
    fn seal_to(&self, photo_id: i64) -> Result<Option<PublicKey>> {
        let to: Option<Option<Vec<u8>>> = self
            .read()?
            .query_row("SELECT seal_to FROM photos WHERE id = ?1 AND private = 1", [photo_id], |r| r.get(0))
            .optional()?;
        match to.flatten() {
            Some(bytes) => Ok(Some(PublicKey::from(
                <[u8; 32]>::try_from(bytes).map_err(|_| anyhow!("Photo {photo_id}'s key is damaged"))?,
            ))),
            None => Ok(None),
        }
    }

    /// A copy Shoebox made of a photo (preview, thumbnail, face picture),
    /// from its sealed file when the photo is private: only while its space is open.
    pub fn read_copy(&self, path: &Path) -> Result<Vec<u8>> {
        match std::fs::read(path) {
            Ok(bytes) => Ok(bytes),
            Err(e) if e.kind() == io::ErrorKind::NotFound => match std::fs::read(DataDir::sealed_path(path)) {
                Ok(sealed) => self.vault.open(&sealed),
                Err(e2) if e2.kind() == io::ErrorKind::NotFound => Err(e.into()),
                Err(e2) => Err(e2.into()),
            },
            Err(e) => Err(e.into()),
        }
    }

    /// [`Catalog::read_copy`], decoded.
    pub fn open_copy(&self, path: &Path) -> Result<image::DynamicImage> {
        Ok(image::load_from_memory(&self.read_copy(path)?)?)
    }

    /// A copy's width and height (from its header when it is in the clear).
    pub fn copy_dimensions(&self, path: &Path) -> Result<(u32, u32)> {
        match image::image_dimensions(path) {
            Ok(wh) => Ok(wh),
            Err(_) if !path.is_file() => {
                let bytes = self.read_copy(path)?;
                Ok(image::ImageReader::new(io::Cursor::new(bytes)).with_guessed_format()?.into_dimensions()?)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Whether a copy is there, in the clear or sealed.
    pub fn has_copy(&self, path: &Path) -> bool {
        path.is_file() || DataDir::sealed_path(path).is_file()
    }

    /// Writes a copy of photo `photo_id`: sealed when the photo is private
    /// (no password needed).
    pub fn write_copy(&self, photo_id: i64, path: &Path, bytes: &[u8]) -> Result<()> {
        match self.seal_to(photo_id)? {
            Some(public) => {
                self.data.write(&DataDir::sealed_path(path), &seal(&public, bytes)?)?;
                self.data.remove(path)?;
            }
            None => self.data.write(path, bytes)?,
        }
        Ok(())
    }

    /// Removes a copy, in the clear or sealed.
    pub fn remove_copy(&self, path: &Path) -> Result<()> {
        self.data.remove(path)?;
        self.data.remove(&DataDir::sealed_path(path))?;
        Ok(())
    }
}

/// Ids as an SQL list ("3,7,12"), None when there are none.
fn ids_sql(ids: &HashSet<i64>) -> Option<String> {
    if ids.is_empty() {
        return None;
    }
    let mut ids: Vec<i64> = ids.iter().copied().collect();
    ids.sort_unstable();
    Some(ids.iter().map(i64::to_string).collect::<Vec<_>>().join(","))
}

/// Asked to make the first photos private under a password: type it again.
#[derive(Debug)]
pub struct NeedsConfirm;

impl std::fmt::Display for NeedsConfirm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Type the password again: no photos are private with it yet.")
    }
}

impl std::error::Error for NeedsConfirm {}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cheaper seed for tests: the real one takes 64 MB a try.
    fn quick_seed(password: &str) -> Seed {
        seed_of(password, b"a salt of 16 b..", 64, 1).unwrap()
    }

    #[test]
    fn every_password_is_its_own_space() {
        let (a, b) = (quick_seed("correct horse"), quick_seed("battery staple"));
        assert_eq!(*a, *quick_seed("correct horse"), "the same password, the same space");
        assert_ne!(*a, *b);
        assert!(seed_of("short", b"a salt of 16 b..", 64, 1).is_err(), "too short");
        // Each photo its own key: one says nothing about another, nor about its space.
        let (a1, a2, b1) =
            (photo_key(&a, 1).public_key(), photo_key(&a, 2).public_key(), photo_key(&b, 1).public_key());
        assert_ne!(a1.as_bytes(), a2.as_bytes());
        assert_ne!(a1.as_bytes(), b1.as_bytes());
        assert_eq!(a1.as_bytes(), photo_key(&a, 1).public_key().as_bytes());
    }

    #[test]
    fn sealed_copies_open_only_in_their_space() {
        let (a, b) = (quick_seed("correct horse"), quick_seed("battery staple"));
        let photo = b"\xFF\xD8\xFF\xE0 a preview".to_vec();
        let public = photo_key(&a, 7).public_key();
        // Sealed without the password: the photo's public key is enough.
        let sealed = seal(&public, &photo).unwrap();
        assert!(sealed.starts_with(SEALED_MAGIC));
        assert!(!sealed.windows(photo.len()).any(|w| w == photo.as_slice()), "nothing readable left");
        assert_eq!(open(&photo_key(&a, 7), &sealed).unwrap(), photo);
        assert!(open(&photo_key(&b, 7), &sealed).is_err(), "another password");
        assert!(open(&photo_key(&a, 8), &sealed).is_err(), "another photo");

        let vault = Vault::default();
        assert!(vault.open(&sealed).unwrap_err().is::<Locked>());
        *vault.open.write().unwrap() = Some(Open { seed: b, photos: HashMap::new() });
        assert!(vault.open(&sealed).unwrap_err().is::<Locked>(), "not in the space open");
        *vault.open.write().unwrap() = Some(Open { seed: a, photos: HashMap::from([(*public.as_bytes(), 7)]) });
        assert_eq!(vault.open(&sealed).unwrap(), photo);
        assert_eq!(vault.ids(), HashSet::from([7]));
        vault.lock();
        assert!(!vault.is_unlocked() && vault.ids().is_empty());
    }
}
