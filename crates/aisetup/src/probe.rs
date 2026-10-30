//! What this machine has: the wizard's first screen. Nothing here changes anything.
//!
//! [`collect`] (native) asks the system: Windows for its version, graphics cards and drives,
//! `nvidia-smi` for the card's real memory, the disk for an existing ComfyUI, and the download
//! hosts whether they answer. Reading those answers is pure and tested here; so is turning a
//! [`Probe`] into the rows the screen shows ([`Probe::checks`]).

use photocraft_aigen::Launch;
use serde::{Deserialize, Serialize};

use crate::human_bytes;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Vendor {
    Nvidia,
    Amd,
    Intel,
    #[default]
    Other,
}

impl Vendor {
    fn of(name: &str) -> Vendor {
        let n = name.to_ascii_lowercase();
        if n.contains("nvidia") || n.contains("geforce") || n.contains("quadro") || n.contains("rtx") {
            Vendor::Nvidia
        } else if n.contains("amd") || n.contains("radeon") {
            Vendor::Amd
        } else if n.contains("intel") {
            Vendor::Intel
        } else {
            Vendor::Other
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Gpu {
    pub name: String,
    pub vendor: Vendor,
    /// Video memory in megabytes, when it could be read.
    pub vram_mb: Option<u64>,
    pub driver: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Os {
    /// As the system names itself: `Windows 11 Pro`.
    pub name: String,
    pub version: String,
    pub sixty_four_bit: bool,
    /// Windows, where the app can install the engine itself.
    pub windows: bool,
    /// Installed memory, when it can be told.
    pub memory_mb: Option<u64>,
    /// Memory and the paging file together, as they stand now (Windows' commit limit).
    pub virtual_mb: Option<u64>,
    /// Whether Windows sizes the paging file itself (it then grows when it must, room allowing).
    pub paging_managed: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Drive {
    /// `D:`
    pub name: String,
    pub free: u64,
    pub total: u64,
}

/// A ComfyUI that is already on this computer.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Existing {
    /// The addresses a ComfyUI answers at now.
    pub running: Vec<String>,
    /// How the one ComfyUI Desktop installed is started.
    pub installed: Option<Launch>,
    /// The folders it reads model files from, the one it saves new ones to first: as the running
    /// one reports them, else as the installed one's settings give them.
    pub models: Vec<String>,
    /// Its version (`0.42.1`): as the running one on this computer says, else as the installed
    /// one's source folder says. Empty: not known.
    pub version: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Host {
    pub name: String,
    pub ok: bool,
    /// Why not, when it did not answer.
    pub note: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Probe {
    pub os: Os,
    pub gpus: Vec<Gpu>,
    pub drives: Vec<Drive>,
    pub comfy: Existing,
    pub hosts: Vec<Host>,
    /// Windows: whether the engine will find the Microsoft C++ runtime PyTorch needs.
    pub vc_runtime: VcRuntime,
}

/// Where the Microsoft C++ runtime (`msvcp140.dll` and its kin) comes from for the engine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VcRuntime {
    /// Not looked at (not Windows).
    #[default]
    Unknown,
    /// The app carries it, and puts it beside the engine's Python.
    Bundled,
    /// The Visual C++ Redistributable is installed.
    Installed,
    /// Neither: the engine would stop while starting.
    Missing,
}

/// How a row of the first screen reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Level {
    Ok,
    Info,
    Warn,
    /// The app can't set up local AI here.
    Stop,
}

/// One row of the first screen.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub id: String,
    pub level: Level,
    pub title: String,
    pub detail: String,
    /// The short reading at the row's right edge: `24 GB`, `supported`, `online`.
    pub value: String,
}

/// NVIDIA drivers older than this can't run the default build (CUDA 13); the `cu126` build runs
/// on them, and on the older cards the default build no longer supports.
const NVIDIA_DEFAULT_DRIVER: u32 = 580;
/// What the first screen says to someone who has a ComfyUI already, under where it is.
const HAS_COMFY: &str = "Choose below: use it with the models it has, or install a private copy beside it (yours is left as it is).";
/// Why a PC without a card the engine can use (a virtual machine, say) can't run local models.
const NO_CARD: &str = "Local models need a graphics card the engine can use (NVIDIA works best), and none was found. A virtual machine usually has none. \
Cloud models work on any PC. The engine can still be installed to run on the processor, but these models are far too slow there.";
/// The smallest drive worth offering: the engine alone is about 8 GB while it unpacks.
const WORTH_LISTING: u64 = 10_000_000_000;
/// Under this much installed memory (with a little for the system that reserves), larger models
/// may not load.
const LITTLE_MEMORY_MB: u64 = 15_000;
/// A paging file set by hand to leave less than this (memory included) is called small.
const SMALL_VIRTUAL_MB: u64 = 32_000;

/// `nvidia-smi --query-gpu=name,memory.total,driver_version --format=csv,noheader,nounits`
pub fn parse_nvidia_smi(text: &str) -> Vec<Gpu> {
    text.lines()
        .take(64)
        .filter_map(|line| {
            let mut f = line.split(',').map(str::trim);
            let name = f.next().filter(|n| !n.is_empty() && n.len() <= 200)?.to_string();
            let vram_mb = f.next()?.parse::<u64>().ok().filter(|mb| *mb > 0 && *mb < 4_000_000)?;
            Some(Gpu { vendor: Vendor::Nvidia, name, vram_mb: Some(vram_mb), driver: f.next().unwrap_or("").chars().take(40).collect() })
        })
        .collect()
}

/// One value or a list of them: PowerShell's `ConvertTo-Json` writes a list of one as the one.
fn list(v: &serde_json::Value) -> Vec<&serde_json::Value> {
    match v {
        serde_json::Value::Array(a) => a.iter().take(64).collect(),
        serde_json::Value::Null => Vec::new(),
        one => vec![one],
    }
}

fn text(v: &serde_json::Value, key: &str) -> String {
    v.get(key).and_then(serde_json::Value::as_str).unwrap_or("").trim().chars().take(200).collect()
}

fn number(v: &serde_json::Value, key: &str) -> Option<u64> {
    let n = v.get(key)?;
    n.as_u64().or_else(|| n.as_f64().filter(|f| f.is_finite() && *f >= 0.0).map(|f| f as u64)).or_else(|| n.as_str().and_then(|s| s.trim().parse().ok()))
}

/// What [`SYSTEM_SCRIPT`] printed: the system, its graphics cards and its drives.
pub fn parse_system(json: &str) -> (Os, Vec<Gpu>, Vec<Drive>) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json.trim().trim_start_matches('\u{feff}')) else { return Default::default() };
    let o = v.get("os").cloned().unwrap_or_default();
    let name = text(&o, "Caption").trim_start_matches("Microsoft ").to_string();
    // Win32_OperatingSystem counts memory in kilobytes.
    let mb = |key: &str| number(&o, key).map(|kb| kb / 1024).filter(|mb| *mb > 0);
    let paging_managed = v.get("cs").and_then(|cs| cs.get("AutomaticManagedPagefile")).and_then(serde_json::Value::as_bool);
    let os = Os {
        windows: !name.is_empty(),
        sixty_four_bit: text(&o, "OSArchitecture").contains("64"),
        version: text(&o, "Version"),
        name,
        memory_mb: mb("TotalVisibleMemorySize"),
        virtual_mb: mb("TotalVirtualMemorySize"),
        paging_managed,
    };
    // The registry knows the real memory of a card; `AdapterRAM` stops at 4 GB.
    let reg: Vec<(String, u64)> = list(v.get("reg").unwrap_or(&serde_json::Value::Null))
        .into_iter()
        .filter_map(|r| Some((text(r, "DriverDesc"), number(r, "HardwareInformation.qwMemorySize")?)))
        .collect();
    let gpus = list(v.get("gpu").unwrap_or(&serde_json::Value::Null))
        .into_iter()
        .filter_map(|g| {
            let name = text(g, "Name");
            if name.is_empty() {
                return None;
            }
            let bytes = reg.iter().find(|(desc, _)| *desc == name).map(|(_, b)| *b).or_else(|| number(g, "AdapterRAM")).filter(|b| *b > 0);
            Some(Gpu { vendor: Vendor::of(&name), vram_mb: bytes.map(|b| b >> 20), driver: text(g, "DriverVersion"), name })
        })
        .collect();
    let drives = list(v.get("disk").unwrap_or(&serde_json::Value::Null))
        .into_iter()
        .filter_map(|d| Some(Drive { name: text(d, "DeviceID"), free: number(d, "FreeSpace")?, total: number(d, "Size")? }))
        .filter(|d| !d.name.is_empty())
        .collect();
    (os, gpus, drives)
}

/// Asks Windows for everything [`parse_system`] reads, as one line of JSON.
pub const SYSTEM_SCRIPT: &str = "$ErrorActionPreference='SilentlyContinue'; \
$os = Get-CimInstance Win32_OperatingSystem | Select-Object Caption,Version,OSArchitecture,TotalVisibleMemorySize,TotalVirtualMemorySize; \
$cs = Get-CimInstance Win32_ComputerSystem | Select-Object AutomaticManagedPagefile; \
$gpu = @(Get-CimInstance Win32_VideoController | Select-Object Name,AdapterRAM,DriverVersion); \
$reg = @(Get-ItemProperty 'HKLM:\\SYSTEM\\CurrentControlSet\\Control\\Class\\{4d36e968-e325-11ce-bfc1-08002be10318}\\0*' | Select-Object DriverDesc,'HardwareInformation.qwMemorySize'); \
$disk = @(Get-CimInstance Win32_LogicalDisk -Filter 'DriveType=3' | Select-Object DeviceID,FreeSpace,Size); \
@{os=$os;cs=$cs;gpu=$gpu;reg=$reg;disk=$disk} | ConvertTo-Json -Depth 4 -Compress";

/// The cards of both readings as one list: `nvidia-smi`'s where it has them (it knows the real
/// memory and the driver's own version number), the system's for the rest.
pub fn merge_gpus(system: Vec<Gpu>, smi: Vec<Gpu>) -> Vec<Gpu> {
    let mut out = smi;
    for g in system {
        if !out.iter().any(|have| have.name.eq_ignore_ascii_case(&g.name)) {
            out.push(g);
        }
    }
    out
}

impl Probe {
    /// The card the engine would use: the one with the most memory among the best-supported
    /// vendor present (NVIDIA, then AMD, then Intel).
    pub fn gpu(&self) -> Option<&Gpu> {
        let rank = |g: &Gpu| match g.vendor {
            Vendor::Nvidia => 3,
            Vendor::Amd => 2,
            Vendor::Intel => 1,
            Vendor::Other => 0,
        };
        self.gpus.iter().max_by_key(|g| (rank(g), g.vram_mb.unwrap_or(0)))
    }

    /// The card the engine would use, when it is one ComfyUI has a build for. A virtual machine's
    /// display adapter, or Windows' basic one, is not: the engine's PyTorch can't run on it.
    pub fn usable_gpu(&self) -> Option<&Gpu> {
        self.gpu().filter(|g| g.vendor != Vendor::Other)
    }

    /// The build of ComfyUI for this machine's card (a key of the manifest's assets), and
    /// whether that pairing has been tried: only NVIDIA has.
    pub fn variant(&self) -> (&'static str, bool) {
        let Some(gpu) = self.gpu() else { return ("nvidia", false) };
        match gpu.vendor {
            Vendor::Nvidia => {
                let major = gpu.driver.split('.').next().and_then(|m| m.trim().parse::<u32>().ok());
                let name = gpu.name.to_ascii_uppercase();
                // GTX 10 series and older (and their Quadro siblings) left the default build.
                let old_card =
                    ["GTX 10", "GTX 9", "GTX 7", "GTX 6", "GTX TITAN", "QUADRO P", "QUADRO M", "QUADRO K", "TITAN X"].iter().any(|k| name.contains(k));
                if old_card || major.is_some_and(|m| m < NVIDIA_DEFAULT_DRIVER) { ("nvidia_cu126", true) } else { ("nvidia", true) }
            }
            Vendor::Amd => ("amd", false),
            Vendor::Intel => ("intel", false),
            Vendor::Other => ("nvidia", false),
        }
    }

    /// The drive with the most room.
    pub fn roomiest(&self) -> Option<&Drive> {
        self.drives.iter().max_by_key(|d| d.free)
    }

    /// Can the app install the engine itself here?
    pub fn can_install(&self) -> bool {
        self.os.windows && self.os.sixty_four_bit
    }

    /// The rows of the first screen.
    pub fn checks(&self) -> Vec<Check> {
        let row = |id: &str, level, title: String, detail: &str, value: String| Check { id: id.into(), level, title, detail: detail.into(), value };
        let mut out = Vec::new();
        // The system.
        out.push(if self.can_install() {
            row("os", Level::Ok, format!("{}, 64-bit", self.os.name), "", "supported".into())
        } else if self.os.windows {
            row("os", Level::Stop, self.os.name.clone(), "The engine needs 64-bit Windows.", "not supported".into())
        } else {
            let name = if self.os.name.is_empty() { "This system".to_string() } else { self.os.name.clone() };
            row("os", Level::Stop, name, "Installing the engine for you is Windows only for now. Use my ComfyUI works here.", "use your own".into())
        });
        // The graphics card.
        out.push(match self.gpu() {
            Some(gpu) => {
                let (variant, tried) = self.variant();
                let memory = gpu.vram_mb.map_or_else(String::new, |mb| format!("{} GB", (mb as f64 / 1024.0).round() as u64));
                match (gpu.vendor, variant, tried) {
                    (Vendor::Nvidia, "nvidia", _) => row("gpu", Level::Ok, gpu.name.clone(), "Graphics driver is recent enough", memory),
                    (Vendor::Nvidia, _, _) => row(
                        "gpu",
                        Level::Info,
                        gpu.name.clone(),
                        "An older card or driver: the engine build for CUDA 12.6 will be installed. Updating the graphics driver is still a good idea.",
                        memory,
                    ),
                    (Vendor::Amd | Vendor::Intel, _, _) => row(
                        "gpu",
                        Level::Warn,
                        gpu.name.clone(),
                        "Not tested: ComfyUI has a build for this card, but these models have only been run on NVIDIA cards. Cloud models work on any PC.",
                        memory,
                    ),
                    _ => row("gpu", Level::Stop, gpu.name.clone(), NO_CARD, "can't run local models".into()),
                }
            }
            None => row("gpu", Level::Stop, "No graphics card found".into(), NO_CARD, "can't run local models".into()),
        });
        // Memory: models load through it on their way to the card, and Windows' paging file makes
        // up what it lacks only while the paging file may grow.
        if let Some(mb) = self.os.memory_mb {
            let value = format!("{} GB", (mb as f64 / 1024.0).round() as u64);
            let fixed_small = self.os.paging_managed == Some(false) && self.os.virtual_mb.is_some_and(|v| v < SMALL_VIRTUAL_MB);
            out.push(if mb < LITTLE_MEMORY_MB {
                row(
                    "memory",
                    Level::Warn,
                    "Little memory".into(),
                    "Local models load through this PC's memory. With under 16 GB, larger models may not load: Windows' paging file makes up the rest only while drive C: has free space. Cloud models work on any PC.",
                    value,
                )
            } else if fixed_small {
                row(
                    "memory",
                    Level::Warn,
                    "Small paging file".into(),
                    "The paging file has a fixed, small size, so larger models may not load. Let Windows manage its size (Advanced system settings › Performance › Virtual memory).",
                    value,
                )
            } else {
                row("memory", Level::Ok, "Memory".into(), "", value)
            });
        }
        // Room.
        out.push(match self.roomiest() {
            Some(d) if d.free >= WORTH_LISTING => {
                row("disk", Level::Ok, "Free disk space".into(), "Models can go on any drive", format!("{} {}", d.name, human_bytes(d.free)))
            }
            Some(d) => row(
                "disk",
                Level::Warn,
                "Little free disk space".into(),
                "The engine takes about 8 GB while it installs, and the smallest model set 18.5 GB.",
                format!("{} {}", d.name, human_bytes(d.free)),
            ),
            None => row("disk", Level::Info, "Free disk space".into(), "Checked when you choose the folders", String::new()),
        });
        // A ComfyUI that is already here.
        let models = match self.comfy.models.first() {
            Some(folder) => format!(" Its models are in {folder}."),
            None => String::new(),
        };
        let found = if self.comfy.version.is_empty() { "ComfyUI detected".to_string() } else { format!("ComfyUI {} detected", self.comfy.version) };
        out.push(match (self.comfy.running.first(), &self.comfy.installed) {
            (Some(url), _) => row("comfy", Level::Info, found, &format!("Running at {url}.{models} {HAS_COMFY}"), "running".into()),
            (None, Some(_)) => row("comfy", Level::Info, found, &format!("ComfyUI Desktop, not running now.{models} {HAS_COMFY}"), "installed".into()),
            (None, None) => {
                row("comfy", Level::Info, "ComfyUI not found".into(), "A private copy will be installed, with its own Python", "will install".into())
            }
        });
        // The Microsoft C++ runtime PyTorch needs: said only when the engine would go without it.
        if self.vc_runtime == VcRuntime::Missing && self.can_install() {
            out.push(row(
                "vcRuntime",
                Level::Warn,
                "Microsoft C++ runtime not found".into(),
                "The engine needs the Microsoft Visual C++ Redistributable (x64), and this build of PhotoCraft doesn't include it. Install it from https://aka.ms/vs/17/release/vc_redist.x64.exe, then check again.",
                "missing".into(),
            ));
        }
        // The download hosts.
        let down: Vec<&Host> = self.hosts.iter().filter(|h| !h.ok).collect();
        out.push(match (self.hosts.is_empty(), down.first()) {
            (true, _) => row("net", Level::Info, "Download servers".into(), "Not checked", String::new()),
            (false, None) => row("net", Level::Ok, "Download servers reachable".into(), "", "online".into()),
            (false, Some(first)) => {
                let names = down.iter().map(|h| h.name.as_str()).collect::<Vec<_>>().join(", ");
                let detail = format!("{names} did not answer ({}). Check the connection, a proxy or a firewall, then check again.", first.note);
                row("net", Level::Warn, "Download servers not reachable".into(), &detail, "offline".into())
            }
        });
        out
    }
}

/// The steps of [`collect`], in the order it takes them: what the wizard shows while it looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    /// Windows, the graphics card, memory and the drives.
    Hardware,
    /// A ComfyUI that is running, or installed.
    Comfy,
    /// The hosts the downloads come from.
    Servers,
}

impl Stage {
    pub const ALL: [Stage; 3] = [Stage::Hardware, Stage::Comfy, Stage::Servers];

    /// What is being done, as a progress message (the wizard knows the step by it).
    pub fn message(self) -> &'static str {
        match self {
            Stage::Hardware => "Checking your hardware",
            Stage::Comfy => "Looking for ComfyUI",
            Stage::Servers => "Checking the download servers",
        }
    }

    /// Where it has got to, for a progress bar.
    pub fn fraction(self) -> f32 {
        match self {
            Stage::Hardware => 0.1,
            Stage::Comfy => 0.4,
            Stage::Servers => 0.7,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::{Drive, Gpu, Host, Os, Probe, SYSTEM_SCRIPT, Stage, VcRuntime, merge_gpus, parse_nvidia_smi, parse_system};
    use crate::fetch::Fetch;
    use crate::manifest::Manifest;
    use photocraft_aigen::Transport;
    use std::process::{Command, Stdio};

    /// Run a system tool and read what it prints. Nothing is shown, and a tool that is missing or
    /// hangs is simply no answer.
    fn output(program: &str, args: &[&str]) -> Option<String> {
        let mut cmd = Command::new(program);
        cmd.args(args).stdin(Stdio::null()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW: no console window flashes up behind the app.
            cmd.creation_flags(0x0800_0000);
        }
        let out = cmd.output().ok()?;
        // Bounded: these tools print a few lines.
        let text = String::from_utf8_lossy(out.stdout.get(..out.stdout.len().min(1 << 20)).unwrap_or(&[])).into_owned();
        Some(text).filter(|t| !t.trim().is_empty())
    }

    fn system() -> (Os, Vec<Gpu>, Vec<Drive>) {
        if cfg!(windows) {
            return output("powershell", &["-NoProfile", "-NonInteractive", "-Command", SYSTEM_SCRIPT]).map(|t| parse_system(&t)).unwrap_or_default();
        }
        let name = match std::env::consts::OS {
            "macos" => "macOS",
            "linux" => "Linux",
            other => other,
        };
        (
            Os { name: name.to_string(), version: String::new(), sixty_four_bit: cfg!(target_pointer_width = "64"), windows: false, ..Os::default() },
            Vec::new(),
            Vec::new(),
        )
    }

    /// Look at this machine. `urls` are the addresses a ComfyUI is looked for at (the one in the
    /// settings, and the usual ports). Takes a few seconds: run it off the UI thread. `stage` is
    /// told each step as it begins.
    pub fn collect(
        manifest: &Manifest,
        transport: &dyn Transport,
        fetch: &dyn Fetch,
        urls: &[String],
        cancelled: &dyn Fn() -> bool,
        stage: &dyn Fn(Stage),
    ) -> Probe {
        stage(Stage::Hardware);
        let (os, system_gpus, drives) = system();
        let smi = output("nvidia-smi", &["--query-gpu=name,memory.total,driver_version", "--format=csv,noheader,nounits"])
            .map(|t| parse_nvidia_smi(&t))
            .unwrap_or_default();
        let vc_runtime = if os.windows { crate::vcrt::Sources::this_pc().found() } else { VcRuntime::Unknown };
        let mut probe = Probe { os, gpus: merge_gpus(system_gpus, smi), drives, vc_runtime, ..Probe::default() };
        if cancelled() {
            return probe;
        }
        stage(Stage::Comfy);
        for url in urls.iter().take(8) {
            if !probe.comfy.running.contains(url) && photocraft_aigen::launch::is_running(transport, url) {
                probe.comfy.running.push(url.clone());
            }
        }
        probe.comfy.installed = photocraft_aigen::launch::find_installed();
        // Where its models are: a running one on this computer says; the installed one's
        // settings tell otherwise.
        let local = probe.comfy.running.iter().find(|u| photocraft_aigen::launch::port_of(u).1);
        probe.comfy.models = local.map(|u| photocraft_aigen::launch::served_models_folders(transport, u)).unwrap_or_default();
        if probe.comfy.models.is_empty()
            && let Some(launch) = &probe.comfy.installed
        {
            probe.comfy.models = photocraft_aigen::launch::models_folders_here(launch);
        }
        // Which version it is, to say which models it is too old for.
        probe.comfy.version = local
            .and_then(|u| photocraft_aigen::launch::server_version(transport, u))
            .or_else(|| probe.comfy.installed.as_ref().and_then(photocraft_aigen::launch::version_here))
            .unwrap_or_default();
        if cancelled() {
            return probe;
        }
        stage(Stage::Servers);
        // One real file from each host: what will be downloaded is what is asked about.
        let runtime = manifest.comfy.assets.values().next().map(|a| ("github.com", a.url.as_str(), a.size));
        let model = manifest.files.values().find(|f| !f.gated).map(|f| ("huggingface.co", f.url.as_str(), f.size));
        for (name, url, size) in [runtime, model].into_iter().flatten() {
            if cancelled() {
                break;
            }
            let host = match fetch.head(url, None) {
                Ok(head) if (200..400).contains(&head.status) && head.size.is_none_or(|s| s == size) => {
                    Host { name: name.into(), ok: true, note: String::new() }
                }
                Ok(head) if (200..400).contains(&head.status) => {
                    Host { name: name.into(), ok: false, note: "its files are not the ones this version of the app expects".into() }
                }
                Ok(head) => Host { name: name.into(), ok: false, note: format!("it answered {}", head.status) },
                Err(e) => Host { name: name.into(), ok: false, note: crate::fetch::clip(&e.to_string(), 160) },
            };
            probe.hosts.push(host);
        }
        probe
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::collect;

#[cfg(test)]
mod tests {
    use super::*;

    const SYSTEM: &str = r#"{"reg":[{"DriverDesc":"NVIDIA GeForce RTX 4090","HardwareInformation.qwMemorySize":25757220864},{"DriverDesc":"Intel(R) UHD Graphics 770","HardwareInformation.qwMemorySize":1073741824}],
      "gpu":[{"Name":"NVIDIA GeForce RTX 4090","AdapterRAM":4293918720,"DriverVersion":"32.0.15.9159"},{"Name":"Intel(R) UHD Graphics 770","AdapterRAM":1073741824,"DriverVersion":"31.0.101.5186"}],
      "disk":[{"DeviceID":"C:","FreeSpace":120000000000,"Size":1000000000000},{"DeviceID":"D:","FreeSpace":412000000000,"Size":2000000000000}],
      "os":{"Caption":"Microsoft Windows 11 Pro","Version":"10.0.26100","OSArchitecture":"64-bit"}}"#;

    fn probe() -> Probe {
        let (os, gpus, drives) = parse_system(SYSTEM);
        let smi = parse_nvidia_smi("NVIDIA GeForce RTX 4090, 24564, 591.59\r\n");
        Probe {
            os,
            gpus: merge_gpus(gpus, smi),
            drives,
            hosts: vec![
                Host { name: "github.com".into(), ok: true, note: String::new() },
                Host { name: "huggingface.co".into(), ok: true, note: String::new() },
            ],
            ..Probe::default()
        }
    }

    #[test]
    fn the_steps_are_told_in_order_each_by_a_message_of_its_own() {
        let messages: std::collections::BTreeSet<_> = Stage::ALL.iter().map(|s| s.message()).collect();
        assert_eq!(messages.len(), Stage::ALL.len());
        assert!(Stage::ALL.windows(2).all(|w| w[0] < w[1] && w[0].fraction() < w[1].fraction()));
    }

    #[test]
    fn the_first_screen_reads_as_the_walkthrough_shows_it() {
        let p = probe();
        let rows: Vec<(String, Level, String, String)> = p.checks().into_iter().map(|c| (c.id, c.level, c.title, c.value)).collect();
        assert_eq!(
            rows,
            [
                ("os".to_string(), Level::Ok, "Windows 11 Pro, 64-bit".to_string(), "supported".to_string()),
                ("gpu".to_string(), Level::Ok, "NVIDIA GeForce RTX 4090".to_string(), "24 GB".to_string()),
                ("disk".to_string(), Level::Ok, "Free disk space".to_string(), "D: 412 GB".to_string()),
                ("comfy".to_string(), Level::Info, "ComfyUI not found".to_string(), "will install".to_string()),
                ("net".to_string(), Level::Ok, "Download servers reachable".to_string(), "online".to_string()),
            ]
        );
        assert_eq!(p.variant(), ("nvidia", true));
        assert!(p.can_install());
        // A ComfyUI that is here says its version, when it is known.
        let comfy_title = |p: &Probe| p.checks().into_iter().find(|c| c.id == "comfy").map(|c| c.title).unwrap_or_default();
        let mut found = probe();
        found.comfy.running = vec!["http://127.0.0.1:8000".into()];
        assert_eq!(comfy_title(&found), "ComfyUI detected");
        found.comfy.version = "0.41.0".into();
        assert_eq!(comfy_title(&found), "ComfyUI 0.41.0 detected");
        // The card's real memory and driver come from nvidia-smi; the other card from Windows.
        assert_eq!(p.gpus.len(), 2);
        assert_eq!((p.gpus[0].vram_mb, p.gpus[0].driver.as_str()), (Some(24564), "591.59"));
        assert_eq!((p.gpus[1].vendor, p.gpus[1].vram_mb), (Vendor::Intel, Some(1024)));
    }

    /// This machine, for real: `cargo test -p photocraft-aisetup --lib this_machine -- --ignored --nocapture`.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    #[ignore = "looks at this machine and the network"]
    fn this_machine_is_described() {
        let m = crate::Manifest::bundled().unwrap();
        let fetch = crate::fetch::UreqFetch::default();
        let transport = photocraft_aigen::UreqTransport::new();
        let urls = ["http://127.0.0.1:8188".to_string(), "http://127.0.0.1:8000".to_string()];
        let found = collect(m, &transport, &fetch, &urls, &|| false, &|step| println!("{}", step.message()));
        println!("{}", serde_json::to_string_pretty(&found).unwrap());
        for check in found.checks() {
            println!("{:?} {} | {} | {}", check.level, check.title, check.detail, check.value);
        }
        println!("variant: {:?}", found.variant());
        assert_eq!(found.checks().len(), if found.os.memory_mb.is_some() { 6 } else { 5 });
    }

    #[test]
    fn memory_is_read_and_little_of_it_or_a_small_fixed_paging_file_is_a_warning() {
        let json = r#"{"os":{"Caption":"Microsoft Windows 11 Pro","OSArchitecture":"64-bit","TotalVisibleMemorySize":16760832,"TotalVirtualMemorySize":"19251200"},"cs":{"AutomaticManagedPagefile":true}}"#;
        let (os, _, _) = parse_system(json);
        assert_eq!((os.memory_mb, os.virtual_mb, os.paging_managed), (Some(16_368), Some(18_800), Some(true)));
        let row = |os: &Os| Probe { os: os.clone(), ..Probe::default() }.checks().into_iter().find(|c| c.id == "memory").map(|c| (c.level, c.title, c.value));
        assert_eq!(row(&os), Some((Level::Ok, "Memory".to_string(), "16 GB".to_string())));
        // A fixed paging file that leaves little: it won't grow when a model needs it to.
        let fixed = Os { paging_managed: Some(false), ..os.clone() };
        assert_eq!(row(&fixed).map(|r| (r.0, r.1)), Some((Level::Warn, "Small paging file".to_string())));
        let little = Os { memory_mb: Some(8_192), ..os.clone() };
        assert_eq!(row(&little).map(|r| (r.0, r.1, r.2)), Some((Level::Warn, "Little memory".to_string(), "8 GB".to_string())));
        // Not told: no row, and odd numbers are no answer.
        assert_eq!(row(&Os::default()), None);
        let (odd, _, _) = parse_system(r#"{"os":{"TotalVisibleMemorySize":-5,"TotalVirtualMemorySize":"lots"},"cs":{"AutomaticManagedPagefile":"yes"}}"#);
        assert_eq!((odd.memory_mb, odd.virtual_mb, odd.paging_managed), (None, None, None));
    }

    #[test]
    fn without_nvidia_smi_the_registry_gives_the_cards_real_memory() {
        let (_, gpus, _) = parse_system(SYSTEM);
        // 24 GB, not the 4 GB `AdapterRAM` stops at.
        assert_eq!(gpus[0].vram_mb, Some(25757220864 >> 20));
        // A single card and a single drive come as objects, not lists.
        let one = r#"{"gpu":{"Name":"AMD Radeon RX 7900 XTX","AdapterRAM":"4293918720","DriverVersion":"31.0"},"disk":{"DeviceID":"C:","FreeSpace":5.0e9,"Size":"256000000000"},"os":{"Caption":"Microsoft Windows 10 Home","Version":"10.0.19045","OSArchitecture":"64-bit"}}"#;
        let (os, gpus, drives) = parse_system(one);
        assert_eq!((os.name.as_str(), os.windows, os.sixty_four_bit), ("Windows 10 Home", true, true));
        assert_eq!((gpus.len(), gpus[0].vendor, gpus[0].vram_mb), (1, Vendor::Amd, Some(4095)));
        assert_eq!(drives, [Drive { name: "C:".into(), free: 5_000_000_000, total: 256_000_000_000 }]);
    }

    #[test]
    fn odd_answers_from_the_system_are_no_answer() {
        for junk in
            ["", "not json", "[]", "{}", "{\"gpu\":7,\"disk\":\"x\",\"os\":[]}", "{\"gpu\":[{\"Name\":\"\"}],\"disk\":[{\"DeviceID\":\"C:\"}]}", "\u{feff}{}"]
        {
            let (os, gpus, drives) = parse_system(junk);
            assert!(!os.windows && gpus.is_empty() && drives.is_empty(), "{junk}");
        }
        for junk in ["", "\n\n", "no commas here", "name, not a number, 1", ", 24564, 1", "x, 0, 1", "x, 99999999999, 1"] {
            assert!(parse_nvidia_smi(junk).is_empty(), "{junk}");
        }
        assert_eq!(parse_nvidia_smi("A, 8192\nB, 12288, 572.16").len(), 2);
    }

    #[test]
    fn the_build_follows_the_card_and_its_driver() {
        let with = |name: &str, driver: &str| Probe {
            gpus: vec![Gpu { name: name.into(), vendor: Vendor::of(name), vram_mb: Some(8192), driver: driver.into() }],
            ..probe()
        };
        assert_eq!(with("NVIDIA GeForce RTX 3060", "591.59").variant(), ("nvidia", true));
        assert_eq!(with("NVIDIA GeForce RTX 3060", "552.22").variant(), ("nvidia_cu126", true));
        assert_eq!(with("NVIDIA GeForce GTX 1080 Ti", "591.59").variant(), ("nvidia_cu126", true));
        assert_eq!(with("NVIDIA GeForce GTX 1660", "591.59").variant(), ("nvidia", true));
        assert_eq!(with("NVIDIA GeForce RTX 3060", "").variant(), ("nvidia", true));
        assert_eq!(with("AMD Radeon RX 7900 XTX", "31.0").variant(), ("amd", false));
        assert_eq!(with("Intel(R) Arc(TM) A770", "31.0").variant(), ("intel", false));
        assert_eq!(Probe::default().variant(), ("nvidia", false));
        // The old card says so; the untested ones warn; no card warns and points at cloud models.
        let level = |p: &Probe| p.checks().into_iter().find(|c| c.id == "gpu").map(|c| (c.level, c.detail)).unwrap();
        assert!(matches!(level(&with("NVIDIA GeForce GTX 1080 Ti", "591.59")), (Level::Info, d) if d.contains("CUDA 12.6")));
        assert!(matches!(level(&with("AMD Radeon RX 7900 XTX", "31.0")), (Level::Warn, d) if d.contains("Not tested")));
        // No card the engine can use (a virtual machine's): local models stop here.
        let none = Probe { gpus: Vec::new(), ..probe() };
        assert!(matches!(level(&none), (Level::Stop, d) if d.contains("Cloud models") && d.contains("virtual machine")));
        let vm = with("VMware SVGA 3D", "9.17");
        assert!(matches!(level(&vm), (Level::Stop, _)) && matches!(level(&with("Microsoft Basic Display Adapter", "")), (Level::Stop, _)));
        assert!(none.usable_gpu().is_none() && vm.usable_gpu().is_none() && vm.gpu().is_some());
        assert!(with("AMD Radeon RX 7900 XTX", "31.0").usable_gpu().is_some() && with("NVIDIA GeForce RTX 3060", "591.59").usable_gpu().is_some());
        // Installing stays possible (on the processor): the system is fine, only the card is missing.
        assert!(vm.can_install());
    }

    #[test]
    fn what_stands_in_the_way_is_said_plainly() {
        let mut p = probe();
        p.os = Os { name: "macOS".into(), ..Os::default() };
        p.drives = vec![Drive { name: "C:".into(), free: 3_000_000_000, total: 100_000_000_000 }];
        p.comfy.running = vec!["http://127.0.0.1:8000".into()];
        p.hosts[1] = Host { name: "huggingface.co".into(), ok: false, note: "it answered 503".into() };
        let rows = p.checks();
        let by = |id: &str| rows.iter().find(|c| c.id == id).unwrap();
        assert_eq!((by("os").level, by("os").value.as_str()), (Level::Stop, "use your own"));
        assert!(!p.can_install());
        assert_eq!((by("disk").level, by("disk").value.as_str()), (Level::Warn, "C: 3.00 GB"));
        assert_eq!((by("comfy").title.as_str(), by("comfy").value.as_str()), ("ComfyUI detected", "running"));
        assert!(by("comfy").detail.starts_with("Running at http://127.0.0.1:8000. Choose below"), "{}", by("comfy").detail);
        assert!(by("net").level == Level::Warn && by("net").detail.contains("huggingface.co did not answer (it answered 503)"));
        // 32-bit Windows can't run the engine; an unchecked network says so.
        p.os = Os { name: "Windows 10 Home".into(), windows: true, ..Os::default() };
        p.hosts.clear();
        p.comfy = Existing { installed: Some(Launch::default()), models: vec![r"C:\Users\a\ComfyUI-Shared\models".into()], ..Existing::default() };
        let rows = p.checks();
        let by = |id: &str| rows.iter().find(|c| c.id == id).unwrap();
        assert_eq!((by("os").level, by("net").level, by("comfy").value.as_str()), (Level::Stop, Level::Info, "installed"));
        assert!(by("comfy").detail.contains(r"Its models are in C:\Users\a\ComfyUI-Shared\models."), "{}", by("comfy").detail);
        // The whole thing round-trips, for the control channel.
        let json = serde_json::to_string(&probe()).unwrap();
        assert_eq!(serde_json::from_str::<Probe>(&json).unwrap(), probe());
        assert_eq!(serde_json::from_str::<Probe>("{}").unwrap(), Probe::default());
    }
    #[test]
    fn a_pc_the_engine_would_not_start_on_says_so_and_how_to_fix_it() {
        let row = |p: &Probe| p.checks().into_iter().find(|c| c.id == "vcRuntime");
        // Carried by the app, installed, or not looked at: nothing to say.
        for found in [VcRuntime::Bundled, VcRuntime::Installed, VcRuntime::Unknown] {
            assert!(row(&Probe { vc_runtime: found, ..probe() }).is_none(), "{found:?}");
        }
        let missing = row(&Probe { vc_runtime: VcRuntime::Missing, ..probe() }).unwrap();
        assert!(missing.level == Level::Warn && missing.detail.contains("vc_redist.x64.exe"), "{missing:?}");
        assert!(Probe { vc_runtime: VcRuntime::Missing, ..probe() }.can_install(), "a warning, not a stop");
    }
}
