//! Help › Check for Updates…, and the same check once at launch.
//!
//! The release list is this fork's GitHub releases ([`FEED`]). Every LayerShop release so far is
//! a pre-release, which GitHub's `/releases/latest` leaves out, so the whole list is read and the
//! highest version wins. Tags look like `layershop-v0.10.0-preview.3` (or plain `v0.11.0`); only
//! the version number counts, and a release is news when it is higher than this build's
//! (`CARGO_PKG_VERSION`). Previews of the version already installed don't count: a build doesn't
//! know its own preview number.
//!
//! The fetch is a platform service ([`crate::Services::release_feed`]) run on a worker thread;
//! [`tick`] picks the answer up. Without the service (web, tests) Help › Check for Updates…
//! opens the releases page instead. The launch check is quiet unless there is news: it skips a
//! version the user chose to skip (`skippedUpdate`), never runs with a control channel attached
//! (an agent driving the app gets no surprise dialog) or with `PHOTOCRAFT_NO_UPDATE_CHECK` set,
//! and is off when Preferences › General › Check for Updates is.

use std::sync::mpsc::{Receiver, TryRecvError};

use serde_json::{Map, Value, json};

use crate::PhotocraftApp;
use crate::state::DialogKind;

/// Every release of this fork, newest first.
pub const FEED: &str = "https://api.github.com/repos/SlavomirDurej/layershop/releases?per_page=30";
/// Where people download releases (Help › Check for Updates… without a feed service).
pub const RELEASES: &str = "https://github.com/SlavomirDurej/layershop/releases";
/// This build's version.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

const MARK: &str = "__updateCheck";
const LABEL: &str = "Check for Updates";

/// A release newer than this build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: (u64, u64, u64),
    pub tag: String,
    /// Its title on GitHub ("LayerShop 0.11.0 Preview 1"), or the tag when it has none.
    pub name: String,
    /// Its page on github.com.
    pub url: String,
}

/// The check in flight, and whether the launch check has had its turn.
#[derive(Default)]
pub struct Checker {
    launched: bool,
    pending: Option<Pending>,
}

struct Pending {
    rx: Receiver<Result<String, String>>,
    /// Asked for with Help › Check for Updates…: "up to date" and failures are reported too.
    manual: bool,
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

pub fn version_text(v: (u64, u64, u64)) -> String {
    format!("{}.{}.{}", v.0, v.1, v.2)
}

/// The newest release in GitHub's release list `body` that is newer than `current`, the first
/// listed (the newest) among equal versions. Drafts don't count, and a release page that isn't on
/// github.com is replaced by [`RELEASES`].
pub fn newer_release(body: &str, current: &str) -> Result<Option<Release>, String> {
    let current = parse_version(current).ok_or_else(|| format!("this build's version `{current}` isn't a version number"))?;
    let list: Value = serde_json::from_str(body).map_err(|e| format!("GitHub's answer wasn't a list of releases ({e})"))?;
    let releases = list.as_array().ok_or("GitHub's answer wasn't a list of releases")?;
    // Reversed, so that among equal versions the last one `max_by_key` keeps is the newest.
    let newest = releases
        .iter()
        .rev()
        .filter(|r| r.get("draft").and_then(Value::as_bool) != Some(true))
        .filter_map(|r| {
            let tag = r.get("tag_name")?.as_str()?;
            let version = parse_version(tag)?;
            let url = r.get("html_url").and_then(Value::as_str).filter(|u| u.starts_with("https://github.com/")).unwrap_or(RELEASES);
            let name = r.get("name").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty()).unwrap_or(tag);
            Some(Release { version, tag: tag.chars().take(120).collect(), name: name.chars().take(120).collect(), url: url.to_string() })
        })
        .filter(|r| r.version > current)
        .max_by_key(|r| r.version);
    Ok(newest)
}

/// Help › Check for Updates…: start a check (its answer comes as a dialog), or open the releases
/// page where this build can't fetch them.
pub fn check_now(app: &mut PhotocraftApp, ctx: &egui::Context) -> Value {
    if start(app, ctx, true) {
        app.ui.status = tl!("Checking for updates…").to_string();
        return json!({"checking": true});
    }
    crate::links::open(app, ctx, RELEASES)
}

/// Per frame: the launch check's turn, then any answer that has arrived.
pub fn tick(app: &mut PhotocraftApp, ctx: &egui::Context) {
    if !app.updates.launched {
        app.updates.launched = true;
        if app.session.prefs().general.check_for_updates && app.control_rx.is_none() && std::env::var_os("PHOTOCRAFT_NO_UPDATE_CHECK").is_none() {
            start(app, ctx, false);
        }
    }
    let Some(pending) = &app.updates.pending else {
        return;
    };
    let answer = match pending.rx.try_recv() {
        Ok(answer) => answer,
        Err(TryRecvError::Empty) => return,
        Err(TryRecvError::Disconnected) => Err("the check stopped before GitHub answered".into()),
    };
    let manual = pending.manual;
    app.updates.pending = None;
    report(app, answer.and_then(|body| newer_release(&body, CURRENT)), manual);
}

/// Fetch the feed on a worker thread. `false` when there is no feed service to fetch it with.
fn start(app: &mut PhotocraftApp, ctx: &egui::Context, manual: bool) -> bool {
    if let Some(p) = app.updates.pending.as_mut() {
        // Already asking: the answer is reported as if this check had asked.
        p.manual |= manual;
        return true;
    }
    let Some(feed) = app.services.release_feed.clone() else {
        return false;
    };
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (feed, ctx);
        false
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new().name("update-check".into()).spawn(move || {
            let _ = tx.send(feed());
            ctx.request_repaint();
        });
        if spawned.is_err() {
            return false;
        }
        app.updates.pending = Some(Pending { rx, manual });
        true
    }
}

fn report(app: &mut PhotocraftApp, answer: Result<Option<Release>, String>, manual: bool) {
    let name = crate::brand::NAME;
    let (message, fields) = match answer {
        Ok(Some(release)) => {
            let version = version_text(release.version);
            if !manual && app.session.prefs().skipped_update == version {
                return;
            }
            let template = tl!("{name} {version} is available. You have {current}.");
            let message = crate::i18n::fmt(template, &[("name", name), ("version", version.as_str()), ("current", CURRENT)]);
            app.ui.status = message.clone();
            (message, json!({"url": release.url, "version": version, "release": release.name, "tag": release.tag}))
        }
        Ok(None) if manual => {
            let template = tl!("You have the latest version, {name} {current}.");
            (crate::i18n::fmt(template, &[("name", name), ("current", CURRENT)]), json!({}))
        }
        Err(e) if manual => {
            let template = tl!("Couldn't check for updates: {error}");
            (crate::i18n::fmt(template, &[("error", e.as_str())]), json!({"error": e}))
        }
        Ok(None) => return,
        Err(e) => {
            log::info!("update check at launch: {e}");
            return;
        }
    };
    // One answer at a time.
    let open: Vec<u64> = app.ui.dialogs.iter().filter(|d| owns(&d.fields)).map(|d| d.id).collect();
    for id in open {
        app.ui.close_dialog(id);
    }
    let mut map = fields.as_object().cloned().unwrap_or_default();
    map.insert(MARK.into(), json!(true));
    map.insert("__label".into(), json!(LABEL));
    map.insert("message".into(), json!(message));
    app.ui.open_dialog(DialogKind::Command, map);
}

pub fn owns(fields: &Map<String, Value>) -> bool {
    fields.contains_key(MARK)
}

/// Whether the dialog announces a release (Download / Not Now / Skip This Version) rather than
/// just saying how the check went (OK).
pub fn offers_download(fields: &Map<String, Value>) -> bool {
    fields.get("url").and_then(Value::as_str).is_some()
}

pub fn body(ui: &mut egui::Ui, fields: &Map<String, Value>) {
    ui.label(fields.get("message").and_then(Value::as_str).unwrap_or_default());
    if let Some(release) = fields.get("release").and_then(Value::as_str) {
        ui.add_space(6.0);
        ui.weak(release);
    }
}

/// Download opens the release's page; Skip This Version (`choice: "skip"`) keeps the launch check
/// quiet about this version; OK just closes.
pub fn confirm(app: &mut PhotocraftApp, fields: &Map<String, Value>) -> Result<Value, String> {
    let Some(url) = fields.get("url").and_then(Value::as_str) else {
        return Ok(Value::Null);
    };
    if fields.get("choice").and_then(Value::as_str) == Some("skip") {
        let version = fields.get("version").and_then(Value::as_str).unwrap_or_default().to_string();
        app.session.edit_prefs(|p| p.skipped_update = version.clone());
        return Ok(json!({"skipped": version}));
    }
    let open = app.services.open_url.as_ref().ok_or_else(|| format!("no browser to open {url} in"))?;
    open(url)?;
    let template = "Opened {url}";
    app.ui.status = crate::i18n::fmt(tl!(template), &[("url", url)]);
    Ok(json!({"url": url}))
}

#[cfg(test)]
#[path = "update_check_tests.rs"]
mod tests;
