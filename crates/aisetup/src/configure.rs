//! The settings a setup leaves behind: where the engine answers, how it is started, and which
//! models it can run.
//!
//! Pure functions over [`Settings`], applied when the install has finished: the user may have
//! changed other settings (an API key, a cloud model) while the files were coming down, so the
//! setup edits the settings as they are then instead of replacing them with a copy made at the
//! start.

use std::collections::BTreeMap;

use photocraft_aigen::{Launch, Model, Settings, detect};
use serde::{Deserialize, Serialize};

use crate::manifest::{Manifest, split_key};

/// What [`scan`] did to the model list.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Configured {
    /// Models added to the list, set up in a way that could not run and repaired, and pointed at
    /// the file the server has of them.
    pub added: usize,
    pub repaired: usize,
    pub matched: usize,
    /// Local models unticked because their files are not there.
    pub unlisted: usize,
    /// The ids of the local models that can run now.
    pub ready: Vec<String>,
}

/// The manifest models whose every file is in `folders` (as a server lists them).
pub fn installed(manifest: &Manifest, folders: &BTreeMap<String, Vec<String>>) -> Vec<String> {
    let has = |key: &str| {
        split_key(key).is_some_and(|(folder, name)| folders.get(folder).is_some_and(|files| files.iter().any(|f| f.rsplit(['/', '\\']).next() == Some(name))))
    };
    manifest.models.iter().filter(|m| m.files.iter().all(|k| has(k))).map(|m| m.id.clone()).collect()
}

/// Bring the model list in line with the files a server has (`folders`, by model folder): what
/// Manage Models › Scan Server does. A detected model the list lacks is added; one listed under
/// an architecture that can't run its file is repaired; a listed model whose own file the server
/// doesn't have is pointed at the file it has of the same model.
pub fn scan(settings: &mut Settings, folders: &BTreeMap<String, Vec<String>>) -> Configured {
    let mut out = Configured::default();
    for found in detect::detect(folders) {
        let same_file = |x: &Model| x.provider.is_local() && x.unet == found.unet && x.checkpoint == found.checkpoint;
        if settings.models.iter().any(|x| same_file(x) && x.family == found.family && detect::problem(x).is_none()) {
            continue;
        }
        // The list has this model already, stored another way, and the server has that file
        // too (HiDream O1's refiner is Z-Image Turbo at full precision): one entry is enough.
        if settings.models.iter().any(|x| detect::same_variant(x, &found) && detect::on_server(folders, x) && detect::problem(x).is_none()) {
            continue;
        }
        let broken = settings.models.iter().position(|x| same_file(x) && detect::problem(x).is_some());
        let stray = || settings.models.iter().position(|x| detect::same_variant(x, &found) && !detect::on_server(folders, x));
        let (at, fixed) = match broken {
            Some(i) => (Some(i), settings.models.get(i).map(|x| detect::repaired(x, &found))),
            None => {
                let i = stray();
                (i, i.and_then(|i| settings.models.get(i)).map(|x| detect::adopted(x, &found)))
            }
        };
        match (at.and_then(|i| settings.models.get_mut(i)), fixed) {
            (Some(slot), Some(fixed)) => {
                *slot = fixed;
                if broken.is_some() { out.repaired += 1 } else { out.matched += 1 }
            }
            _ => {
                let mut model = found;
                model.id = settings.unique_id(&model.name);
                settings.models.push(model);
                out.added += 1;
            }
        }
    }
    out
}

/// The local models of `settings` that `folders` has the main file of.
fn runnable(settings: &Settings, folders: &BTreeMap<String, Vec<String>>) -> Vec<String> {
    settings.models.iter().filter(|m| m.provider.is_local() && m.enabled && detect::on_server(folders, m)).map(|m| m.id.clone()).collect()
}

/// After the app installed its own engine: the address and launch settings are the engine's,
/// it is started when a local model needs it, and the model list says what it can run. The
/// models the wizard installed (`installed`, manifest ids) are ticked; local models whose files
/// are not in the engine's models folder are unticked, since this engine can't run them (they
/// stay in the list, and are ticked again when their files are added).
pub fn managed(settings: &mut Settings, manifest: &Manifest, url: &str, launch: &Launch, folders: &BTreeMap<String, Vec<String>>) -> Configured {
    settings.comfy_url = url.to_string();
    settings.comfy_launch = launch.clone();
    settings.comfy_autostart = true;
    let mut out = scan(settings, folders);
    let whole = installed(manifest, folders);
    for model in settings.models.iter_mut().filter(|m| m.provider.is_local() && m.family != photocraft_aigen::Family::Custom) {
        let listed_by_wizard = manifest.model(&model.id).is_some();
        let can_run = if listed_by_wizard { whole.contains(&model.id) } else { detect::on_server(folders, model) };
        if listed_by_wizard && can_run {
            model.enabled = true;
        } else if !can_run && model.enabled {
            model.enabled = false;
            out.unlisted += 1;
        }
    }
    if !whole.is_empty() {
        settings.show_local = true;
    }
    out.ready = runnable(settings, folders);
    out
}

/// After connecting the user's own ComfyUI: its address, how it is started when that is known
/// (and then that it is started when needed), and a scan of its files. Nothing is unticked:
/// what else the user set up there is theirs.
pub fn existing(settings: &mut Settings, manifest: &Manifest, url: &str, launch: Option<&Launch>, folders: &BTreeMap<String, Vec<String>>) -> Configured {
    settings.comfy_url = url.to_string();
    if let Some(launch) = launch.filter(|l| !l.python.trim().is_empty() && !l.folder.trim().is_empty()) {
        settings.comfy_launch = launch.clone();
        settings.comfy_autostart = true;
    }
    let mut out = scan(settings, folders);
    let whole = installed(manifest, folders);
    for model in settings.models.iter_mut().filter(|m| m.provider.is_local() && whole.contains(&m.id)) {
        model.enabled = true;
    }
    if !whole.is_empty() {
        settings.show_local = true;
    }
    out.ready = runnable(settings, folders);
    out
}

/// The model the test image is rendered with: the first the panel offers that the setup made
/// runnable and that generates from a prompt alone.
pub fn test_model<'a>(settings: &'a Settings, ready: &[String]) -> Option<&'a Model> {
    settings.models.iter().find(|m| m.provider.is_local() && m.enabled && m.can_generate() && ready.contains(&m.id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_aigen::Family;

    fn manifest() -> &'static Manifest {
        Manifest::bundled().unwrap()
    }

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn folders(models: &[&str]) -> BTreeMap<String, Vec<String>> {
        let m = manifest();
        m.folders(m.select(&ids(models)).unwrap())
    }

    fn launch() -> Launch {
        Launch { python: "D:/e/python_embeded/python.exe".into(), folder: "D:/e".into(), args: "-s ComfyUI/main.py".into() }
    }

    fn local_enabled(s: &Settings) -> Vec<&str> {
        s.models.iter().filter(|m| m.provider.is_local() && m.enabled).map(|m| m.id.as_str()).collect()
    }

    #[test]
    fn a_fresh_install_lists_exactly_what_was_installed() {
        let mut s = Settings::default();
        let cloud_before: Vec<Model> = s.models.iter().filter(|m| !m.provider.is_local()).cloned().collect();
        let done = managed(&mut s, manifest(), "http://127.0.0.1:8190", &launch(), &folders(&["qwen-image-2-1"]));
        assert_eq!((s.comfy_url.as_str(), s.comfy_autostart, &s.comfy_launch), ("http://127.0.0.1:8190", true, &launch()));
        assert_eq!(local_enabled(&s), ["qwen-image-2-1"]);
        assert_eq!(done.ready, ["qwen-image-2-1"]);
        // The nine other local defaults are unticked, not removed; nothing was added twice.
        assert_eq!((done.added, done.repaired, done.unlisted), (0, 0, 9));
        assert_eq!(s.models.len(), Settings::default().models.len());
        // Cloud models are not the engine's business.
        assert_eq!(s.models.iter().filter(|m| !m.provider.is_local()).cloned().collect::<Vec<_>>(), cloud_before);
        assert_eq!(test_model(&s, &done.ready).map(|m| m.id.as_str()), Some("qwen-image-2-1"));
    }

    #[test]
    fn adding_models_later_ticks_them_again() {
        let mut s = Settings::default();
        managed(&mut s, manifest(), "http://127.0.0.1:8190", &launch(), &folders(&["qwen-image-2-1"]));
        let done = managed(&mut s, manifest(), "http://127.0.0.1:8190", &launch(), &folders(&["qwen-image-2-1", "z-image-turbo", "hidream-o1"]));
        assert_eq!(local_enabled(&s), ["qwen-image-2-1", "z-image-turbo", "hidream-o1"]);
        assert_eq!((done.unlisted, done.added), (0, 0));
        // HiDream O1 keeps its prompt writer and refiner: their files came with it.
        let o1 = s.model("hidream-o1").unwrap();
        assert_eq!((o1.enhancer.as_str(), o1.refiner.as_str()), ("gemma4_e4b_it_fp8_scaled.safetensors", "z_image_turbo_bf16.safetensors"));
    }

    #[test]
    fn a_model_missing_one_of_its_files_is_not_offered() {
        let mut files = folders(&["qwen-image-2-1", "z-image-turbo"]);
        // Z-Image Turbo's text encoder did not arrive.
        files.get_mut("text_encoders").unwrap().retain(|f| f != "qwen_3_4b.safetensors");
        assert_eq!(installed(manifest(), &files), ["qwen-image-2-1"]);
        let mut s = Settings::default();
        let done = managed(&mut s, manifest(), "http://127.0.0.1:8190", &launch(), &files);
        assert_eq!(local_enabled(&s), ["qwen-image-2-1"]);
        assert_eq!(done.ready, ["qwen-image-2-1"]);
        // Files in subfolders count, as a server lists them.
        let nested = BTreeMap::from([
            ("diffusion_models".to_string(), vec!["sub/z_image_turbo_int8_convrot.safetensors".to_string()]),
            ("text_encoders".to_string(), vec!["qwen_3_4b.safetensors".to_string()]),
            ("vae".to_string(), vec!["ae.safetensors".to_string()]),
        ]);
        assert_eq!(installed(manifest(), &nested), ["z-image-turbo"]);
        assert!(installed(manifest(), &BTreeMap::new()).is_empty());
    }

    #[test]
    fn the_users_own_models_survive_an_install() {
        let mut s = Settings::default();
        let custom = Model { id: "mine".into(), name: "My workflow".into(), family: Family::Custom, workflow: "{}".into(), ..Model::default() };
        let elsewhere = Model { id: "sdxl".into(), name: "SDXL".into(), checkpoint: "sdxl_base_1.0.safetensors".into(), ..Family::Checkpoint.preset() };
        s.models.push(custom.clone());
        s.models.push(elsewhere);
        let done = managed(&mut s, manifest(), "http://127.0.0.1:8190", &launch(), &folders(&["z-image-turbo"]));
        // A custom workflow has no file to be missing: it is left as it was.
        assert_eq!(s.model("mine"), Some(&custom));
        // A model whose file is on another server can't run on this engine: unticked, kept.
        assert_eq!(s.model("sdxl").map(|m| m.enabled), Some(false));
        assert_eq!(done.unlisted, 10);
        assert_eq!(local_enabled(&s), ["z-image-turbo", "mine"]);
    }

    #[test]
    fn connecting_the_users_comfyui_scans_it_and_unticks_nothing() {
        let mut s = Settings::default();
        let mut files = folders(&["qwen-image-2-1"]);
        // Their server stores Z-Image Turbo another way, and has a model the list doesn't know.
        files.get_mut("diffusion_models").unwrap().extend(["z_image_turbo_bf16.safetensors".to_string(), "flux1-dev.safetensors".to_string()]);
        files.get_mut("text_encoders").unwrap().extend(["qwen_3_4b.safetensors".to_string(), "clip_l.safetensors".to_string()]);
        files.get_mut("vae").unwrap().push("ae.safetensors".to_string());
        let before = local_enabled(&s).len();
        let done = existing(&mut s, manifest(), "http://127.0.0.1:8000", None, &files);
        assert_eq!((s.comfy_url.as_str(), s.comfy_autostart), ("http://127.0.0.1:8000", false));
        assert_eq!((done.added, done.matched, done.unlisted), (1, 1, 0));
        assert_eq!(s.model("z-image-turbo").map(|m| m.unet.as_str()), Some("z_image_turbo_bf16.safetensors"));
        assert_eq!(local_enabled(&s).len(), before + 1);
        assert!(done.ready.contains(&"qwen-image-2-1".to_string()) && done.ready.contains(&"z-image-turbo".to_string()));
        // With its launch settings known, it is started when needed; half of them is not enough.
        existing(&mut s, manifest(), "http://127.0.0.1:8000", Some(&Launch { python: "p".into(), ..Launch::default() }), &files);
        assert!(!s.comfy_autostart);
        existing(&mut s, manifest(), "http://127.0.0.1:8000", Some(&launch()), &files);
        assert!(s.comfy_autostart && s.comfy_launch == launch());
        // Scanning again changes nothing.
        let again = scan(&mut s.clone(), &files);
        assert_eq!((again.added, again.repaired, again.matched), (0, 0, 0));
    }

    #[test]
    fn a_model_set_up_under_the_wrong_architecture_is_repaired_in_place() {
        let mut s = Settings::default_without_models();
        s.models.push(Model { id: "old".into(), name: "my qwen".into(), unet: "qwen_image_2.1_int8_convrot.safetensors".into(), ..Family::QwenImage.preset() });
        let done = scan(&mut s, &folders(&["qwen-image-2-1"]));
        assert_eq!((done.added, done.repaired), (0, 1));
        assert_eq!((s.models.len(), s.models[0].family, s.models[0].id.as_str(), s.models[0].name.as_str()), (1, Family::QwenImage21, "old", "my qwen"));
        // No model can render the test image when none is ready.
        assert!(test_model(&s, &[]).is_none());
        assert!(scan(&mut Settings::default(), &BTreeMap::new()) == Configured::default());
    }
}
