//! Is there a newer Shoebox? Asks GitHub for the latest **published release**
//! of akhater/shoebox. A commit on `main` or on a branch never counts, nor do
//! drafts and pre-releases: only what a release build was made for.
//!
//! Nothing is downloaded or installed: the window shows the new version and
//! opens its release page. The request carries no catalog data, only
//! Shoebox's version in its user agent.

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

/// The version running.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
/// Where a new version is downloaded from.
pub const RELEASES_PAGE: &str = "https://github.com/akhater/shoebox/releases/latest";
const LATEST_API: &str = "https://api.github.com/repos/akhater/shoebox/releases/latest";
/// How often a running Shoebox asks again, in seconds.
pub const EVERY: i64 = 6 * 3600;
/// How soon after a failed check (offline, GitHub busy) it tries again.
pub const RETRY: i64 = 30 * 60;

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    /// This Shoebox.
    pub current: String,
    /// The latest published release ("26.43.1"); none if none is published yet.
    pub latest: Option<String>,
    /// `latest` is newer than `current`.
    pub newer: bool,
    /// The release's page on GitHub.
    pub url: String,
    /// When it was published (as GitHub gives it, RFC 3339).
    pub published: Option<String>,
    /// When GitHub was last asked, unix seconds.
    pub checked: i64,
    /// Why the last check failed, if it did. What an earlier check found is kept.
    pub error: Option<String>,
}

impl UpdateStatus {
    /// Seconds until the next check is due.
    pub fn due_in(&self, now: i64) -> i64 {
        self.checked + if self.error.is_some() { RETRY } else { EVERY } - now
    }
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    html_url: Option<String>,
    published_at: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

/// Asks GitHub now. Never fails: a failure is in `error`, and keeps what
/// `previous` found (an update seen this morning still shows when offline).
pub fn check(previous: Option<&UpdateStatus>) -> UpdateStatus {
    let mut s = UpdateStatus {
        current: CURRENT.into(),
        url: RELEASES_PAGE.into(),
        checked: chrono::Utc::now().timestamp(),
        ..Default::default()
    };
    match latest() {
        Ok(Some(r)) if !r.draft && !r.prerelease => {
            let version = clean(&r.tag_name);
            s.newer = is_newer(&version, CURRENT);
            s.latest = Some(version);
            s.published = r.published_at;
            // Only ever a page of this repository.
            if let Some(url) = r.html_url.filter(|u| u.starts_with("https://github.com/akhater/shoebox/")) {
                s.url = url;
            }
        }
        Ok(_) => {}
        Err(e) => {
            if let Some(p) = previous {
                (s.latest, s.newer, s.url, s.published) =
                    (p.latest.clone(), p.newer, p.url.clone(), p.published.clone());
            }
            s.error = Some(format!("{e:#}"));
        }
    }
    match (&s.error, &s.latest) {
        (Some(e), _) => log::info!("updates: {e}"),
        (None, Some(v)) if s.newer => log::info!("updates: Shoebox {v} is out (this is {CURRENT})"),
        _ => {}
    }
    s
}

/// The latest published release; none (404) until the first one.
fn latest() -> Result<Option<Release>> {
    let agent = crate::models::http_agent(15, 20);
    let response = match agent.get(LATEST_API).header("Accept", "application/vnd.github+json").call() {
        Ok(r) => r,
        Err(ureq::Error::StatusCode(404)) => return Ok(None),
        Err(e) => return Err(anyhow!("Couldn't ask GitHub for the latest version: {e}")),
    };
    let body = response.into_body().read_to_string()?;
    Ok(Some(serde_json::from_str(&body)?))
}

/// "v26.43.1", "Shoebox v26.43.1" -> "26.43.1".
fn clean(tag: &str) -> String {
    tag.trim().trim_start_matches(|c: char| !c.is_ascii_digit()).to_string()
}

/// "26.43.1" -> [26, 43, 1]. A suffix ("-beta", "+build") is set aside.
fn numbers(version: &str) -> Option<Vec<u64>> {
    let core = clean(version);
    let core = core.split(['-', '+']).next()?;
    core.split('.').map(|n| n.parse().ok()).collect()
}

/// `latest` is a later version than `current` ("26.43.1" > "26.42.9";
/// "26.43" = "26.43.0"). Anything that isn't a version is never newer.
pub fn is_newer(latest: &str, current: &str) -> bool {
    let (Some(mut l), Some(mut c)) = (numbers(latest), numbers(current)) else {
        return false;
    };
    let n = l.len().max(c.len());
    l.resize(n, 0);
    c.resize(n, 0);
    l > c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_as_numbers() {
        assert!(is_newer("26.43.1", "26.42.1"));
        assert!(is_newer("v26.42.2", "26.42.1"));
        assert!(is_newer("26.42.10", "26.42.9"));
        assert!(is_newer("27.1.0", "26.52.3"));
        assert!(!is_newer("26.42.1", "26.42.1"));
        assert!(!is_newer("v26.42.1", "26.42.1"));
        assert!(!is_newer("26.41.5", "26.42.1"));
        assert!(!is_newer("26.42", "26.42.0"));
        assert!(is_newer("26.42.0.1", "26.42"));
    }

    #[test]
    fn odd_tags_never_announce_anything() {
        assert!(!is_newer("", "26.42.1"));
        assert!(!is_newer("latest", "26.42.1"));
        assert!(!is_newer("v26.x", "26.42.1"));
        assert!(is_newer("Shoebox v26.43.1", "26.42.1"));
        assert!(is_newer("26.43.0-beta", "26.42.1"));
    }

    #[test]
    fn tags_lose_their_prefix() {
        assert_eq!(clean("v26.43.1"), "26.43.1");
        assert_eq!(clean("Shoebox v26.43.1"), "26.43.1");
        assert_eq!(clean("26.43.1"), "26.43.1");
    }

    #[test]
    fn a_failed_check_is_retried_sooner() {
        let ok = UpdateStatus { checked: 1000, ..Default::default() };
        let failed = UpdateStatus { checked: 1000, error: Some("offline".into()), ..Default::default() };
        assert_eq!(ok.due_in(1000), EVERY);
        assert_eq!(failed.due_in(1000), RETRY);
    }
}
