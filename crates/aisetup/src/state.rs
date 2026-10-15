//! The record of a setup: what was chosen and how far it got.
//!
//! It is what lets the app say "Setup was interrupted. Continue?" after being closed in the
//! middle, and what Add models, Repair and Remove start from. It holds no progress in bytes:
//! what is on disk says that ([`crate::plan`]). It is a file of its own, not a preference:
//! preferences are what the user set, this is what the app did.

use serde::{Deserialize, Serialize};

use crate::plan::Choices;

/// How far a setup got.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    /// Never started.
    #[default]
    None,
    /// Running, or the app was closed while it ran.
    Installing,
    /// Stopped by the user; what arrived is kept.
    Paused,
    /// Stopped by an error; what arrived is kept.
    Failed,
    Done,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SetupState {
    pub phase: Phase,
    /// What the user chose, so the setup can be continued without asking again.
    pub choices: Choices,
    /// The port the engine was given.
    pub port: u16,
    /// The ComfyUI version of the engine.
    pub comfy_version: String,
    /// The manifest models that are installed whole.
    pub installed: Vec<String>,
    /// Why it stopped, when it failed.
    pub error: String,
    /// The wizard was offered at first launch (it is offered once).
    pub offered: bool,
}

impl SetupState {
    /// Is there a setup to continue?
    pub fn interrupted(&self) -> bool {
        matches!(self.phase, Phase::Installing | Phase::Paused | Phase::Failed)
    }

    /// Read a saved record. A missing, damaged or absurdly large file is no record.
    pub fn parse(text: &str) -> SetupState {
        if text.len() > 1 << 20 {
            return SetupState::default();
        }
        let mut state: SetupState = serde_json::from_str(text).unwrap_or_default();
        // The server's file list is of the moment it was read, not something to resume from.
        state.choices.server_files = None;
        state.error = state.error.chars().take(2000).collect();
        state.installed.truncate(1000);
        state
    }

    pub fn to_json(&self) -> String {
        let mut state = self.clone();
        state.choices.server_files = None;
        serde_json::to_string_pretty(&state).unwrap_or_else(|_| "{}".into())
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::SetupState;
    use std::io::Write;
    use std::path::Path;

    /// The record at `path`.
    pub fn load(path: &Path) -> SetupState {
        // A record, not a database: anything huge is not it.
        match std::fs::metadata(path) {
            Ok(meta) if meta.len() <= 1 << 20 => std::fs::read_to_string(path).map(|text| SetupState::parse(&text)).unwrap_or_default(),
            _ => SetupState::default(),
        }
    }

    /// Save the record so that a crash while saving leaves the old one: written beside the
    /// target, flushed, then renamed over it.
    pub fn save(path: &Path, state: &SetupState) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut temp = path.as_os_str().to_os_string();
        temp.push(".new");
        let temp = std::path::PathBuf::from(temp);
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(state.to_json().as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, path)
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::{load, save};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::Route;

    fn state() -> SetupState {
        SetupState {
            phase: Phase::Installing,
            choices: Choices {
                route: Route::Managed,
                models: vec!["qwen-image-2-1".into()],
                engine_dir: "D:\\LS\\engine".into(),
                models_dir: "D:\\LS\\models".into(),
                variant: "nvidia".into(),
                server_files: Some(Default::default()),
                ..Choices::default()
            },
            port: 8190,
            comfy_version: "0.39.0".into(),
            installed: vec![],
            error: String::new(),
            offered: true,
        }
    }

    #[test]
    fn a_record_round_trips_without_what_is_only_of_the_moment() {
        let s = state();
        let back = SetupState::parse(&s.to_json());
        assert_eq!(back.choices.server_files, None);
        assert_eq!(SetupState { choices: Choices { server_files: None, ..s.choices.clone() }, ..s.clone() }, back);
        assert!(back.interrupted());
        assert!(!SetupState { phase: Phase::Done, ..s.clone() }.interrupted() && !SetupState::default().interrupted());
        assert!(SetupState { phase: Phase::Paused, ..s.clone() }.interrupted() && SetupState { phase: Phase::Failed, ..s }.interrupted());
    }

    #[test]
    fn a_truncated_or_corrupt_record_is_no_record() {
        let json = state().to_json();
        for cut in [0, 1, json.len() / 2, json.len() - 1] {
            assert_eq!(SetupState::parse(&json[..cut]), SetupState::default(), "cut at {cut}");
        }
        for junk in ["", "null", "[]", "42", "{\"phase\":\"flying\"}", "{\"port\":-1}", "{\"choices\":7}", "\0\0\0"] {
            assert_eq!(SetupState::parse(junk), SetupState::default(), "{junk}");
        }
        // Unknown keys of a later version are ignored; missing ones take their defaults.
        let later = SetupState::parse(r#"{"phase":"done","future":1,"choices":{"models":["a"],"later":true}}"#);
        assert_eq!((later.phase, later.choices.models, later.port), (Phase::Done, vec!["a".to_string()], 0));
        assert_eq!(SetupState::parse(&" ".repeat((1 << 20) + 1)), SetupState::default());
        let long = SetupState { error: "x".repeat(10_000), installed: vec!["m".into(); 5000], ..SetupState::default() };
        let back = SetupState::parse(&long.to_json());
        assert_eq!((back.error.len(), back.installed.len()), (2000, 1000));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn saving_replaces_the_record_whole() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings").join("ai-setup.json");
        assert_eq!(load(&path), SetupState::default());
        save(&path, &state()).unwrap();
        assert_eq!(load(&path).phase, Phase::Installing);
        save(&path, &SetupState { phase: Phase::Done, ..state() }).unwrap();
        assert_eq!(load(&path).phase, Phase::Done);
        assert!(!dir.path().join("settings").join("ai-setup.json.new").exists());
        // A damaged file reads as no record; a folder in its place too.
        std::fs::write(&path, b"{\"phase\":").unwrap();
        assert_eq!(load(&path), SetupState::default());
        assert_eq!(load(dir.path()), SetupState::default());
    }
}
