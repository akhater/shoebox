//! A sidecar is read without being built: its parts are checked as JSON and
//! kept as slices of its text, and of the edit (`adjustments`) a value is
//! built only when the rule asks for it. The EXIF copy, history, snapshots,
//! masks and AI patches (often megabytes) never are.

use std::cell::OnceCell;
use std::collections::HashMap;

use serde_json::Value;
use serde_json::value::RawValue;

use crate::Error;
use crate::fingerprint::Fingerprint;

/// What the edited rule reads of an edit.
#[derive(Default)]
pub(crate) struct Facts<'a> {
    /// The edit is a JSON object; Argentum reads nothing else as an edit.
    pub object: bool,
    /// Each of the edit's values as text, and built once asked for.
    pub values: HashMap<String, (&'a RawValue, OnceCell<Value>)>,
    /// `masks` is an array with something in it.
    pub masks: bool,
    /// `aiPatches` is an array with something in it.
    pub ai_patches: bool,
}

impl Facts<'_> {
    /// The edit's value for `key`, null when it has none.
    pub fn get(&self, key: &str) -> &Value {
        match self.values.get(key) {
            Some((text, value)) => value.get_or_init(|| serde_json::from_str(text.get()).unwrap_or(Value::Null)),
            None => &Value::Null,
        }
    }
}

pub(crate) struct Edit<'a> {
    pub facts: Facts<'a>,
    pub fingerprint: Fingerprint,
}

pub(crate) fn edit(bytes: &[u8]) -> Result<Edit<'_>, Error> {
    // Argentum reads a sidecar as text first: one that is not UTF-8 is no edit.
    let text = std::str::from_utf8(bytes).map_err(|_| Error)?;
    // Of a key given twice, the later counts, as in Argentum.
    let sidecar: HashMap<String, &RawValue> = serde_json::from_str(text).map_err(|_| Error)?;
    // A sidecar without one reads as an empty (null) edit.
    let edit = sidecar.get("adjustments").map_or("null", |raw| raw.get());
    Ok(Edit { facts: facts(edit), fingerprint: Fingerprint::of(edit) })
}

fn facts(edit: &str) -> Facts<'_> {
    let Ok(values) = serde_json::from_str::<HashMap<String, &RawValue>>(edit) else {
        return Facts::default();
    };
    let non_empty_array = |key: &str| values.get(key).is_some_and(|raw| is_non_empty_array(raw.get()));
    Facts {
        object: true,
        masks: non_empty_array("masks"),
        ai_patches: non_empty_array("aiPatches"),
        values: values.into_iter().map(|(key, raw)| (key, (raw, OnceCell::new()))).collect(),
    }
}

/// Whether `json`, one valid JSON value, is an array with something in it.
fn is_non_empty_array(json: &str) -> bool {
    json.strip_prefix('[').is_some_and(|rest| !rest.trim_start().starts_with(']'))
}
