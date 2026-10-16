//! Where the desktop app keeps its per-user data: preferences, brush presets, crash-recovery
//! autosaves and the GPU startup marker all live under the one directory [`config_dir`] returns.
//!
//! In order:
//! 1. `PHOTOCRAFT_CONFIG_DIR`, when set (tests, agents, custom setups).
//! 2. **Portable mode** (#228): a marker file ([`PORTABLE_MARKERS`]) beside the executable puts
//!    everything in `<exe dir>/PhotoCraftData`, so a portable copy on a USB stick leaves nothing in
//!    `%APPDATA%`. The Windows portable zip ships with `portable.txt`; the MSI doesn't. If that
//!    folder can't be written (read-only medium, Program Files), the app warns and falls back.
//! 3. The platform convention, in a folder named after the product ([`DIR_NAME`]): macOS
//!    `~/Library/Application Support/LayerShop`, Windows `%APPDATA%\LayerShop`, Linux
//!    `$XDG_CONFIG_HOME/layershop` or `~/.config/layershop`.
//!
//! Builds before the rename kept their settings in a `Photocraft` folder ([`LEGACY_DIR_NAME`]).
//! When the product's folder doesn't exist yet and that one does, it is copied over once
//! ([`adopt_legacy`]). Copied, not moved: the old folder may be PhotoCraft's own.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The settings folder's name in the platform's per-user config directory (lowercase on Linux).
pub const DIR_NAME: &str = photocraft_ui_egui::brand::NAME;
/// Where builds before the rename kept their settings.
pub const LEGACY_DIR_NAME: &str = "Photocraft";

/// Files beside the executable that switch on portable mode (either one; contents are ignored).
pub const PORTABLE_MARKERS: [&str; 2] = ["portable.txt", "PhotoCraft.portable"];
/// The data folder created beside the executable in portable mode.
pub const PORTABLE_DATA_DIR: &str = "PhotoCraftData";

/// How [`DataDir::dir`] was chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// `PHOTOCRAFT_CONFIG_DIR`.
    Override,
    /// `<exe dir>/PhotoCraftData`.
    Portable,
    /// The platform's per-user config directory.
    Platform,
}

/// The resolved data directory.
#[derive(Clone, Debug)]
pub struct DataDir {
    /// `None` only when no platform directory is known either (no `HOME`/`APPDATA`).
    pub dir: Option<PathBuf>,
    pub mode: Mode,
    /// Set when a portable marker was found but its data folder isn't writable.
    pub warning: Option<String>,
}

/// The data directory for this process, resolved once (the portable check probes the disk).
pub fn current() -> &'static DataDir {
    static DIR: OnceLock<DataDir> = OnceLock::new();
    DIR.get_or_init(|| {
        let exe = std::env::current_exe().ok();
        let env = |k: &str| std::env::var_os(k);
        let d = resolve(env, exe.as_deref().and_then(Path::parent));
        if d.mode == Mode::Platform
            && let (Some(dir), Some(legacy)) = (&d.dir, platform_dir_named(&env, LEGACY_DIR_NAME))
        {
            match adopt_legacy(&legacy, dir) {
                Ok(true) => log::info!("settings copied from {} to {}", legacy.display(), dir.display()),
                Ok(false) => {}
                Err(e) => log::warn!("the settings in {} could not be copied to {}: {e}", legacy.display(), dir.display()),
            }
        }
        if let Some(w) = &d.warning {
            log::warn!("{w}");
            eprintln!("photocraft: {w}");
        }
        log::info!("settings directory ({:?}): {:?}", d.mode, d.dir);
        d
    })
}

/// The per-user settings directory (see the module docs).
pub fn config_dir() -> Option<PathBuf> {
    current().dir.clone()
}

/// Resolve the data directory from the environment (`env`) and the executable's folder.
pub fn resolve(env: impl Fn(&str) -> Option<OsString>, exe_dir: Option<&Path>) -> DataDir {
    if let Some(d) = env("PHOTOCRAFT_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return DataDir { dir: Some(PathBuf::from(d)), mode: Mode::Override, warning: None };
    }
    let platform = platform_dir(&env);
    let Some((exe_dir, marker)) = exe_dir.and_then(|d| portable_marker(d).map(|m| (d, m))) else {
        return DataDir { dir: platform, mode: Mode::Platform, warning: None };
    };
    let data = exe_dir.join(PORTABLE_DATA_DIR);
    match ensure_writable(&data) {
        Ok(()) => DataDir { dir: Some(data), mode: Mode::Portable, warning: None },
        Err(e) => {
            let fallback = platform.as_ref().map_or_else(|| "nowhere (no user folder found)".to_string(), |p| p.display().to_string());
            let warning =
                format!("portable mode ({} found) but {} isn't writable ({e}); settings are kept in {fallback} instead", marker.display(), data.display());
            DataDir { dir: platform, mode: Mode::Platform, warning: Some(warning) }
        }
    }
}

/// The platform convention, from `env` (so tests can fake `HOME`/`APPDATA`/`XDG_CONFIG_HOME`).
pub fn platform_dir(env: &impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    platform_dir_named(env, DIR_NAME)
}

/// The platform convention for a folder called `name`.
pub fn platform_dir_named(env: &impl Fn(&str) -> Option<OsString>, name: &str) -> Option<PathBuf> {
    let home = env("HOME").filter(|h| !h.is_empty()).map(PathBuf::from);
    if cfg!(target_os = "macos") {
        return home.map(|h| h.join("Library/Application Support").join(name));
    }
    if cfg!(windows) {
        return env("APPDATA").filter(|a| !a.is_empty()).map(|a| PathBuf::from(a).join(name));
    }
    env("XDG_CONFIG_HOME").filter(|x| !x.is_empty()).map(PathBuf::from).or_else(|| home.map(|h| h.join(".config"))).map(|c| c.join(name.to_lowercase()))
}

/// Copy the settings of a build before the rename (`legacy`) to `dir`, when `dir` doesn't exist
/// yet and `legacy` does. Returns whether it copied. The copy is made beside `dir` and renamed
/// into place, so a copy cut short never passes for the settings.
pub fn adopt_legacy(legacy: &Path, dir: &Path) -> std::io::Result<bool> {
    if dir.exists() || !legacy.is_dir() || legacy == dir {
        return Ok(false);
    }
    let mut partial = dir.as_os_str().to_owned();
    partial.push(".copying");
    let partial = PathBuf::from(partial);
    let _ = std::fs::remove_dir_all(&partial);
    let copied = copy_dir(legacy, &partial, 0).and_then(|()| std::fs::rename(&partial, dir));
    if copied.is_err() {
        let _ = std::fs::remove_dir_all(&partial);
    }
    copied.map(|()| true)
}

/// Copy the folder `from` to `to`: files and folders, links not followed, at most 32 levels. A
/// file that can't be read (in use) is left out rather than costing the user the rest.
fn copy_dir(from: &Path, to: &Path, depth: u32) -> std::io::Result<()> {
    if depth > 32 {
        return Ok(());
    }
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_dir(&entry.path(), &target, depth + 1)?;
        } else if kind.is_file()
            && let Err(e) = std::fs::copy(entry.path(), &target)
        {
            log::warn!("settings: {} was not copied: {e}", entry.path().display());
        }
    }
    Ok(())
}

/// Where the Local AI setup puts the engine and the models unless the user chooses otherwise:
/// `(engine, models)`, in folders named after the product (`name`).
///
/// Not under the settings folder: the engine is several gigabytes of program, which on Windows
/// belongs in `%LOCALAPPDATA%` (it must not roam with a profile). A portable copy, or a settings
/// folder chosen with `PHOTOCRAFT_CONFIG_DIR`, keeps them beside its settings, so the whole
/// thing still moves as one folder.
pub fn local_ai_dirs(data: &DataDir, env: impl Fn(&str) -> Option<OsString>, name: &str) -> Option<(PathBuf, PathBuf)> {
    let own = |base: PathBuf| (base.join("engine"), base.join("models"));
    match data.mode {
        Mode::Override | Mode::Portable => data.dir.as_ref().map(|d| own(d.join("LocalAI"))),
        Mode::Platform if cfg!(windows) => env("LOCALAPPDATA").filter(|a| !a.is_empty()).map(|a| own(PathBuf::from(a).join(name))),
        Mode::Platform => data.dir.as_ref().map(|d| own(d.join("LocalAI"))),
    }
}

/// The first portable marker file in `exe_dir`, if any.
pub fn portable_marker(exe_dir: &Path) -> Option<PathBuf> {
    PORTABLE_MARKERS.iter().map(|m| exe_dir.join(m)).find(|p| p.is_file())
}

/// Create `dir` and prove a file can be written in it (a folder can exist yet be read-only).
fn ensure_writable(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let probe = dir.join(format!(".write-test-{}", std::process::id()));
    std::fs::write(&probe, b"")?;
    // A leftover probe is harmless; don't fail portable mode over it.
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn temp(tag: &str) -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!("photocraft-app-dirs-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn env_of(pairs: &[(&str, &Path)]) -> impl Fn(&str) -> Option<OsString> + use<> {
        let m: HashMap<String, OsString> = pairs.iter().map(|(k, v)| (k.to_string(), v.as_os_str().to_owned())).collect();
        move |k| m.get(k).cloned()
    }

    /// A fake user folder that every platform's convention resolves under.
    fn fake_home(root: &Path) -> impl Fn(&str) -> Option<OsString> + use<> {
        let home = root.join("home");
        let appdata = root.join("appdata");
        env_of(&[("HOME", &home), ("APPDATA", &appdata)])
    }

    #[test]
    fn no_marker_uses_the_platform_dir() {
        let root = temp("platform");
        let exe = root.join("bin");
        std::fs::create_dir_all(&exe).unwrap();
        let env = fake_home(&root);
        let d = resolve(&env, Some(&exe));
        assert_eq!(d.mode, Mode::Platform);
        assert!(d.warning.is_none());
        let dir = d.dir.unwrap();
        assert_eq!(Some(dir.clone()), platform_dir(&env));
        assert!(!dir.starts_with(&exe));
        assert!(!exe.join(PORTABLE_DATA_DIR).exists(), "no portable folder without a marker");
        // The convention per platform.
        if cfg!(target_os = "macos") {
            assert_eq!(dir, root.join("home/Library/Application Support/LayerShop"));
        } else if cfg!(windows) {
            assert_eq!(dir, root.join("appdata").join("LayerShop"));
        } else {
            assert_eq!(dir, root.join("home/.config/layershop"));
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn settings_from_before_the_rename_are_copied_once() {
        let root = temp("legacy");
        let env = fake_home(&root);
        let (legacy, dir) = (platform_dir_named(&env, LEGACY_DIR_NAME).unwrap(), platform_dir(&env).unwrap());
        // Nothing to copy: nothing happens.
        assert!(!adopt_legacy(&legacy, &dir).unwrap() && !dir.exists());
        std::fs::create_dir_all(legacy.join("Presets/Brushes")).unwrap();
        std::fs::write(legacy.join("preferences.json"), b"{\"a\":1}").unwrap();
        std::fs::write(legacy.join("Presets/Brushes/soft.json"), b"soft").unwrap();
        assert!(adopt_legacy(&legacy, &dir).unwrap());
        assert_eq!(std::fs::read(dir.join("preferences.json")).unwrap(), b"{\"a\":1}");
        assert_eq!(std::fs::read(dir.join("Presets/Brushes/soft.json")).unwrap(), b"soft");
        // The old folder stays (it may be PhotoCraft's), and no half copy is left beside the new one.
        assert!(legacy.join("preferences.json").is_file());
        assert_eq!(std::fs::read_dir(dir.parent().unwrap()).unwrap().count(), 2);
        // Once the new folder exists it is the settings: the old one is never copied over it again.
        std::fs::write(dir.join("preferences.json"), b"{\"a\":2}").unwrap();
        assert!(!adopt_legacy(&legacy, &dir).unwrap());
        assert_eq!(std::fs::read(dir.join("preferences.json")).unwrap(), b"{\"a\":2}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn either_marker_keeps_everything_beside_the_exe() {
        for marker in PORTABLE_MARKERS {
            let root = temp("portable");
            let exe = root.join("PhotoCraft");
            std::fs::create_dir_all(&exe).unwrap();
            std::fs::write(exe.join(marker), b"").unwrap();
            let d = resolve(fake_home(&root), Some(&exe));
            assert_eq!(d.mode, Mode::Portable, "{marker}");
            assert!(d.warning.is_none());
            let dir = d.dir.unwrap();
            assert_eq!(dir, exe.join(PORTABLE_DATA_DIR));
            assert!(dir.is_dir(), "the data folder is created");
            assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "the write probe is removed");
            // Every derived path sits under the exe dir.
            for sub in ["preferences.json", "Presets", "Recovery", crate::gpu_startup::MARKER_FILE] {
                assert!(dir.join(sub).starts_with(&exe));
            }
            assert!(!root.join("home").exists() && !root.join("appdata").exists(), "nothing written to the user folder");
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn a_marker_folder_is_not_a_marker() {
        let root = temp("dir-marker");
        let exe = root.join("bin");
        std::fs::create_dir_all(exe.join("portable.txt")).unwrap();
        assert_eq!(resolve(fake_home(&root), Some(&exe)).mode, Mode::Platform);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unwritable_portable_folder_falls_back_with_a_warning() {
        let root = temp("unwritable");
        let exe = root.join("bin");
        std::fs::create_dir_all(&exe).unwrap();
        std::fs::write(exe.join("portable.txt"), b"").unwrap();
        // A file where the data folder should go: it can't be created, on every platform.
        std::fs::write(exe.join(PORTABLE_DATA_DIR), b"in the way").unwrap();
        let env = fake_home(&root);
        let d = resolve(&env, Some(&exe));
        assert_eq!(d.mode, Mode::Platform);
        assert_eq!(d.dir, platform_dir(&env));
        let w = d.warning.unwrap();
        assert!(w.contains("portable") && w.contains("isn't writable"), "{w}");
        // No user folder at all: still no panic, just no directory.
        let d = resolve(|_| None, Some(&exe));
        assert!(d.dir.is_none() && d.warning.is_some());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn override_wins_over_portable() {
        let root = temp("override");
        let exe = root.join("bin");
        std::fs::create_dir_all(&exe).unwrap();
        std::fs::write(exe.join("portable.txt"), b"").unwrap();
        let custom = root.join("custom");
        let d = resolve(env_of(&[("PHOTOCRAFT_CONFIG_DIR", &custom)]), Some(&exe));
        assert_eq!((d.mode, d.dir), (Mode::Override, Some(custom)));
        assert!(!exe.join(PORTABLE_DATA_DIR).exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_exe_dir_or_env_never_panics() {
        let d = resolve(|_| None, None);
        assert_eq!((d.mode, d.dir), (Mode::Platform, None));
        let empty = PathBuf::new();
        let d = resolve(env_of(&[("PHOTOCRAFT_CONFIG_DIR", &empty), ("HOME", &empty), ("APPDATA", &empty)]), None);
        assert_eq!(d.dir, None, "empty variables are ignored");
    }
}
