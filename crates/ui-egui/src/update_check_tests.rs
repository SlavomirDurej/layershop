use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;

use super::*;
use crate::Services;

fn current() -> (u64, u64, u64) {
    parse_version(CURRENT).unwrap()
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
        {"tag_name": format!("layershop-v{next}-preview.2"), "name": "LayerShop next Preview 2", "html_url": "https://github.com/SlavomirDurej/layershop/releases/tag/p2"},
        {"tag_name": format!("layershop-v{next}-preview.1"), "name": "LayerShop next Preview 1", "html_url": "https://github.com/SlavomirDurej/layershop/releases/tag/p1"},
        {"tag_name": format!("layershop-v{major}.{minor}.{patch}-preview.9"), "html_url": "https://github.com/SlavomirDurej/layershop/releases/tag/same"},
        {"tag_name": "layershop-v0.1.0-preview.1"},
    ])
    .to_string()
}

fn app_with(feed: Option<Result<String, String>>, opened: Arc<Mutex<Vec<String>>>) -> PhotocraftApp {
    let services = Services {
        release_feed: feed.map(|answer| -> crate::ReleaseFeedFn { Arc::new(move || answer.clone()) }),
        open_url: Some(Box::new(move |u: &str| {
            opened.lock().unwrap().push(u.to_string());
            Ok(())
        })),
        ..Default::default()
    };
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), services);
    // The launch check is tested on its own.
    app.updates.launched = true;
    app
}

/// Ticks until the check in flight has answered.
fn settle(app: &mut PhotocraftApp, ctx: &egui::Context) {
    let until = Instant::now() + Duration::from_secs(10);
    tick(app, ctx);
    while app.updates.pending.is_some() {
        assert!(Instant::now() < until, "the update check never answered");
        std::thread::sleep(Duration::from_millis(5));
        tick(app, ctx);
    }
}

fn update_dialog(app: &PhotocraftApp) -> Option<&crate::state::Dialog> {
    app.ui.dialogs.iter().find(|d| owns(&d.fields))
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
    let found = newer_release(&feed_body(), CURRENT).unwrap().unwrap();
    assert_eq!(version_text(found.version), next_minor());
    // Of the two previews of that version, the newest (listed first).
    assert_eq!(found.name, "LayerShop next Preview 2");
    assert!(found.url.ends_with("/p2"));
    // Nothing newer than the newest.
    assert_eq!(newer_release(&feed_body(), &next_minor()).unwrap(), None);
    assert_eq!(newer_release("[]", CURRENT).unwrap(), None);
}

#[test]
fn a_release_page_off_github_is_not_opened() {
    let body = json!([{"tag_name": "v999.0.0", "html_url": "https://example.com/evil"}]).to_string();
    let found = newer_release(&body, CURRENT).unwrap().unwrap();
    assert_eq!(found.url, RELEASES);
    assert_eq!(found.name, "v999.0.0");
}

#[test]
fn garbage_from_the_feed_is_an_error_not_a_crash() {
    for body in ["", "not json", "{}", "null", "{\"message\": \"API rate limit exceeded\"}"] {
        assert!(newer_release(body, CURRENT).is_err(), "{body}");
    }
    // Entries of the wrong shape are passed over.
    let odd = json!([1, null, "x", {"tag_name": 5}, {"tag_name": "v999.0.0", "name": 3, "html_url": []}]).to_string();
    assert_eq!(newer_release(&odd, CURRENT).unwrap().map(|r| r.version), Some((999, 0, 0)));
    assert!(newer_release("[]", "not a version").is_err());
}

#[test]
fn help_check_for_updates_offers_the_new_release_and_download_opens_it() {
    let opened = Arc::new(Mutex::new(Vec::new()));
    let mut app = app_with(Some(Ok(feed_body())), opened.clone());
    let ctx = egui::Context::default();
    assert!(crate::menus::is_live("help.checkForUpdates") && crate::menus::is_enabled(&app, "help.checkForUpdates"));
    let r = crate::menus::invoke(&mut app, &ctx, "help.checkForUpdates", json!({})).unwrap();
    assert_eq!(r["checking"], true);
    settle(&mut app, &ctx);
    let d = update_dialog(&app).expect("a dialog says there is a new version");
    assert!(offers_download(&d.fields));
    assert_eq!(d.fields["version"], next_minor());
    let id = d.id;
    let r = crate::dialogs::confirm(&mut app, id).unwrap();
    assert!(r["url"].as_str().unwrap().ends_with("/p2"));
    assert_eq!(opened.lock().unwrap().len(), 1);
    assert!(update_dialog(&app).is_none());
}

#[test]
fn skip_this_version_quiets_the_launch_check_but_not_the_menu() {
    let opened = Arc::new(Mutex::new(Vec::new()));
    let mut app = app_with(Some(Ok(feed_body())), opened.clone());
    let ctx = egui::Context::default();
    crate::menus::invoke(&mut app, &ctx, "help.checkForUpdates", json!({})).unwrap();
    settle(&mut app, &ctx);
    let d = update_dialog(&app).unwrap();
    let id = d.id;
    app.ui.dialogs.iter_mut().find(|d| d.id == id).unwrap().fields.insert("choice".into(), json!("skip"));
    crate::dialogs::confirm(&mut app, id).unwrap();
    assert_eq!(app.session.prefs().skipped_update, next_minor());
    assert!(opened.lock().unwrap().is_empty(), "skipping opens nothing");
    // At the next launch: no dialog for the skipped version.
    app.updates.launched = false;
    settle(&mut app, &ctx);
    assert!(update_dialog(&app).is_none());
    // Help › Check for Updates… still tells.
    crate::menus::invoke(&mut app, &ctx, "help.checkForUpdates", json!({})).unwrap();
    settle(&mut app, &ctx);
    assert!(update_dialog(&app).is_some_and(|d| offers_download(&d.fields)));
}

#[test]
fn the_launch_check_runs_once_and_follows_the_preference() {
    let mut app = app_with(Some(Ok(feed_body())), Default::default());
    let ctx = egui::Context::default();
    app.session.edit_prefs(|p| p.general.check_for_updates = false);
    app.updates.launched = false;
    settle(&mut app, &ctx);
    assert!(update_dialog(&app).is_none(), "off in Preferences");

    app.session.edit_prefs(|p| p.general.check_for_updates = true);
    app.updates.launched = false;
    settle(&mut app, &ctx);
    let id = update_dialog(&app).expect("on: the launch check tells").id;
    // Not Now.
    app.ui.close_dialog(id);
    settle(&mut app, &ctx);
    assert!(update_dialog(&app).is_none(), "once per launch");
}

#[test]
fn the_launch_check_is_quiet_when_up_to_date_or_offline_and_the_menu_is_not() {
    let ctx = egui::Context::default();
    for (answer, says) in [(Ok("[]".to_string()), "latest version"), (Err("no network".to_string()), "no network")] {
        let mut app = app_with(Some(answer), Default::default());
        app.updates.launched = false;
        settle(&mut app, &ctx);
        assert!(update_dialog(&app).is_none(), "{says}");
        crate::menus::invoke(&mut app, &ctx, "help.checkForUpdates", json!({})).unwrap();
        settle(&mut app, &ctx);
        let d = update_dialog(&app).unwrap();
        assert!(!offers_download(&d.fields));
        assert!(d.fields["message"].as_str().unwrap().contains(says), "{:?}", d.fields);
        // OK just closes.
        let id = d.id;
        assert_eq!(crate::dialogs::confirm(&mut app, id).unwrap(), Value::Null);
    }
}

#[test]
fn without_a_feed_the_menu_opens_the_releases_page() {
    let opened = Arc::new(Mutex::new(Vec::new()));
    let mut app = app_with(None, opened.clone());
    let ctx = egui::Context::default();
    app.updates.launched = false;
    tick(&mut app, &ctx);
    assert!(app.updates.pending.is_none());
    let r = crate::menus::invoke(&mut app, &ctx, "help.checkForUpdates", json!({})).unwrap();
    assert_eq!(r["url"], RELEASES);
    assert_eq!(opened.lock().unwrap().as_slice(), [RELEASES]);
}
