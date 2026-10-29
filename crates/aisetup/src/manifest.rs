//! What the wizard can install: the pinned ComfyUI build and the model files.
//!
//! A model is a set of files, and files are shared (Ideogram 4 uses the text encoder Qwen Image
//! 2.1 uses). So the manifest lists every file once and each model as references to them: totals
//! count a shared file once and nothing is downloaded twice.
//!
//! The manifest travels with the binary (`manifest.json`, checked by `cargo xtask ai-manifest`).
//! It is still read as hostile input: a later version may come from the network.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use photocraft_aigen::Family;
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// The manifest format this build reads.
pub const VERSION: u32 = 1;
/// The folders of a ComfyUI `models` directory a file may go to.
pub const FOLDERS: [&str; 6] = ["diffusion_models", "text_encoders", "vae", "checkpoints", "loras", "controlnet"];
/// Model files come from Hugging Face; the runtime from ComfyUI's GitHub releases.
const FILE_HOST: &str = "huggingface.co";
const ASSET_HOST: &str = "github.com";
/// No single file is larger than this (the largest today is 19.5 GB).
const MAX_FILE_BYTES: u64 = 96 << 30;
const MAX_FILES: usize = 2000;
const MAX_MODELS: usize = 500;
/// A card with less video memory than this is offered the `light` preset first.
pub const LIGHT_BELOW_VRAM_GB: u64 = 16;

/// One downloadable file: where it is, how large, and its checksum.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub url: String,
    pub size: u64,
    /// Lowercase hex.
    pub sha256: String,
    /// The host answers 401 until the user has accepted the model's licence and signed in.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub gated: bool,
}

/// The licence a model is published under.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Licence {
    pub name: String,
    pub url: String,
    /// Whether the licence allows commercial work, as its text says. `None`: not established
    /// (the text could not be read when the manifest was built), so the user is sent to read it.
    pub commercial: Option<bool>,
}

/// One model the wizard offers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelEntry {
    /// The id of the same model in [`photocraft_aigen::Model::defaults`].
    pub id: String,
    pub name: String,
    /// One line for the checklist.
    #[serde(default)]
    pub about: String,
    pub family: Family,
    /// Keys of [`Manifest::files`], the main file first.
    pub files: Vec<String>,
    /// What it is for: `create`, `edit`, `fill`.
    #[serde(default)]
    pub does: Vec<String>,
    #[serde(default)]
    pub licence: Licence,
    /// The model's page: where a gated model's licence is accepted.
    #[serde(default)]
    pub page: String,
    #[serde(default)]
    pub gated: bool,
    /// The least video memory it is known to need. `None`: not measured.
    #[serde(default)]
    pub min_vram_gb: Option<u32>,
    /// Card sizes it has been run on.
    #[serde(default)]
    pub tested_vram_gb: Vec<u32>,
}

/// One build of the runtime.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Asset {
    pub url: String,
    pub size: u64,
    pub sha256: String,
    /// What the build takes on disk once unpacked.
    #[serde(default)]
    pub unpacked_bytes: u64,
    /// The longest path inside the build (without its top folder), for the "path too long"
    /// check before anything is downloaded. 0: not measured.
    #[serde(default)]
    pub longest_path: u32,
}

/// The pinned ComfyUI: a release works with exactly the version its workflows were tested on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comfy {
    pub version: String,
    /// Inside the unpacked build: its Python, and the script it runs.
    pub python: String,
    pub script: String,
    /// By graphics card: `nvidia`, `nvidia_cu126`, `amd`, `intel`.
    pub assets: BTreeMap<String, Asset>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub version: u32,
    /// The day the sizes and checksums were last compared with the hosts.
    #[serde(default)]
    pub checked: String,
    pub comfy: Comfy,
    /// By `folder/name`, as the file sits in a ComfyUI `models` directory.
    pub files: BTreeMap<String, FileEntry>,
    pub models: Vec<ModelEntry>,
    /// Named selections of model ids: `light`, `starter`, `recommended`, `everything`.
    #[serde(default)]
    pub presets: BTreeMap<String, Vec<String>>,
}

fn bad(msg: impl Into<String>) -> Error {
    Error::Config(format!("the setup manifest is not usable: {}", msg.into()))
}

fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `url` is `https://<host>/…` with nothing that could change where it goes.
fn on_host(url: &str, host: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else { return false };
    let authority = rest.split('/').next().unwrap_or("");
    authority == host && rest.len() > authority.len() + 1 && !url.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// A file's key is `folder/name`: one of the model folders and a plain `.safetensors` file name.
/// Only weights are ever installed: no pickles, no scripts, no custom nodes.
pub fn split_key(key: &str) -> Option<(&str, &str)> {
    let (folder, name) = key.split_once('/')?;
    let plain = !name.is_empty()
        && name.len() <= 200
        && !name.starts_with('.')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        && !name.contains("..");
    (FOLDERS.contains(&folder) && plain && name.to_ascii_lowercase().ends_with(".safetensors")).then_some((folder, name))
}

impl Manifest {
    /// Read and check a manifest. Everything a later step relies on is checked here, once: every
    /// reference resolves, every URL is https on the expected host, every name is a plain file
    /// name in a known folder, every size and checksum is plausible.
    pub fn parse(text: &str) -> Result<Manifest> {
        if text.len() > 4 << 20 {
            return Err(bad("it is larger than any manifest should be"));
        }
        let m: Manifest = serde_json::from_str(text).map_err(|e| bad(e.to_string()))?;
        if m.version != VERSION {
            return Err(bad(format!("it is version {}, and this build reads version {VERSION}", m.version)));
        }
        if m.files.len() > MAX_FILES || m.models.len() > MAX_MODELS {
            return Err(bad("it lists more files or models than is plausible"));
        }
        let version_ok = !m.comfy.version.is_empty() && m.comfy.version.len() <= 32 && m.comfy.version.chars().all(|c| c.is_ascii_alphanumeric() || c == '.');
        if !version_ok {
            return Err(bad("the ComfyUI version is not a version number"));
        }
        for rel in [&m.comfy.python, &m.comfy.script] {
            let plain = !rel.is_empty() && rel.split('/').all(|part| !part.is_empty() && part != "." && part != ".." && !part.contains(['\\', ':']));
            if !plain {
                return Err(bad(format!("`{rel}` is not a path inside the ComfyUI build")));
            }
        }
        if m.comfy.assets.is_empty() {
            return Err(bad("it has no ComfyUI build"));
        }
        for (name, asset) in &m.comfy.assets {
            if !on_host(&asset.url, ASSET_HOST) || !asset.url.ends_with(".7z") {
                return Err(bad(format!("the `{name}` build is not a .7z on https://{ASSET_HOST}")));
            }
            if asset.size == 0 || asset.size > MAX_FILE_BYTES || !is_sha256(&asset.sha256) {
                return Err(bad(format!("the `{name}` build has no usable size or sha256")));
            }
        }
        for (key, file) in &m.files {
            if split_key(key).is_none() {
                return Err(bad(format!("`{key}` is not a .safetensors file in a model folder")));
            }
            if !on_host(&file.url, FILE_HOST) {
                return Err(bad(format!("`{key}` is not on https://{FILE_HOST}")));
            }
            if file.size == 0 || file.size > MAX_FILE_BYTES || !is_sha256(&file.sha256) {
                return Err(bad(format!("`{key}` has no usable size or sha256")));
            }
        }
        let mut ids = BTreeSet::new();
        for model in &m.models {
            let id_ok = !model.id.is_empty() && model.id.len() <= 80 && model.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
            if !id_ok || !ids.insert(model.id.as_str()) {
                return Err(bad(format!("the model id `{}` is not a unique plain id", model.id)));
            }
            if model.files.is_empty() || model.files.len() > 16 {
                return Err(bad(format!("`{}` lists no files, or too many", model.id)));
            }
            if let Some(missing) = model.files.iter().find(|f| !m.files.contains_key(*f)) {
                return Err(bad(format!("`{}` names `{missing}`, which the manifest does not list", model.id)));
            }
            for link in [&model.licence.url, &model.page] {
                if !link.is_empty() && !on_host(link, FILE_HOST) {
                    return Err(bad(format!("a link of `{}` does not go to https://{FILE_HOST}", model.id)));
                }
            }
        }
        for (name, list) in &m.presets {
            if let Some(unknown) = list.iter().find(|id| !ids.contains(id.as_str())) {
                return Err(bad(format!("the preset `{name}` names `{unknown}`, which is not a model")));
            }
        }
        Ok(m)
    }

    /// The manifest this build was made with.
    pub fn bundled() -> Result<&'static Manifest> {
        static BUNDLED: OnceLock<Result<Manifest>> = OnceLock::new();
        BUNDLED.get_or_init(|| Manifest::parse(include_str!("../manifest.json"))).as_ref().map_err(Clone::clone)
    }

    pub fn model(&self, id: &str) -> Option<&ModelEntry> {
        self.models.iter().find(|m| m.id == id)
    }

    /// The model ids of a preset (empty for a name the manifest doesn't have).
    pub fn preset(&self, name: &str) -> &[String] {
        self.presets.get(name).map_or(&[], Vec::as_slice)
    }

    /// The preset the wizard starts on for a card with `vram_gb` of memory: `light` on a card
    /// with less than [`LIGHT_BELOW_VRAM_GB`], `recommended` otherwise and when it is not known.
    pub fn suggested_preset(&self, vram_gb: Option<u64>) -> &'static str {
        let small = vram_gb.is_some_and(|gb| gb < LIGHT_BELOW_VRAM_GB);
        if small && !self.preset("light").is_empty() { "light" } else { "recommended" }
    }

    /// The models in the order the wizard lists them for a card with `vram_gb` of memory: the
    /// suggested preset's first, the rest in the manifest's order.
    pub fn offered(&self, vram_gb: Option<u64>) -> Vec<&ModelEntry> {
        let first = self.preset(self.suggested_preset(vram_gb));
        let (mut out, rest): (Vec<&ModelEntry>, Vec<&ModelEntry>) = self.models.iter().partition(|m| first.contains(&m.id));
        out.extend(rest);
        out
    }

    /// The models named by `ids`, in the manifest's order; an unknown id is an error.
    pub fn select(&self, ids: &[String]) -> Result<Vec<&ModelEntry>> {
        if let Some(unknown) = ids.iter().find(|id| self.model(id).is_none()) {
            return Err(Error::Config(format!("`{unknown}` is not a model the setup offers")));
        }
        Ok(self.models.iter().filter(|m| ids.contains(&m.id)).collect())
    }

    /// The file keys `models` need, each once, in the order they are first needed.
    pub fn file_keys<'a>(&self, models: impl IntoIterator<Item = &'a ModelEntry>) -> Vec<&'a str> {
        let mut seen = BTreeSet::new();
        models.into_iter().flat_map(|m| m.files.iter()).map(String::as_str).filter(|key| seen.insert(*key)).collect()
    }

    /// What downloading `ids` takes, with shared files counted once. Unknown ids count nothing.
    pub fn download_bytes(&self, ids: &[String]) -> u64 {
        let models = self.models.iter().filter(|m| ids.contains(&m.id));
        self.file_keys(models).into_iter().filter_map(|key| self.files.get(key)).fold(0u64, |sum, f| sum.saturating_add(f.size))
    }

    /// What adding `id` to `have` downloads: its files that none of `have` brings.
    pub fn extra_bytes(&self, id: &str, have: &[String]) -> u64 {
        let mut with = have.to_vec();
        with.push(id.to_string());
        self.download_bytes(&with).saturating_sub(self.download_bytes(have))
    }

    /// The files of `models` by folder, as a ComfyUI server would list them: what
    /// [`photocraft_aigen::detect::detect`] reads.
    pub fn folders<'a>(&self, models: impl IntoIterator<Item = &'a ModelEntry>) -> BTreeMap<String, Vec<String>> {
        let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for key in self.file_keys(models) {
            if let Some((folder, name)) = split_key(key) {
                out.entry(folder.to_string()).or_default().push(name.to_string());
            }
        }
        out
    }

    /// The build for a graphics card (`nvidia`, …), by its key.
    pub fn asset(&self, variant: &str) -> Result<&Asset> {
        self.comfy.assets.get(variant).ok_or_else(|| Error::Config(format!("there is no ComfyUI build for `{variant}`")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_aigen::{Model, detect};

    fn bundled() -> &'static Manifest {
        Manifest::bundled().unwrap()
    }

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_bundled_manifest_is_valid_and_pins_one_comfyui() {
        let m = bundled();
        assert_eq!(m.comfy.version, "0.42.1");
        assert_eq!(m.comfy.assets.keys().map(String::as_str).collect::<Vec<_>>(), ["amd", "intel", "nvidia", "nvidia_cu126"]);
        assert!(m.comfy.assets.values().all(|a| a.url.contains("/v0.42.1/")));
        assert!(m.asset("nvidia").is_ok() && m.asset("voodoo").is_err());
        // Measured from each archive's header (`--example archive_contents`).
        assert!(m.comfy.assets.values().all(|a| a.unpacked_bytes > a.size && (100..=200).contains(&a.longest_path)));
        // Every file is used by a model: nothing is listed that nobody would download.
        let used: BTreeSet<&str> = m.models.iter().flat_map(|x| x.files.iter().map(String::as_str)).collect();
        assert_eq!(used.len(), m.files.len());
    }

    /// The table of the plan (D4), from the real sizes: shared files are counted once.
    #[test]
    fn presets_total_what_the_website_says() {
        let m = bundled();
        let gb = |ids: &[String]| (m.download_bytes(ids) as f64 / 1e9 * 10.0).round() / 10.0;
        assert_eq!(gb(m.preset("light")), 13.0);
        assert_eq!(gb(m.preset("starter")), 18.5);
        assert_eq!(gb(m.preset("recommended")), 44.1);
        // FLUX.3 Klein (2026-10-16) is 7.4 GB less than the FLUX.2 Klein 9B it replaced; MOSS
        // Image (2026-10-20) shares no file with the others.
        assert_eq!(gb(m.preset("everything")), 182.1);
        assert_eq!(m.preset("everything").len(), m.models.len());
        assert!(m.preset("nope").is_empty());
        for (id, alone) in [
            ("qwen-image-2-1", 18.5),
            ("z-image-turbo", 14.6),
            ("flux-3-klein", 11.0),
            ("moss-image", 13.0),
            ("krea-2-turbo", 19.0),
            ("ming-image-0-1-design", 25.9),
            ("ernie-image-turbo", 24.1),
            ("hidream-o1", 46.1),
        ] {
            assert_eq!(gb(&ids(&[id])), alone, "{id}");
        }
        // Ideogram 4 shares Qwen Image 2.1's text encoder and FLUX.2's VAE: beside them it adds
        // only its two diffusion models. HiDream O1's refiner shares Z-Image Turbo's files.
        let extra = |id: &str, have: &[&str]| (m.extra_bytes(id, &ids(have)) as f64 / 1e9 * 10.0).round() / 10.0;
        assert_eq!(extra("ideogram-4", &["qwen-image-2-1", "flux-3-klein"]), 18.6);
        assert_eq!(extra("hidream-o1", &["z-image-turbo"]), 37.7);
        assert_eq!(extra("ernie-image-turbo", &["flux-3-klein"]), 23.8);
        assert_eq!(extra("qwen-image-2-1", &["qwen-image-2-1"]), 0.0);
        assert_eq!(m.download_bytes(&ids(&["no-such-model"])), 0);
    }

    /// What keeps the wizard and the recommended list from drifting apart: every model the
    /// wizard installs is one of the app's default models, found again by Scan Server from the
    /// wizard's own file names, with the settings that default has.
    #[test]
    fn every_model_is_a_default_model_and_is_detected_from_its_files() {
        let m = bundled();
        let defaults = Model::defaults();
        for entry in &m.models {
            let default = defaults.iter().find(|d| d.id == entry.id).unwrap_or_else(|| panic!("{} is not a default model", entry.id));
            assert_eq!((default.family, default.name.as_str()), (entry.family, entry.name.as_str()), "{}", entry.id);
            // The main file comes first, and is the default's.
            let (_, main) = split_key(&entry.files[0]).unwrap();
            assert_eq!(default.main_file(), main, "{}", entry.id);
            // Every file the default names is one the wizard downloads for it.
            let names: Vec<&str> = entry.files.iter().map(|k| split_key(k).unwrap().1).collect();
            let wanted = [
                &default.unet,
                &default.unet2,
                &default.checkpoint,
                &default.clip,
                &default.clip2,
                &default.clip3,
                &default.clip4,
                &default.vae,
                &default.controlnet,
                &default.enhancer,
                &default.refiner,
                &default.refiner_clip,
                &default.refiner_vae,
            ];
            for file in wanted.into_iter().filter(|f| !f.is_empty()) {
                assert!(names.contains(&file.as_str()), "{}: {file} is not downloaded", entry.id);
            }
            assert_eq!(names.len(), wanted.iter().filter(|f| !f.is_empty()).collect::<BTreeSet<_>>().len(), "{}: a file nobody uses", entry.id);
            // A scan of a server that has just these files finds the model, set up as the default.
            let found = detect::detect(&m.folders([entry]));
            let same = found.iter().find(|f| detect::same_variant(default, f)).unwrap_or_else(|| panic!("{} is not detected from its files", entry.id));
            let sampling = |x: &Model| (x.steps, x.cfg, x.sampler.clone(), x.scheduler.clone(), x.shift, x.guidance, x.megapixels, x.alpha);
            assert_eq!(sampling(same), sampling(default), "{}", entry.id);
            let files =
                |x: &Model| [x.unet.clone(), x.unet2.clone(), x.checkpoint.clone(), x.clip.clone(), x.vae.clone(), x.enhancer.clone(), x.refiner.clone()];
            assert_eq!(files(same), files(default), "{}", entry.id);
            assert!(detect::problem(same).is_none());
            // Gated means a gated file, and the page to accept the licence on.
            assert_eq!(entry.gated, entry.files.iter().any(|k| m.files[k].gated), "{}", entry.id);
            assert!(!entry.gated || !entry.page.is_empty());
            assert!(!entry.licence.name.is_empty() && !entry.licence.url.is_empty() && !entry.about.is_empty(), "{}", entry.id);
        }
    }

    #[test]
    fn keys_name_weights_in_model_folders_and_nothing_else() {
        assert_eq!(split_key("vae/ae.safetensors"), Some(("vae", "ae.safetensors")));
        assert_eq!(split_key("diffusion_models/qwen_image_2.1_int8_convrot.safetensors").map(|k| k.0), Some("diffusion_models"));
        for key in [
            "",
            "ae.safetensors",
            "vae/",
            "vae/ae.ckpt",
            "vae/ae.pt",
            "custom_nodes/evil.safetensors",
            "vae/../../evil.safetensors",
            "vae/sub/ae.safetensors",
            "vae/..safetensors",
            "vae/.safetensors",
            "vae/a b.safetensors",
            "vae/C:ae.safetensors",
            "vae\\ae.safetensors",
            "/vae/ae.safetensors",
        ] {
            assert_eq!(split_key(key), None, "{key}");
        }
    }

    #[test]
    fn a_hostile_manifest_is_refused_whole() {
        let good: serde_json::Value = serde_json::from_str(include_str!("../manifest.json")).unwrap();
        let broken = |edit: &dyn Fn(&mut serde_json::Value)| {
            let mut v = good.clone();
            edit(&mut v);
            Manifest::parse(&v.to_string()).unwrap_err().to_string()
        };
        assert!(Manifest::parse(&good.to_string()).is_ok());
        let key = "vae/ae.safetensors";
        for (what, edit) in [
            ("version", Box::new(|v: &mut serde_json::Value| v["version"] = 2.into()) as Box<dyn Fn(&mut serde_json::Value)>),
            ("another host", Box::new(|v| v["files"][key]["url"] = "https://evil.example/ae.safetensors".into())),
            ("http", Box::new(|v| v["files"][key]["url"] = "http://huggingface.co/x/ae.safetensors".into())),
            ("host prefix", Box::new(|v| v["files"][key]["url"] = "https://huggingface.co.evil.example/ae.safetensors".into())),
            ("userinfo", Box::new(|v| v["files"][key]["url"] = "https://huggingface.co@evil.example/ae.safetensors".into())),
            ("sha", Box::new(|v| v["files"][key]["sha256"] = "ABC".into())),
            ("size", Box::new(|v| v["files"][key]["size"] = 0.into())),
            ("huge", Box::new(|v| v["files"][key]["size"] = (u64::MAX).into())),
            ("traversal", Box::new(|v| v["files"]["vae/../x.safetensors"] = v["files"][key].clone())),
            ("pickle", Box::new(|v| v["files"]["vae/x.ckpt"] = v["files"][key].clone())),
            ("dangling file", Box::new(|v| v["models"][0]["files"][0] = "vae/nope.safetensors".into())),
            ("dangling preset", Box::new(|v| v["presets"]["starter"][0] = "nope".into())),
            ("duplicate id", Box::new(|v| v["models"][1]["id"] = v["models"][0]["id"].clone())),
            ("odd id", Box::new(|v| v["models"][0]["id"] = "../x".into())),
            ("asset host", Box::new(|v| v["comfy"]["assets"]["nvidia"]["url"] = "https://evil.example/a.7z".into())),
            ("asset kind", Box::new(|v| v["comfy"]["assets"]["nvidia"]["url"] = "https://github.com/a/b.exe".into())),
            ("python path", Box::new(|v| v["comfy"]["python"] = "../python.exe".into())),
            ("script path", Box::new(|v| v["comfy"]["script"] = "C:\\main.py".into())),
            ("version text", Box::new(|v| v["comfy"]["version"] = "../..".into())),
            ("licence link", Box::new(|v| v["models"][0]["licence"]["url"] = "https://evil.example/licence".into())),
        ] {
            assert!(broken(&*edit).contains("not usable"), "{what}");
        }
        for junk in ["", "{}", "[]", "null", "{\"version\":1}", "not json"] {
            assert!(Manifest::parse(junk).is_err(), "{junk}");
        }
        assert!(Manifest::parse(&" ".repeat((4 << 20) + 1)).is_err());
    }

    /// A card with little memory starts on Light, MOSS Image, listed first; a larger card, or one
    /// whose memory is not known, on Recommended, in the manifest's order.
    #[test]
    fn a_small_card_is_offered_the_light_preset_first() {
        let m = bundled();
        assert_eq!(m.preset("light"), ["moss-image"]);
        for (vram, preset) in
            [(Some(4), "light"), (Some(8), "light"), (Some(15), "light"), (Some(16), "recommended"), (Some(24), "recommended"), (None, "recommended")]
        {
            assert_eq!(m.suggested_preset(vram), preset, "{vram:?}");
        }
        let order = |vram| m.offered(vram).iter().map(|x| x.id.as_str()).collect::<Vec<_>>();
        assert_eq!(order(Some(8))[0], "moss-image");
        assert_eq!(order(Some(8)).len(), m.models.len());
        assert_eq!(order(Some(24)), m.models.iter().map(|x| x.id.as_str()).collect::<Vec<_>>());
        assert_eq!(order(None), order(Some(24)));
        // A manifest without a Light preset suggests Recommended on any card.
        let mut without = m.clone();
        without.presets.remove("light");
        assert_eq!(without.suggested_preset(Some(8)), "recommended");
        assert_eq!(without.offered(Some(8)).first().map(|x| x.id.as_str()), Some("qwen-image-2-1"));
    }

    #[test]
    fn selections_keep_the_manifests_order_and_refuse_unknown_models() {
        let m = bundled();
        let chosen = m.select(&ids(&["z-image-turbo", "qwen-image-2-1"])).unwrap();
        assert_eq!(chosen.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["qwen-image-2-1", "z-image-turbo"]);
        assert!(m.select(&ids(&["nope"])).unwrap_err().to_string().contains("nope"));
        assert!(m.select(&[]).unwrap().is_empty());
        let folders = m.folders(chosen);
        assert_eq!(folders["vae"], ["qwen_image_2.1_vae_bf16.safetensors", "ae.safetensors"]);
        assert_eq!(folders["diffusion_models"].len(), 2);
    }
}
