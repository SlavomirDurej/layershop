//! Release checks against this fork's GitHub releases. Only public release metadata is requested.
//!
//! Upstream checks `storytold/photocraft` through `craft-update`, which reads stable releases
//! only. Every LayerShop release is a pre-release on `SlavomirDurej/layershop`, tagged
//! `layershop-v0.10.0-preview.3`, so the whole release list is read here and the highest version
//! wins. Only the version number counts: a release is news when it is higher than this build's
//! (`build_info::VERSION`). Previews of the version already installed don't count, since a build
//! doesn't know its own preview number.

use photocraft_engine::updates_cmds::{CheckFn, CheckResult, Release};
use serde_json::Value;

/// Every release of this fork, newest first.
pub const FEED: &str = "https://api.github.com/repos/SlavomirDurej/layershop/releases?per_page=30";
/// Where people download releases; a release page off github.com is replaced by this.
pub const RELEASES: &str = "https://github.com/SlavomirDurej/layershop/releases";

pub fn service() -> CheckFn {
    Box::new(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("update check".into())
            .spawn(move || {
                let result = check();
                if let Err(e) = &result {
                    log::warn!("Update check: {e}");
                }
                let _ = tx.send(result);
            })
            .map_err(|e| format!("Could not start update check: {e}"))?;
        Ok(rx)
    })
}

fn check() -> CheckResult {
    let http = craft_update::Http::new("LayerShop/Update")?;
    match http.get_api(FEED, None)? {
        craft_update::http::Fetched::Body { text, .. } => newer_release(&text, photocraft_engine::build_info::VERSION),
        craft_update::http::Fetched::RateLimited { .. } => Err("GitHub's request limit was reached".into()),
        craft_update::http::Fetched::NotModified | craft_update::http::Fetched::NotFound => Ok(None),
    }
}

/// The version number in a release tag: `layershop-v0.10.0-preview.3`, `v0.11.0` and `0.11` give
/// (0, 10, 0), (0, 11, 0) and (0, 11, 0). Anything else is `None`.
pub fn parse_version(tag: &str) -> Option<(u64, u64, u64)> {
    let t = tag.trim();
    let t = t.strip_prefix("layershop-").unwrap_or(t);
    let t = t.strip_prefix(['v', 'V']).unwrap_or(t);
    let core = t.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = match parts.next() {
        Some(p) => p.parse().ok()?,
        None => 0,
    };
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// The newest release in GitHub's release list `body` that is newer than `current`, the first
/// listed (the newest) among equal versions. Drafts don't count, and a release page that isn't on
/// github.com is replaced by [`RELEASES`].
pub fn newer_release(body: &str, current: &str) -> CheckResult {
    let current = parse_version(current).ok_or_else(|| format!("this build's version `{current}` isn't a version number"))?;
    let list: Value = serde_json::from_str(body).map_err(|e| format!("GitHub's answer wasn't a list of releases ({e})"))?;
    let releases = list.as_array().ok_or("GitHub's answer wasn't a list of releases")?;
    // Reversed, so that among equal versions the last one `max_by_key` keeps is the newest.
    let newest = releases
        .iter()
        .rev()
        .filter(|r| r.get("draft").and_then(Value::as_bool) != Some(true))
        .filter_map(|r| {
            let version = parse_version(r.get("tag_name")?.as_str()?)?;
            let url = r.get("html_url").and_then(Value::as_str).filter(|u| u.starts_with("https://github.com/")).unwrap_or(RELEASES);
            Some((version, url))
        })
        .filter(|(version, _)| *version > current)
        .max_by_key(|(version, _)| *version);
    Ok(newest.map(|((major, minor, patch), url)| Release { version: format!("{major}.{minor}.{patch}"), url: url.to_string() }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn current() -> (u64, u64, u64) {
        parse_version(photocraft_engine::build_info::VERSION).unwrap()
    }

    fn next_minor() -> String {
        let (major, minor, _) = current();
        format!("{major}.{}.0", minor + 1)
    }

    /// GitHub's release list, newest first: a draft of the next major, two previews of the next
    /// minor, this build's own version and an older one.
    fn feed_body() -> String {
        let (major, minor, patch) = current();
        let next = next_minor();
        json!([
            {"tag_name": format!("layershop-v{}.0.0-preview.1", major + 1), "draft": true, "html_url": "https://github.com/x/y/releases/draft"},
            {"tag_name": format!("layershop-v{next}-preview.2"), "html_url": "https://github.com/SlavomirDurej/layershop/releases/tag/p2"},
            {"tag_name": format!("layershop-v{next}-preview.1"), "html_url": "https://github.com/SlavomirDurej/layershop/releases/tag/p1"},
            {"tag_name": format!("layershop-v{major}.{minor}.{patch}-preview.9"), "html_url": "https://github.com/SlavomirDurej/layershop/releases/tag/same"},
            {"tag_name": "layershop-v0.1.0-preview.1"},
        ])
        .to_string()
    }

    #[test]
    fn tags_give_their_version_number() {
        assert_eq!(parse_version("layershop-v0.10.0-preview.3"), Some((0, 10, 0)));
        assert_eq!(parse_version("v0.11.2"), Some((0, 11, 2)));
        assert_eq!(parse_version("1.2"), Some((1, 2, 0)));
        assert_eq!(parse_version("V3.0.1+build.7"), Some((3, 0, 1)));
        for bad in ["", "v", "latest", "layershop-v1", "v1.2.3.4", "v1..2", "v-1.0.0", "v99999999999999999999999.0.0", "v1.x.0"] {
            assert_eq!(parse_version(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_newest_higher_release_wins_and_drafts_and_older_ones_dont_count() {
        let found = newer_release(&feed_body(), photocraft_engine::build_info::VERSION).unwrap().unwrap();
        assert_eq!(found.version, next_minor());
        // Of the two previews of that version, the newest (listed first).
        assert!(found.url.ends_with("/p2"), "{}", found.url);
        // Nothing newer than the newest.
        assert_eq!(newer_release(&feed_body(), &next_minor()).unwrap(), None);
        assert_eq!(newer_release("[]", photocraft_engine::build_info::VERSION).unwrap(), None);
    }

    #[test]
    fn a_release_page_off_github_is_not_opened() {
        let body = json!([{"tag_name": "v999.0.0", "html_url": "https://example.com/evil"}]).to_string();
        let found = newer_release(&body, "0.1.0").unwrap().unwrap();
        assert_eq!((found.version.as_str(), found.url.as_str()), ("999.0.0", RELEASES));
    }

    #[test]
    fn garbage_from_the_feed_is_an_error_not_a_crash() {
        for body in ["", "not json", "{}", "null", "{\"message\": \"API rate limit exceeded\"}"] {
            assert!(newer_release(body, "0.1.0").is_err(), "{body}");
        }
        // Entries of the wrong shape are passed over.
        let odd = json!([1, null, "x", {"tag_name": 5}, {"tag_name": "v999.0.0", "name": 3, "html_url": []}]).to_string();
        assert_eq!(newer_release(&odd, "0.1.0").unwrap().map(|r| r.version), Some("999.0.0".to_string()));
        assert!(newer_release("[]", "not a version").is_err());
    }
}
