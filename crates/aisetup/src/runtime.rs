//! The installed engine: how it is started, where it finds the models, and how it is removed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use photocraft_aigen::Launch;
use photocraft_aigen::launch::join_args;
use serde::{Deserialize, Serialize};

use crate::manifest::{FOLDERS, LEGACY_FOLDERS, Manifest, split_key};
use crate::plan::{Layout, MARKER, PART};
pub use crate::plan::{has_model_folders, models_root};
use crate::{Error, Result};

/// The first port tried for the engine. Not 8188 (the portable build's default) and not 8000
/// (ComfyUI Desktop's), so nothing collides with a ComfyUI the user installs themselves.
pub const FIRST_PORT: u16 = 8190;

/// What the marker file in an engine folder records.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Marker {
    /// The ComfyUI version.
    pub version: String,
    /// The build (`nvidia`, …) and the sha256 of the archive it was unpacked from.
    pub variant: String,
    pub sha256: String,
}

fn disk(path: &Path, what: &str, e: &std::io::Error) -> Error {
    Error::Disk(format!("Could not {what} `{}`: {e}", path.display()))
}

/// Write the marker into `dir` (the staging folder, before it becomes the engine).
pub fn write_marker(dir: &Path, marker: &Marker) -> Result<()> {
    let text = serde_json::to_string_pretty(marker).map_err(|e| Error::Disk(e.to_string()))?;
    let path = dir.join(MARKER);
    std::fs::write(&path, text).map_err(|e| disk(&path, "write", &e))
}

/// The marker of the engine in `root`, when it is one this app installed.
pub fn read_marker(root: &Path) -> Option<Marker> {
    let path = root.join(MARKER);
    if std::fs::metadata(&path).ok()?.len() > 64 * 1024 {
        return None;
    }
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// A YAML double-quoted scalar: any path survives it, whatever characters it has.
fn yaml_quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// ComfyUI's "extra model paths" file for `models_dir`: every model folder of it, searched
/// first (`is_default`), so what the wizard downloads is what the engine lists. Then the user's
/// own models folders (`extra`), read after it, with the older folder names they may use.
pub fn model_paths_yaml(models_dir: &Path, extra: &[PathBuf]) -> String {
    let mut out = String::from("# Written by the Local AI setup: where this engine finds its models.\nlayershop:\n");
    out.push_str(&format!("    base_path: {}\n    is_default: true\n", yaml_quoted(&models_dir.to_string_lossy())));
    for folder in FOLDERS {
        out.push_str(&format!("    {folder}: {folder}\n"));
    }
    for (i, dir) in extra.iter().enumerate() {
        out.push_str(&format!("yours{}:\n    base_path: {}\n", i + 1, yaml_quoted(&dir.to_string_lossy())));
        for folder in FOLDERS.iter().copied().chain(LEGACY_FOLDERS.iter().map(|(_, old)| *old)) {
            out.push_str(&format!("    {folder}: {folder}\n"));
        }
    }
    out
}

/// Write the model paths file, and make the folders it names in the setup's own models folder
/// (ComfyUI lists a missing one as empty, but a download into it should not have to create it
/// under a running server). Nothing is made in the user's own folders.
pub fn write_model_paths(layout: &Layout) -> Result<()> {
    for folder in FOLDERS {
        let dir = layout.models_dir.join(folder);
        std::fs::create_dir_all(&dir).map_err(|e| disk(&dir, "create the folder", &e))?;
    }
    std::fs::write(&layout.model_paths, model_paths_yaml(&layout.models_dir, &layout.extra_models)).map_err(|e| disk(&layout.model_paths, "write", &e))
}

/// The file types ComfyUI lists as models (its `supported_pt_extensions`). Others in a models
/// folder (`.gguf`, which needs a custom node) are not offered.
const MODEL_EXTENSIONS: [&str; 8] = ["safetensors", "sft", "ckpt", "pt", "pt2", "pth", "bin", "pkl"];

/// The model files in one of the user's own models folders, by folder, named as the engine lists
/// them: in subfolders too (`sub\name` on Windows), with what an older folder name holds
/// (`unet`) listed under the folder it stands for, and only the file types ComfyUI loads.
pub fn list_linked_models(root: &Path) -> BTreeMap<String, Vec<String>> {
    const DEPTH: usize = 8;
    const MAX_FILES: usize = 20_000;
    let mut out = BTreeMap::new();
    for folder in FOLDERS {
        let older = LEGACY_FOLDERS.iter().filter(|(now, _)| *now == folder).map(|(_, old)| *old);
        let mut names: Vec<String> = Vec::new();
        for source in std::iter::once(folder).chain(older) {
            // Folders to read, each with the name it adds in front of what is in it.
            let mut stack = vec![(root.join(source), String::new(), 0usize)];
            // Bounded: a models folder has tens of subfolders, not thousands.
            let mut visited = 0usize;
            while let Some((dir, prefix, depth)) = stack.pop() {
                visited += 1;
                if visited > 5_000 {
                    break;
                }
                let Ok(entries) = std::fs::read_dir(&dir) else { continue };
                for entry in entries.flatten().take(MAX_FILES) {
                    let Ok(name) = entry.file_name().into_string() else { continue };
                    // Followed through links, as ComfyUI does.
                    let Ok(meta) = std::fs::metadata(entry.path()) else { continue };
                    let shown = if prefix.is_empty() { name.clone() } else { format!("{prefix}{}{name}", std::path::MAIN_SEPARATOR) };
                    if meta.is_dir() {
                        if depth < DEPTH && name != ".git" {
                            stack.push((entry.path(), shown, depth + 1));
                        }
                    } else if name.rsplit_once('.').is_some_and(|(_, ext)| MODEL_EXTENSIONS.iter().any(|e| ext.eq_ignore_ascii_case(e)))
                        && names.len() < MAX_FILES
                        && !names.contains(&shown)
                    {
                        names.push(shown);
                    }
                }
            }
        }
        names.sort();
        if !names.is_empty() {
            out.insert(folder.to_string(), names);
        }
    }
    out
}

/// The model files the installed engine has, by folder, as it lists them: those in the setup's
/// own models folder, then those in the user's own folders (`extra`) that it doesn't have.
pub fn list_all_models(models_dir: &Path, extra: &[PathBuf]) -> BTreeMap<String, Vec<String>> {
    let mut out = list_models(models_dir);
    for root in extra {
        for (folder, names) in list_linked_models(root) {
            let have = out.entry(folder).or_default();
            for name in names {
                if !have.contains(&name) {
                    have.push(name);
                }
            }
        }
    }
    for names in out.values_mut() {
        names.sort();
    }
    out
}

/// How the installed engine is started: the portable build's own command line (its
/// `run_nvidia_gpu.bat`), without the browser it would open, with the models folder added.
/// `cpu`: on the processor, as its `run_cpu.bat` does, for a PC without a graphics card it can use.
pub fn launch(manifest: &Manifest, layout: &Layout, cpu: bool) -> Launch {
    let script = manifest.comfy.script.replace('/', std::path::MAIN_SEPARATOR_STR);
    let mut args = vec![
        // Python's `-s`: the user's own site-packages stay out of the engine.
        "-s".to_string(),
        script,
        "--windows-standalone-build".into(),
        "--disable-auto-launch".into(),
        "--extra-model-paths-config".into(),
        layout.model_paths.to_string_lossy().into_owned(),
    ];
    if cpu {
        args.push("--cpu".into());
    }
    Launch { python: layout.python.to_string_lossy().into_owned(), folder: layout.engine_root.to_string_lossy().into_owned(), args: join_args(&args) }
}

/// The first port from `from` up that nothing on this computer listens on.
pub fn free_port(from: u16) -> Option<u16> {
    (from..from.saturating_add(200)).find(|port| std::net::TcpListener::bind(("127.0.0.1", *port)).is_ok())
}

/// The address the engine answers at on `port`.
pub fn url(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

/// The model files in `models_dir`, by folder, as a ComfyUI server pointed at it lists them.
/// Parts of unfinished downloads are not files yet.
pub fn list_models(models_dir: &Path) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    for folder in FOLDERS {
        let Ok(entries) = std::fs::read_dir(models_dir.join(folder)) else { continue };
        let mut names: Vec<String> = entries
            .flatten()
            .take(20_000)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| !n.ends_with(PART) && !n.ends_with(".etag"))
            .collect();
        names.sort();
        if !names.is_empty() {
            out.insert(folder.to_string(), names);
        }
    }
    out
}

/// The bytes under `path` (a file, or a folder and everything in it). Links are not followed.
pub fn size_of(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return 0 };
    if meta.is_file() {
        return meta.len();
    }
    if !meta.is_dir() {
        return 0;
    }
    let mut total = 0u64;
    let mut stack = vec![path.to_path_buf()];
    // Bounded: an engine is under a hundred thousand folders.
    let mut visited = 0usize;
    while let Some(dir) = stack.pop() {
        visited += 1;
        if visited > 2_000_000 {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            match entry.file_type() {
                Ok(t) if t.is_dir() => stack.push(entry.path()),
                Ok(t) if t.is_file() => total = total.saturating_add(entry.metadata().map_or(0, |m| m.len())),
                _ => {}
            }
        }
    }
    total
}

/// The ComfyUI versions installed in `engine_dir`: the folders that carry this app's marker.
pub fn installed_engines(engine_dir: &Path) -> Vec<(PathBuf, Marker)> {
    let Ok(entries) = std::fs::read_dir(engine_dir) else { return Vec::new() };
    let mut out: Vec<(PathBuf, Marker)> = entries.flatten().take(1000).filter_map(|e| read_marker(&e.path()).map(|m| (e.path(), m))).collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Remove what the setup put in `engine_dir`: every engine that carries the marker, unfinished
/// unpacks and runtime archives. Anything else in the folder is not this app's and stays.
/// Returns the bytes freed.
pub fn remove_engines(engine_dir: &Path) -> Result<u64> {
    let mut freed = 0u64;
    let Ok(entries) = std::fs::read_dir(engine_dir) else { return Ok(0) };
    for entry in entries.flatten().take(10_000) {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() && (read_marker(&path).is_some() || name.ends_with(".unpacking")) {
            let size = size_of(&path);
            std::fs::remove_dir_all(&path).map_err(|e| disk(&path, "remove", &e))?;
            freed = freed.saturating_add(size);
        } else if kind.is_file()
            && name.starts_with("ComfyUI_windows_portable")
            && (name.ends_with(".7z") || name.ends_with(".7z.part") || name.ends_with(".7z.part.etag"))
        {
            let size = entry.metadata().map_or(0, |m| m.len());
            std::fs::remove_file(&path).map_err(|e| disk(&path, "remove", &e))?;
            freed = freed.saturating_add(size);
        }
    }
    // The folder itself, when nothing else was in it.
    let _ = std::fs::remove_dir(engine_dir);
    Ok(freed)
}

/// Remove the model files of `ids` from `models_dir`, keeping every file a model in `keep` still
/// uses. Only files the manifest lists are ever touched (and their unfinished parts): what the
/// user put in the folder themselves stays. Returns the bytes freed.
pub fn remove_models(manifest: &Manifest, models_dir: &Path, ids: &[String], keep: &[String]) -> Result<u64> {
    let kept: Vec<&str> = manifest.file_keys(manifest.models.iter().filter(|m| keep.contains(&m.id) && !ids.contains(&m.id)));
    let mut freed = 0u64;
    for key in manifest.file_keys(manifest.models.iter().filter(|m| ids.contains(&m.id))) {
        if kept.contains(&key) {
            continue;
        }
        let Some((folder, name)) = split_key(key) else { continue };
        let file = models_dir.join(folder).join(name);
        for path in [file.clone(), crate::plan::part_of(&file), PathBuf::from(format!("{}{PART}.etag", file.display()))] {
            let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
            if !meta.is_file() {
                continue;
            }
            std::fs::remove_file(&path).map_err(|e| disk(&path, "remove", &e))?;
            freed = freed.saturating_add(meta.len());
        }
    }
    Ok(freed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{Choices, MODEL_PATHS, same_folder};
    use photocraft_aigen::launch::split_args;

    fn manifest() -> &'static Manifest {
        Manifest::bundled().unwrap()
    }

    fn layout(dir: &Path) -> Layout {
        let choices = Choices {
            engine_dir: dir.join("engine dir").to_string_lossy().into_owned(),
            models_dir: dir.join("my models").to_string_lossy().into_owned(),
            variant: "nvidia".into(),
            ..Choices::default()
        };
        Layout::new(manifest(), &choices).unwrap()
    }

    #[test]
    fn the_engine_starts_as_the_portable_build_does_without_a_browser() {
        let dir = tempfile::tempdir().unwrap();
        let layout = layout(dir.path());
        let l = launch(manifest(), &layout, false);
        assert_eq!(l.python, layout.python.to_string_lossy());
        assert_eq!(l.folder, layout.engine_root.to_string_lossy());
        // The folder has a space in it: the arguments still split back into what was meant.
        let args = split_args(&l.args);
        let script = format!("ComfyUI{}main.py", std::path::MAIN_SEPARATOR);
        assert_eq!(args[..5], ["-s", script.as_str(), "--windows-standalone-build", "--disable-auto-launch", "--extra-model-paths-config"]);
        assert_eq!(args[5], layout.model_paths.to_string_lossy());
        assert!(args[5].ends_with(MODEL_PATHS) && !l.args.contains("--port") && !l.args.contains("--cpu"));
        // Without a graphics card it can use, the engine runs on the processor.
        let cpu = split_args(&launch(manifest(), &layout, true).args);
        assert_eq!((cpu.len(), cpu.last().map(String::as_str)), (args.len() + 1, Some("--cpu")));
    }

    #[test]
    fn the_model_paths_file_survives_any_folder_name() {
        let yaml = model_paths_yaml(Path::new("D:\\My \"AI\" models\\ünï: # stuff"), &[]);
        assert!(yaml.contains("    base_path: \"D:\\\\My \\\"AI\\\" models\\\\ünï: # stuff\"\n"), "{yaml}");
        assert!(yaml.contains("    is_default: true\n") && yaml.starts_with("# "));
        for folder in FOLDERS {
            assert!(yaml.contains(&format!("    {folder}: {folder}\n")));
        }
        assert_eq!(yaml_quoted("a\nb\u{7}"), "\"a\\u000Ab\\u0007\"");
        let dir = tempfile::tempdir().unwrap();
        let layout = layout(dir.path());
        std::fs::create_dir_all(&layout.engine_root).unwrap();
        write_model_paths(&layout).unwrap();
        assert!(layout.model_paths.is_file() && layout.models_dir.join("text_encoders").is_dir());
    }

    #[test]
    fn the_users_own_models_folders_are_read_after_the_setups_and_never_made() {
        let dir = tempfile::tempdir().unwrap();
        let mut layout = layout(dir.path());
        let (mine, theirs) = (dir.path().join("ComfyUIModels").join("models"), dir.path().join("old drive"));
        layout.extra_models = vec![mine.clone(), theirs.clone()];
        std::fs::create_dir_all(&layout.engine_root).unwrap();
        std::fs::create_dir_all(mine.join("loras")).unwrap();
        write_model_paths(&layout).unwrap();
        let yaml = std::fs::read_to_string(&layout.model_paths).unwrap();
        // Only one folder is where ComfyUI saves: the setup's own.
        assert_eq!(yaml.matches("is_default").count(), 1, "{yaml}");
        assert!(yaml.contains(&format!("yours1:\n    base_path: {}\n", yaml_quoted(&mine.to_string_lossy()))), "{yaml}");
        assert!(yaml.contains("yours2:") && yaml.contains("    unet: unet\n") && yaml.contains("    clip: clip\n"), "{yaml}");
        assert!(!theirs.exists() && !mine.join("vae").exists(), "nothing is made in the user's folders");
        // The app reads the file the way ComfyUI does: its own folder first, then the user's.
        let launch = launch(manifest(), &layout, false);
        let found = photocraft_aigen::launch::models_folders(&launch, &|p| std::fs::read_to_string(p).ok(), &|p| p.is_dir());
        let at = |p: &Path| found.iter().position(|f| same_folder(Path::new(f), p));
        assert!(at(&layout.models_dir) < at(&mine) && at(&mine).is_some(), "{found:?}");
    }

    #[test]
    fn a_linked_folder_lists_what_comfyui_would() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for file in [
            "loras/style.safetensors",
            "loras/qwen/lightning.safetensors",
            "loras/notes.txt",
            "unet/old_style.safetensors",
            "diffusion_models/z_image_turbo_bf16.safetensors",
            "diffusion_models/ltx-q4.gguf",
            "clip/t5.SAFETENSORS",
            "loras/.git/objects/x.pt",
        ] {
            let path = file.split('/').fold(root.to_path_buf(), |p, part| p.join(part));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"x").unwrap();
        }
        let sub = format!("qwen{}lightning.safetensors", std::path::MAIN_SEPARATOR);
        let listed = list_linked_models(root);
        assert_eq!(listed["loras"], vec![sub, "style.safetensors".to_string()]);
        assert_eq!(listed["diffusion_models"], vec!["old_style.safetensors".to_string(), "z_image_turbo_bf16.safetensors".to_string()]);
        assert_eq!(listed["text_encoders"], vec!["t5.SAFETENSORS".to_string()]);
        assert!(!listed.contains_key("vae") && list_linked_models(&root.join("missing")).is_empty());

        // Beside the setup's own folder: each name once.
        let own = dir.path().join("own");
        std::fs::create_dir_all(own.join("loras")).unwrap();
        std::fs::write(own.join("loras").join("style.safetensors"), b"x").unwrap();
        std::fs::write(own.join("loras").join("mine.safetensors"), b"x").unwrap();
        let all = list_all_models(&own, &[root.to_path_buf()]);
        assert_eq!(all["loras"].len(), 3, "{all:?}");
        assert!(all["loras"].contains(&"mine.safetensors".to_string()));
    }

    #[test]
    fn a_comfyui_folder_stands_for_the_models_folder_in_it() {
        let dir = tempfile::tempdir().unwrap();
        let comfy = dir.path().join("ComfyUI");
        std::fs::create_dir_all(comfy.join("models").join("checkpoints")).unwrap();
        assert_eq!(models_root(&comfy), comfy.join("models"));
        assert_eq!(models_root(&comfy.join("models")), comfy.join("models"));
        // Nothing to go by: what was picked.
        assert_eq!(models_root(dir.path()), dir.path().to_path_buf());
        assert!(has_model_folders(&comfy.join("models")) && !has_model_folders(&comfy));
    }

    #[test]
    fn a_free_port_is_one_nothing_listens_on() {
        let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = taken.local_addr().unwrap().port();
        let next = free_port(port).unwrap();
        assert!(next > port, "{next} vs {port}");
        assert_eq!(url(8190), "http://127.0.0.1:8190");
        assert!(free_port(u16::MAX).is_none() || free_port(u16::MAX) == Some(u16::MAX));
    }

    #[test]
    fn the_models_folder_is_listed_as_a_server_would() {
        let dir = tempfile::tempdir().unwrap();
        let vae = dir.path().join("vae");
        std::fs::create_dir_all(vae.join("sub")).unwrap();
        for name in ["b.safetensors", "a.safetensors", "c.safetensors.part", "c.safetensors.part.etag"] {
            std::fs::write(vae.join(name), b"x").unwrap();
        }
        std::fs::create_dir_all(dir.path().join("loras")).unwrap();
        std::fs::create_dir_all(dir.path().join("not_a_model_folder")).unwrap();
        let listed = list_models(dir.path());
        assert_eq!(listed, BTreeMap::from([("vae".to_string(), vec!["a.safetensors".to_string(), "b.safetensors".to_string()])]));
        assert!(list_models(&dir.path().join("missing")).is_empty());
        assert_eq!(size_of(&vae), 4);
        assert_eq!((size_of(&vae.join("a.safetensors")), size_of(&dir.path().join("missing"))), (1, 0));
    }

    #[test]
    fn removing_takes_only_what_the_setup_installed() {
        let dir = tempfile::tempdir().unwrap();
        let engines = dir.path().join("engine");
        let ours = engines.join("0.39.0");
        let theirs = engines.join("my notes");
        std::fs::create_dir_all(ours.join("python_embeded")).unwrap();
        std::fs::create_dir_all(&theirs).unwrap();
        std::fs::create_dir_all(engines.join("0.40.0.unpacking")).unwrap();
        std::fs::write(ours.join("python_embeded").join("python.exe"), vec![0u8; 1000]).unwrap();
        write_marker(&ours, &Marker { version: "0.39.0".into(), variant: "nvidia".into(), sha256: "ab".into() }).unwrap();
        std::fs::write(theirs.join("keep.txt"), b"mine").unwrap();
        std::fs::write(engines.join("ComfyUI_windows_portable_nvidia.7z.part"), vec![0u8; 500]).unwrap();
        std::fs::write(engines.join("holiday.7z"), b"mine too").unwrap();
        assert_eq!(installed_engines(&engines).iter().map(|(_, m)| m.version.as_str()).collect::<Vec<_>>(), ["0.39.0"]);
        let freed = remove_engines(&engines).unwrap();
        assert!(freed >= 1500, "{freed}");
        assert!(!ours.exists() && !engines.join("0.40.0.unpacking").exists() && !engines.join("ComfyUI_windows_portable_nvidia.7z.part").exists());
        assert!(theirs.join("keep.txt").is_file() && engines.join("holiday.7z").is_file());
        assert_eq!(remove_engines(&dir.path().join("missing")).unwrap(), 0);
        // A marker that is not one (a huge file, junk) marks nothing.
        std::fs::write(theirs.join(MARKER), b"not json").unwrap();
        assert!(read_marker(&theirs).is_none());
    }

    #[test]
    fn removing_a_model_keeps_the_files_another_still_uses() {
        let m = manifest();
        let dir = tempfile::tempdir().unwrap();
        let ids = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let installed = ids(&["qwen-image-2-1", "ideogram-4"]);
        for key in m.file_keys(m.select(&installed).unwrap()) {
            let (folder, name) = split_key(key).unwrap();
            std::fs::create_dir_all(dir.path().join(folder)).unwrap();
            std::fs::write(dir.path().join(folder).join(name), vec![0u8; 10]).unwrap();
        }
        std::fs::write(dir.path().join("vae").join("mine.safetensors"), b"mine").unwrap();
        std::fs::write(dir.path().join("diffusion_models").join("ideogram4_fp8_scaled.safetensors.part"), vec![0u8; 3]).unwrap();
        // Ideogram 4 goes; the text encoder it shares with Qwen Image 2.1 stays.
        let freed = remove_models(m, dir.path(), &ids(&["ideogram-4"]), &installed).unwrap();
        assert_eq!(freed, 33, "two diffusion models, a part, and the VAE only Ideogram used");
        assert!(dir.path().join("text_encoders").join("qwen3vl_8b_fp8_scaled.safetensors").is_file());
        assert!(!dir.path().join("vae").join("flux2-vae.safetensors").exists());
        assert!(dir.path().join("vae").join("mine.safetensors").is_file());
        // Removing everything listed still leaves the user's own file.
        remove_models(m, dir.path(), &installed, &[]).unwrap();
        assert_eq!(list_models(dir.path()), BTreeMap::from([("vae".to_string(), vec!["mine.safetensors".to_string()])]));
        assert_eq!(remove_models(m, &dir.path().join("missing"), &installed, &[]).unwrap(), 0);
    }
}
