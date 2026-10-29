//! What a setup will do, as data.
//!
//! [`plan`] is a pure function from the manifest, the user's choices and what is on disk to the
//! ordered list of steps. Each step says whether it is already done, so there is no separate
//! resume code to get wrong: resuming is planning again and running what is left. Repairing is
//! the same with the checksums looked at again; adding a model later is the same with a longer
//! list.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::manifest::{Manifest, ModelEntry, split_key};
use crate::{Error, Result};

/// Written into the engine folder when the runtime is unpacked whole: its presence is what
/// "the engine is installed" means, and what marks the folder as one this app may remove.
pub const MARKER: &str = "layershop-engine.json";
/// ComfyUI's list of extra model folders, pointing it at the models folder.
pub const MODEL_PATHS: &str = "layershop-models.yaml";
/// A download in progress, beside where the finished file goes.
pub const PART: &str = ".part";
/// Room kept free on a drive beyond what the plan needs.
pub const HEADROOM: u64 = 2_000_000_000;
/// The longest path Windows opens without long-path support, which Python may not have.
pub const MAX_PATH: usize = 259;

/// Who installs ComfyUI.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Route {
    /// The app installs a private copy, with its own Python.
    #[default]
    Managed,
    /// The user's own ComfyUI: nothing is installed but the model files it is missing.
    Existing,
}

/// What the user decided on the wizard's screens.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Choices {
    pub route: Route,
    /// Ids of the manifest's models.
    pub models: Vec<String>,
    /// Managed: where the engine goes (a folder per ComfyUI version inside it).
    pub engine_dir: String,
    /// Where model files go: a ComfyUI `models` directory.
    pub models_dir: String,
    /// Managed: the build for the graphics card (a key of the manifest's assets).
    pub variant: String,
    /// Managed: no graphics card the engine can use, and the user installs anyway. The engine
    /// runs on the processor (`--cpu`): it starts, but these models render far too slowly there
    /// for a test image.
    pub cpu: bool,
    /// The user stored a Hugging Face token: gated files can be asked for.
    pub token: bool,
    /// Look at the checksum of every file that is there (Repair), not only its size.
    pub recheck: bool,
    /// Existing: the model files the server lists, by folder. A file it has is not downloaded.
    pub server_files: Option<BTreeMap<String, Vec<String>>>,
    /// Render a test image at the end.
    pub test_image: bool,
}

/// One thing a setup does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Step {
    /// Download the runtime archive.
    FetchRuntime,
    /// Unpack it into the engine folder.
    UnpackRuntime,
    /// Download a model file (a key of the manifest's files).
    FetchFile(String),
    /// Tell the engine where the models folder is.
    WriteModelPaths,
    /// Work out the settings: address, launch, models.
    Configure,
    /// Start the engine, and render the test image.
    Verify,
}

impl Step {
    /// A stable name for the step, for progress and for tests.
    pub fn key(&self) -> String {
        match self {
            Step::FetchRuntime => "runtime".into(),
            Step::UnpackRuntime => "unpack".into(),
            Step::FetchFile(key) => format!("file:{key}"),
            Step::WriteModelPaths => "paths".into(),
            Step::Configure => "configure".into(),
            Step::Verify => "verify".into(),
        }
    }

    pub fn is_download(&self) -> bool {
        matches!(self, Step::FetchRuntime | Step::FetchFile(_))
    }
}

/// A step with what the plan knows about it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Planned {
    pub step: Step,
    /// Shown on the Install screen.
    pub label: String,
    /// The size of what the step produces: bytes to download, or bytes to unpack. 0 for the rest.
    pub bytes: u64,
    /// How much of a download is already there (a `.part` file).
    pub have: u64,
    /// Nothing left to do.
    pub done: bool,
    /// The file is there and its checksum is to be looked at before it counts as done.
    pub recheck: bool,
    /// Why the step is left out (a gated file without a sign-in), when it is.
    pub skipped: Option<String>,
}

/// A model of the selection, and whether the plan can install it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedModel {
    pub id: String,
    pub name: String,
    /// Why it is left out (gated, no sign-in), when it is.
    pub skipped: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub steps: Vec<Planned>,
    pub models: Vec<PlannedModel>,
    /// Bytes still to download.
    pub download_bytes: u64,
    /// Bytes this plan still adds to the engine's drive and to the models' drive.
    pub engine_bytes: u64,
    pub models_bytes: u64,
}

impl Plan {
    /// Is there anything left that changes the disk?
    pub fn pending(&self) -> impl Iterator<Item = &Planned> {
        self.steps.iter().filter(|s| !s.done && s.skipped.is_none())
    }

    /// The ids of the models that are installed once the plan has run.
    pub fn installable(&self) -> Vec<String> {
        self.models.iter().filter(|m| m.skipped.is_none()).map(|m| m.id.clone()).collect()
    }
}

/// What [`plan`] asks about the disk. The tests answer from a table.
pub trait Disk {
    /// The length of the file at `path`, when there is one.
    fn len(&self, path: &Path) -> Option<u64>;
}

/// The real disk.
pub struct RealDisk;

impl Disk for RealDisk {
    fn len(&self, path: &Path) -> Option<u64> {
        std::fs::metadata(path).ok().filter(std::fs::Metadata::is_file).map(|m| m.len())
    }
}

/// Where a setup's files are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    /// The folder that holds one engine per ComfyUI version.
    pub engine_dir: PathBuf,
    /// This version's engine: the unpacked portable build (its Python, `ComfyUI`, …).
    pub engine_root: PathBuf,
    /// The downloaded runtime archive.
    pub archive: PathBuf,
    pub marker: PathBuf,
    pub model_paths: PathBuf,
    pub python: PathBuf,
    pub script: PathBuf,
    pub models_dir: PathBuf,
}

impl Layout {
    pub fn new(manifest: &Manifest, choices: &Choices) -> Result<Layout> {
        let models_dir = folder(&choices.models_dir, "models")?;
        let engine_dir = match choices.route {
            Route::Managed => folder(&choices.engine_dir, "engine")?,
            // Nothing is installed there; the paths under it are never used.
            Route::Existing => PathBuf::new(),
        };
        let engine_root = engine_dir.join(&manifest.comfy.version);
        let archive_name = match choices.route {
            Route::Managed => {
                let url = &manifest.asset(&choices.variant)?.url;
                url.rsplit('/')
                    .next()
                    .filter(|n| n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
                    .unwrap_or("runtime.7z")
                    .to_string()
            }
            Route::Existing => String::new(),
        };
        let inside = |rel: &str| rel.split('/').fold(engine_root.clone(), |p, part| p.join(part));
        Ok(Layout {
            archive: engine_dir.join(archive_name),
            marker: engine_root.join(MARKER),
            model_paths: engine_root.join(MODEL_PATHS),
            python: inside(&manifest.comfy.python),
            script: inside(&manifest.comfy.script),
            engine_dir,
            engine_root,
            models_dir,
        })
    }

    /// Where a model file (a key of the manifest's files) goes.
    pub fn model_file(&self, key: &str) -> Result<PathBuf> {
        let (folder, name) = split_key(key).ok_or_else(|| Error::Config(format!("`{key}` is not a model file")))?;
        Ok(self.models_dir.join(folder).join(name))
    }
}

/// `path` with [`PART`] appended to its file name.
pub fn part_of(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(PART);
    PathBuf::from(name)
}

/// A folder the user chose: an absolute path, with nothing in it that climbs back out.
fn folder(text: &str, what: &str) -> Result<PathBuf> {
    let text = text.trim();
    if text.is_empty() {
        return Err(Error::Config(format!("choose a folder for the {what}")));
    }
    if text.len() > 1024 || text.chars().any(char::is_control) {
        return Err(Error::Config(format!("that is not a folder for the {what}")));
    }
    let path = PathBuf::from(text);
    if !path.is_absolute() || path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err(Error::Config(format!("the {what} folder must be a full path (like D:\\AI\\{what}), not `{text}`")));
    }
    Ok(path)
}

/// Why the engine folder can't be used, when its paths would be too long for Windows.
pub fn path_too_long(asset: &crate::manifest::Asset, layout: &Layout) -> Option<String> {
    let longest = asset.longest_path as usize;
    let root = layout.engine_root.as_os_str().len();
    (cfg!(windows) && longest > 0 && root.saturating_add(1).saturating_add(longest) > MAX_PATH).then(|| {
        format!(
            "The engine folder's path is too long: files inside it would be {} characters, and Windows stops at {MAX_PATH}. Choose a shorter folder, such as C:\\LayerShop\\engine.",
            root.saturating_add(1).saturating_add(longest)
        )
    })
}

fn has_file(list: &BTreeMap<String, Vec<String>>, folder: &str, name: &str) -> bool {
    // A server lists files in subfolders as `sub/name`.
    list.get(folder).is_some_and(|files| files.iter().any(|f| f.rsplit(['/', '\\']).next() == Some(name)))
}

/// The steps that bring the disk from what `disk` says is there to what `choices` ask for.
pub fn plan(manifest: &Manifest, choices: &Choices, disk: &dyn Disk) -> Result<Plan> {
    let layout = Layout::new(manifest, choices)?;
    let selected = manifest.select(&choices.models)?;
    let managed = choices.route == Route::Managed;
    let mut out = Plan::default();

    // A gated file can't be had without a sign-in: its models are left out, whole, so their
    // other files aren't downloaded for nothing. Everything else still installs.
    let blocked = |m: &ModelEntry| !choices.token && m.files.iter().any(|k| manifest.files.get(k).is_some_and(|f| f.gated));
    for m in &selected {
        let skipped = blocked(m).then(|| "needs a Hugging Face sign-in".to_string());
        out.models.push(PlannedModel { id: m.id.clone(), name: m.name.clone(), skipped });
    }
    let wanted: Vec<&ModelEntry> = selected.iter().copied().filter(|m| !blocked(m)).collect();
    let wanted_keys: BTreeSet<&str> = manifest.file_keys(wanted.iter().copied()).into_iter().collect();

    if managed {
        let asset = manifest.asset(&choices.variant)?;
        let installed = disk.len(&layout.marker).is_some() && disk.len(&layout.python).is_some();
        let fetched = installed || disk.len(&layout.archive) == Some(asset.size);
        let have = if fetched { 0 } else { disk.len(&part_of(&layout.archive)).unwrap_or(0).min(asset.size) };
        out.steps.push(Planned {
            step: Step::FetchRuntime,
            label: format!("ComfyUI {} engine", manifest.comfy.version),
            bytes: asset.size,
            have,
            done: fetched,
            recheck: false,
            skipped: None,
        });
        out.steps.push(Planned {
            step: Step::UnpackRuntime,
            label: "Unpack the engine".into(),
            bytes: asset.unpacked_bytes,
            have: 0,
            done: installed,
            recheck: false,
            skipped: None,
        });
        if !installed {
            // The archive is deleted once it is unpacked, but both are on the drive for a while.
            out.engine_bytes = asset.unpacked_bytes.saturating_add(if fetched { 0 } else { asset.size.saturating_sub(have) });
        }
        if !fetched {
            out.download_bytes = out.download_bytes.saturating_add(asset.size.saturating_sub(have));
        }
    }

    // Gated files first: they start while a sign-in is fresh (its access token lasts hours, and
    // a large setup can take longer).
    let mut keys = manifest.file_keys(selected.iter().copied());
    keys.sort_by_key(|key| !manifest.files.get(*key).is_some_and(|f| f.gated));
    for key in keys {
        let Some(file) = manifest.files.get(key) else { continue };
        let (folder, name) = split_key(key).ok_or_else(|| Error::Config(format!("`{key}` is not a model file")))?;
        let path = layout.model_file(key)?;
        let skipped = (!wanted_keys.contains(key)).then(|| "needs a Hugging Face sign-in".to_string());
        // On the user's own server, a file it lists is there, wherever its folder is.
        let on_server = choices.server_files.as_ref().is_some_and(|list| has_file(list, folder, name));
        let there = disk.len(&path) == Some(file.size);
        let done = on_server || (there && !choices.recheck);
        let have = if done || there { 0 } else { disk.len(&part_of(&path)).unwrap_or(0).min(file.size) };
        if !done && !there && skipped.is_none() {
            let left = file.size.saturating_sub(have);
            out.download_bytes = out.download_bytes.saturating_add(left);
            out.models_bytes = out.models_bytes.saturating_add(left);
        }
        out.steps.push(Planned {
            step: Step::FetchFile(key.to_string()),
            label: name.to_string(),
            bytes: file.size,
            have,
            done,
            recheck: there && choices.recheck && !on_server,
            skipped,
        });
    }

    let cheap = |step: Step, label: &str| Planned { step, label: label.into(), bytes: 0, have: 0, done: false, recheck: false, skipped: None };
    if managed {
        out.steps.push(cheap(Step::WriteModelPaths, "Point the engine at your models"));
    }
    out.steps.push(cheap(Step::Configure, "Set up the models"));
    if choices.test_image {
        let label = if managed && choices.cpu { "Start the engine on the processor" } else { "Start the engine and render a test image" };
        out.steps.push(cheap(Step::Verify, label));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct Fake(HashMap<PathBuf, u64>);

    impl Disk for Fake {
        fn len(&self, path: &Path) -> Option<u64> {
            self.0.get(path).copied()
        }
    }

    fn manifest() -> &'static Manifest {
        Manifest::bundled().unwrap()
    }

    fn root(name: &str) -> String {
        if cfg!(windows) { format!("D:\\LS\\{name}") } else { format!("/ls/{name}") }
    }

    fn choices(models: &[&str]) -> Choices {
        Choices {
            models: models.iter().map(|s| s.to_string()).collect(),
            engine_dir: root("engine"),
            models_dir: root("models"),
            variant: "nvidia".into(),
            test_image: true,
            ..Choices::default()
        }
    }

    fn keys(plan: &Plan) -> Vec<String> {
        plan.steps.iter().map(|s| s.step.key()).collect()
    }

    #[test]
    fn a_clean_machine_gets_the_runtime_then_each_file_once_then_the_settings() {
        let m = manifest();
        let p = plan(m, &choices(&["qwen-image-2-1"]), &Fake::default()).unwrap();
        assert_eq!(
            keys(&p),
            [
                "runtime",
                "unpack",
                "file:diffusion_models/qwen_image_2.1_int8_convrot.safetensors",
                "file:text_encoders/qwen3vl_8b_fp8_scaled.safetensors",
                "file:vae/qwen_image_2.1_vae_bf16.safetensors",
                "paths",
                "configure",
                "verify",
            ]
        );
        assert!(p.steps.iter().all(|s| !s.done && s.skipped.is_none()));
        let asset = m.asset("nvidia").unwrap().size;
        let models = m.download_bytes(&choices(&["qwen-image-2-1"]).models);
        assert_eq!(p.download_bytes, asset + models);
        assert_eq!((p.models_bytes, p.engine_bytes), (models, m.asset("nvidia").unwrap().unpacked_bytes + asset));
        assert_eq!(p.installable(), ["qwen-image-2-1"]);
        // Without the test image the last step is left out.
        let quiet = plan(m, &Choices { test_image: false, ..choices(&["qwen-image-2-1"]) }, &Fake::default()).unwrap();
        assert_eq!(keys(&quiet).last().map(String::as_str), Some("configure"));
    }

    #[test]
    fn shared_files_are_planned_once_for_every_preset() {
        let m = manifest();
        for preset in ["light", "starter", "recommended", "everything"] {
            let ids: Vec<&str> = m.preset(preset).iter().map(String::as_str).collect();
            let p = plan(m, &Choices { token: true, ..choices(&ids) }, &Fake::default()).unwrap();
            let files: Vec<String> = keys(&p).into_iter().filter(|k| k.starts_with("file:")).collect();
            let unique: BTreeSet<&String> = files.iter().collect();
            assert_eq!(files.len(), unique.len(), "{preset}");
            assert_eq!(p.models_bytes, m.download_bytes(m.preset(preset)), "{preset}");
        }
    }

    #[test]
    fn resuming_is_planning_again() {
        let m = manifest();
        let c = choices(&["qwen-image-2-1"]);
        let layout = Layout::new(m, &c).unwrap();
        let unet = "diffusion_models/qwen_image_2.1_int8_convrot.safetensors";
        let encoder = "text_encoders/qwen3vl_8b_fp8_scaled.safetensors";
        let mut disk = Fake::default();
        // Killed while the runtime was coming down: the part is counted.
        disk.0.insert(part_of(&layout.archive), 1_000_000_000);
        let p = plan(m, &c, &disk).unwrap();
        let asset = m.asset("nvidia").unwrap().size;
        assert_eq!((p.steps[0].have, p.steps[0].done), (1_000_000_000, false));
        assert_eq!(p.download_bytes, asset - 1_000_000_000 + m.download_bytes(&c.models));
        // The archive is whole, not yet unpacked: fetching is done, unpacking is not.
        disk.0.clear();
        disk.0.insert(layout.archive.clone(), asset);
        let p = plan(m, &c, &disk).unwrap();
        assert_eq!((p.steps[0].done, p.steps[1].done, p.engine_bytes), (true, false, m.asset("nvidia").unwrap().unpacked_bytes));
        // Unpacked (the archive is gone again), one file finished and one half there.
        disk.0.clear();
        disk.0.insert(layout.marker.clone(), 80);
        disk.0.insert(layout.python.clone(), 100_000);
        disk.0.insert(layout.model_file(unet).unwrap(), m.files[unet].size);
        disk.0.insert(part_of(&layout.model_file(encoder).unwrap()), 5_000_000_000);
        let p = plan(m, &c, &disk).unwrap();
        let by = |key: &str| p.steps.iter().find(|s| s.step.key() == key).unwrap();
        assert!(by("runtime").done && by("unpack").done && by(&format!("file:{unet}")).done);
        assert_eq!((by(&format!("file:{encoder}")).have, by(&format!("file:{encoder}")).done), (5_000_000_000, false));
        assert_eq!(p.engine_bytes, 0);
        assert_eq!(p.download_bytes, m.files[encoder].size - 5_000_000_000 + m.files["vae/qwen_image_2.1_vae_bf16.safetensors"].size);
        // The cheap steps always run again: they are what repairs the settings.
        assert_eq!(p.pending().map(|s| s.step.key()).filter(|k| !k.starts_with("file:")).collect::<Vec<_>>(), ["paths", "configure", "verify"]);
        // A file of the wrong length is not done, and a part longer than the file counts as the file.
        disk.0.insert(layout.model_file(unet).unwrap(), 12);
        disk.0.insert(part_of(&layout.model_file(unet).unwrap()), u64::MAX);
        let p = plan(m, &c, &disk).unwrap();
        let by = |key: &str| p.steps.iter().find(|s| s.step.key() == key).unwrap();
        assert_eq!((by(&format!("file:{unet}")).done, by(&format!("file:{unet}")).have), (false, m.files[unet].size));
        // A marker without its Python is not an engine.
        disk.0.remove(&layout.python);
        assert!(!plan(m, &c, &disk).unwrap().steps[1].done);
    }

    #[test]
    fn a_repair_looks_at_every_file_that_is_there() {
        let m = manifest();
        let c = Choices { recheck: true, ..choices(&["qwen-image-2-1"]) };
        let layout = Layout::new(m, &c).unwrap();
        let unet = "diffusion_models/qwen_image_2.1_int8_convrot.safetensors";
        let mut disk = Fake::default();
        disk.0.insert(layout.model_file(unet).unwrap(), m.files[unet].size);
        let p = plan(m, &c, &disk).unwrap();
        let step = p.steps.iter().find(|s| s.step.key() == format!("file:{unet}")).unwrap();
        // Not done until its checksum has been looked at, but nothing is counted as a download.
        assert!(!step.done && step.recheck);
        assert_eq!(p.models_bytes, m.download_bytes(&c.models) - m.files[unet].size);
        // The missing ones are plain downloads.
        assert!(p.steps.iter().filter(|s| s.step.key().starts_with("file:") && s.step.key() != format!("file:{unet}")).all(|s| !s.recheck && !s.done));
    }

    #[test]
    fn a_gated_model_is_left_out_whole_without_a_sign_in() {
        let m = manifest();
        let recommended: Vec<&str> = m.preset("recommended").iter().map(String::as_str).collect();
        let p = plan(m, &choices(&recommended), &Fake::default()).unwrap();
        assert_eq!(p.installable(), ["qwen-image-2-1", "z-image-turbo"]);
        let klein = p.models.iter().find(|x| x.id == "flux-3-klein").unwrap();
        assert!(klein.skipped.as_deref().unwrap().contains("Hugging Face"));
        // Its encoder and VAE are not downloaded for nothing either.
        let skipped: Vec<String> = p.steps.iter().filter(|s| s.skipped.is_some()).map(|s| s.step.key()).collect();
        assert_eq!(
            skipped,
            [
                "file:diffusion_models/flux3_image_klein_base_int8_convrot.safetensors",
                "file:text_encoders/qwen3vl_4b_int8_convrot.safetensors",
                "file:vae/flux2-vae.safetensors"
            ]
        );
        assert_eq!(p.models_bytes, m.download_bytes(&p.installable()));
        // With Ideogram 4 chosen too, the VAE they share is wanted after all.
        let mut with = recommended.clone();
        with.push("ideogram-4");
        let p = plan(m, &choices(&with), &Fake::default()).unwrap();
        assert!(p.steps.iter().find(|s| s.step.key() == "file:vae/flux2-vae.safetensors").unwrap().skipped.is_none());
        // Signed in: everything.
        let p = plan(m, &Choices { token: true, ..choices(&recommended) }, &Fake::default()).unwrap();
        assert!(p.steps.iter().all(|s| s.skipped.is_none()) && p.installable().len() == 3);
        // Signed in, the gated file comes first: it starts while the sign-in is fresh.
        let first = p.steps.iter().find(|s| matches!(s.step, Step::FetchFile(_))).unwrap();
        assert_eq!(first.step.key(), "file:diffusion_models/flux3_image_klein_base_int8_convrot.safetensors");
        assert_eq!(p.models_bytes, m.download_bytes(m.preset("recommended")));
    }

    #[test]
    fn the_users_own_comfyui_gets_only_the_files_it_lacks() {
        let m = manifest();
        let server = BTreeMap::from([
            ("diffusion_models".to_string(), vec!["sub/qwen_image_2.1_int8_convrot.safetensors".to_string(), "other.safetensors".to_string()]),
            ("vae".to_string(), vec!["qwen_image_2.1_vae_bf16.safetensors".to_string()]),
        ]);
        let c =
            Choices { route: Route::Existing, engine_dir: String::new(), variant: String::new(), server_files: Some(server), ..choices(&["qwen-image-2-1"]) };
        let p = plan(m, &c, &Fake::default()).unwrap();
        // No runtime, no model paths: nothing of theirs is touched but the models folder.
        assert_eq!(
            keys(&p),
            [
                "file:diffusion_models/qwen_image_2.1_int8_convrot.safetensors",
                "file:text_encoders/qwen3vl_8b_fp8_scaled.safetensors",
                "file:vae/qwen_image_2.1_vae_bf16.safetensors",
                "configure",
                "verify",
            ]
        );
        assert_eq!(p.steps.iter().map(|s| s.done).collect::<Vec<_>>(), [true, false, true, false, false]);
        assert_eq!((p.download_bytes, p.engine_bytes), (m.files["text_encoders/qwen3vl_8b_fp8_scaled.safetensors"].size, 0));
    }

    #[test]
    fn choices_that_cannot_be_planned_are_errors() {
        let m = manifest();
        let fails = |c: Choices| plan(m, &c, &Fake::default()).unwrap_err().to_string();
        assert!(fails(choices(&["no-such-model"])).contains("no-such-model"));
        assert!(fails(Choices { variant: "voodoo".into(), ..choices(&[]) }).contains("voodoo"));
        assert!(fails(Choices { models_dir: String::new(), ..choices(&[]) }).contains("models"));
        assert!(fails(Choices { engine_dir: "  ".into(), ..choices(&[]) }).contains("engine"));
        for odd in ["relative/models", "..", "models\0x", &"x".repeat(2000)] {
            assert!(plan(m, &Choices { models_dir: odd.into(), ..choices(&[]) }, &Fake::default()).is_err(), "{odd}");
        }
        let climbing = if cfg!(windows) { "D:\\a\\..\\..\\Windows" } else { "/a/../../etc" };
        assert!(fails(Choices { engine_dir: climbing.into(), ..choices(&[]) }).contains("full path"));
        // Nothing chosen is still a plan: the engine alone.
        assert_eq!(keys(&plan(m, &choices(&[]), &Fake::default()).unwrap()), ["runtime", "unpack", "paths", "configure", "verify"]);
    }

    #[test]
    fn the_layout_keeps_one_engine_per_version_and_the_models_outside_it() {
        let m = manifest();
        let layout = Layout::new(m, &choices(&[])).unwrap();
        assert_eq!(layout.engine_root, PathBuf::from(root("engine")).join("0.42.1"));
        assert_eq!(layout.archive, PathBuf::from(root("engine")).join("ComfyUI_windows_portable_nvidia.7z"));
        assert_eq!(layout.python, layout.engine_root.join("python_embeded").join("python.exe"));
        assert_eq!(layout.script, layout.engine_root.join("ComfyUI").join("main.py"));
        assert_eq!(layout.model_file("vae/ae.safetensors").unwrap(), PathBuf::from(root("models")).join("vae").join("ae.safetensors"));
        assert!(layout.model_file("vae/../ae.safetensors").is_err());
        assert!(!layout.models_dir.starts_with(&layout.engine_dir));
        assert_eq!(part_of(Path::new("a/b.7z")), PathBuf::from("a/b.7z.part"));
    }

    #[test]
    fn a_folder_too_deep_for_windows_is_caught_before_anything_is_downloaded() {
        let m = manifest();
        let mut asset = m.asset("nvidia").unwrap().clone();
        let short = Layout::new(m, &choices(&[])).unwrap();
        let deep = Layout::new(m, &Choices { engine_dir: format!("{}{}", root("engine"), "\\deep".repeat(20)), ..choices(&[]) }).unwrap();
        assert_eq!(path_too_long(&asset, &short), None);
        assert_eq!(path_too_long(&asset, &deep).is_some(), cfg!(windows));
        // The default folder leaves room for a long user name.
        if cfg!(windows) {
            let home = Choices { engine_dir: format!(r"C:\Users\{}\AppData\Local\LayerShop\engine", "n".repeat(40)), ..choices(&[]) };
            assert_eq!(path_too_long(&asset, &Layout::new(m, &home).unwrap()), None);
        }
        asset.longest_path = 0;
        assert_eq!(path_too_long(&asset, &deep), None);
    }
}
