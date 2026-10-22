//! Starting ComfyUI for the user, so local models work without opening it first.
//!
//! [`LocalServer`] is what the engine sees: "make sure the server is up". [`ComfyProcess`] (native
//! only) is the implementation the desktop app installs. Its rules:
//!
//! - **A server that already answers is used as it is.** Nothing is started, and nothing is
//!   stopped later: it is the user's (ComfyUI Desktop, a terminal, another machine's tunnel).
//! - Otherwise, when the settings allow it, ComfyUI is started hidden with the configured Python,
//!   folder and arguments on the port of the configured address, and waited for.
//! - Only a server this process started is ever stopped: on [`LocalServer::stop`] and when the
//!   [`ComfyProcess`] is dropped (the app quitting).
//!
//! [`find_running`] reads the launch settings off a ComfyUI that is running now, which is how
//! the settings are filled in without the user knowing where their Python lives.

use serde::{Deserialize, Serialize};

use crate::http::{Request, Transport};
use crate::model::Settings;
use crate::run::Control;
use crate::{Error, Result, comfy};

/// How to start ComfyUI.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Launch {
    /// The Python that has ComfyUI's packages (its virtual environment's, or the portable
    /// build's `python_embeded`).
    pub python: String,
    /// The folder ComfyUI is started in.
    pub folder: String,
    /// Arguments after the Python executable: the script (`main.py`) and its options, without
    /// `--port` (the port comes from the server address).
    pub args: String,
}

/// What the app knows about the local server without asking it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerState {
    /// Not started by this app (it may still be running: started elsewhere).
    NotStarted,
    /// Started by this app and still alive.
    Started,
}

/// The engine's view of the local server.
pub trait LocalServer: Send + Sync {
    /// Make sure a ComfyUI answers at the configured address: use a running one, or start one if
    /// the settings say so. Blocks until it answers, fails, or `control` cancels.
    fn ensure(&self, transport: &dyn Transport, settings: &Settings, control: &Control<'_>) -> Result<()>;
    /// Stop the server if this app started it. Returns whether it did.
    fn stop(&self) -> bool;
    fn state(&self) -> ServerState;
    /// The launch settings of the ComfyUI running at `url` now.
    fn find(&self, transport: &dyn Transport, url: &str) -> Result<Launch>;
    /// How the server this app started ended, when it has stopped without being asked to (its
    /// exit code and last output): what a render that lost its server reports. `None` while it
    /// runs, after [`LocalServer::stop`], and for a server started elsewhere.
    fn stopped(&self) -> Option<String> {
        None
    }
    /// The last `lines` lines the server this app started printed, oldest first, as a terminal
    /// shows them ([`crate::activity::lines`]): what it is doing while a step takes long. Empty
    /// for a server started elsewhere.
    fn output(&self, lines: usize) -> Vec<String> {
        let _ = lines;
        Vec::new()
    }
    /// Where all of its output goes, when this app keeps it.
    fn log_file(&self) -> Option<String> {
        None
    }
}

/// Does a ComfyUI answer at `base` (no trailing slash)? Anything else on the port does not count.
pub fn is_running(transport: &dyn Transport, base: &str) -> bool {
    server_argv(transport, base).is_some()
}

/// The command line of the ComfyUI at `base`, as it reports it (`/system_stats`).
fn server_argv(transport: &dyn Transport, base: &str) -> Option<Vec<String>> {
    let r = transport.send(&Request::get(format!("{base}/system_stats")).timeout(3)).ok().filter(|r| r.ok())?;
    let v: serde_json::Value = serde_json::from_slice(&r.body).ok()?;
    let system = v.get("system")?.as_object()?;
    Some(system.get("argv").and_then(|a| a.as_array()).into_iter().flatten().filter_map(|a| a.as_str().map(str::to_string)).take(256).collect())
}

/// Why a ComfyUI that quit while starting quit, from its output (oldest line first): the last
/// line that names a failure (`Failed to …`, `SomethingError: …`, `main.py: error: …`) and the
/// one after it, which often says what it needed. Its last lines are usually unrelated chatter
/// (add-ons finishing their own start).
pub fn startup_cause(lines: &[&str]) -> Option<String> {
    let names_failure = |l: &&str| {
        let l = l.trim();
        let exception = l.split_once(": ").is_some_and(|(head, _)| !head.contains(' ') && (head.ends_with("Error") || head.ends_with("Exception")));
        l.starts_with("Failed to ") || l.contains(": error: ") || exception
    };
    let at = lines.iter().rposition(names_failure)?;
    let cause = lines.get(at)?.trim();
    let next = lines.get(at.saturating_add(1)).map(|l| l.trim()).filter(|l| !l.is_empty() && !l.starts_with("File ") && !l.starts_with("Traceback"));
    let mut out = match next {
        Some(next) => format!("{cause} | {next}"),
        None => cause.to_string(),
    };
    // Two copies of ComfyUI that share a user folder: the newer one moved the database on.
    if out.contains("Can't locate revision") {
        out.push_str(
            ". This ComfyUI is older than one that has used the same user folder. With your usual ComfyUI running, press Find Installation in Preferences › Generative AI › Local Server so that one is started instead",
        );
    }
    Some(out)
}

/// The version the ComfyUI at `base` says it is (`/system_stats`).
fn server_version(transport: &dyn Transport, base: &str) -> Option<String> {
    let r = transport.send(&Request::get(format!("{base}/system_stats")).timeout(3)).ok().filter(|r| r.ok())?;
    let v: serde_json::Value = serde_json::from_slice(&r.body).ok()?;
    v.pointer("/system/comfyui_version").and_then(serde_json::Value::as_str).filter(|s| !s.is_empty() && s.len() <= 32).map(str::to_string)
}

/// The port of an `http://host:port` address (ComfyUI's default without one), and whether the
/// host is this computer.
pub fn port_of(base: &str) -> (u16, bool) {
    let rest = base.split_once("://").map_or(base, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    // `[::1]:8188` keeps its brackets; a bare name or IPv4 splits at the last colon.
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !p.contains(']') => (h, p.parse::<u16>().ok()),
        _ => (authority, None),
    };
    let local = matches!(host.trim_matches(['[', ']']), "127.0.0.1" | "localhost" | "::1" | "0.0.0.0");
    (port.filter(|p| *p != 0).unwrap_or(8188), local)
}

/// Where a ComfyUI may be answering, `base` first: on this computer also the other port a
/// ComfyUI listens on (ComfyUI Desktop 8000, the portable build and a manual install 8188).
/// Another computer's address is only itself.
pub fn addresses(base: &str) -> Vec<String> {
    let base = base.trim().trim_end_matches('/');
    let mut out = vec![base.to_string()];
    let (port, local) = port_of(base);
    if !local {
        return out;
    }
    let (scheme, rest) = base.split_once("://").unwrap_or(("http", base));
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = match authority.rsplit_once(':') {
        Some((h, p)) if !p.contains(']') => h,
        _ => authority,
    };
    out.extend([8000u16, 8188].into_iter().filter(|p| *p != port).map(|p| format!("{scheme}://{host}:{p}")));
    out
}

/// Split a command line into arguments: spaces separate, double quotes group.
pub fn split_args(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut quoted, mut any) = (false, false);
    for c in line.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        out.push(cur);
    }
    out
}

/// Join arguments into a line [`split_args`] reads back.
pub fn join_args(args: &[String]) -> String {
    args.iter().map(|a| if a.is_empty() || a.chars().any(char::is_whitespace) { format!("\"{a}\"") } else { a.clone() }).collect::<Vec<_>>().join(" ")
}

/// `args` without `--port <n>` / `--port=<n>`: the port is the server address's to decide.
pub fn without_port(args: Vec<String>) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
        } else if a == "--port" {
            skip = true;
        } else if !a.starts_with("--port=") {
            out.push(a);
        }
    }
    out
}

/// Where a Python for the ComfyUI in `dir` usually is: its own virtual environment, the portable
/// build's embedded one beside it, or a virtual environment one level up.
fn python_candidates(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let up = dir.parent().unwrap_or(dir);
    vec![
        dir.join(".venv/Scripts/python.exe"),
        dir.join("venv/Scripts/python.exe"),
        up.join("python_embeded/python.exe"),
        up.join(".venv/Scripts/python.exe"),
        dir.join(".venv/bin/python"),
        dir.join("venv/bin/python"),
        up.join(".venv/bin/python"),
    ]
}

/// The launch settings after the user picked `path` with Browse, starting from `current`:
/// a Python executable sets the Python; a script (`main.py`) sets the folder and, when there are
/// no arguments yet, the script to run. Whichever half is still empty is filled in when it sits
/// in one of the usual places next to the other. `exists` answers "is this a file?".
pub fn from_picked(path: &str, current: &Launch, exists: &dyn Fn(&std::path::Path) -> bool) -> Launch {
    use std::path::Path;
    let mut launch = current.clone();
    let picked = Path::new(path.trim());
    let name = picked.file_name().and_then(|n| n.to_str()).unwrap_or("").to_ascii_lowercase();
    let text = |p: &Path| p.to_string_lossy().into_owned();
    if name.ends_with(".py") {
        let folder = picked.parent().unwrap_or(Path::new(""));
        launch.folder = text(folder);
        if split_args(&launch.args).first().is_none_or(|a| !a.to_ascii_lowercase().ends_with(".py")) {
            let script = picked.file_name().and_then(|n| n.to_str()).unwrap_or("main.py").to_string();
            launch.args = join_args(&std::iter::once(script).chain(split_args(&launch.args)).collect::<Vec<_>>());
        }
        if launch.python.trim().is_empty()
            && let Some(python) = python_candidates(folder).into_iter().find(|p| exists(p))
        {
            // The portable build wants its own flag, as its `run_*.bat` files pass it.
            if python.components().any(|c| c.as_os_str() == "python_embeded") && !launch.args.contains("--windows-standalone-build") {
                launch.args = format!("{} --windows-standalone-build", launch.args);
            }
            launch.python = text(&python);
        }
    } else if !name.is_empty() {
        launch.python = text(picked);
        if launch.folder.trim().is_empty() {
            // `<project>/.venv/Scripts/python.exe`, `<portable>/python_embeded/python.exe`.
            let roots = picked.ancestors().skip(1).take(3);
            if let Some(folder) = roots.flat_map(|r| [r.to_path_buf(), r.join("ComfyUI")]).find(|d| exists(&d.join("main.py"))) {
                launch.folder = text(&folder);
                if launch.args.trim().is_empty() {
                    launch.args = "main.py".into();
                }
            }
        }
    }
    launch
}

/// The launch settings of a ComfyUI that ComfyUI Desktop installed, from Desktop's own record
/// (`installations.json` in its settings folder `config`): the installation used last that has
/// a Python and a `main.py` on disk. `exists` answers "is this a file?".
pub fn desktop_install(record: &str, config: &std::path::Path, exists: &dyn Fn(&std::path::Path) -> bool) -> Option<Launch> {
    use std::path::{Path, PathBuf};
    let v: serde_json::Value = serde_json::from_str(record).ok()?;
    let mut entries: Vec<&serde_json::Value> = v.as_array()?.iter().take(64).collect();
    let stamp = |e: &serde_json::Value| e.get("lastLaunchedAt").and_then(serde_json::Value::as_f64).unwrap_or(0.0);
    entries.sort_by(|a, b| stamp(b).total_cmp(&stamp(a)));
    let text_of = |e: &serde_json::Value, keys: &[&str]| {
        keys.iter().find_map(|k| e.get(*k).and_then(serde_json::Value::as_str)).filter(|s| !s.trim().is_empty()).map(str::to_string)
    };
    for e in entries {
        let Some(install) = text_of(e, &["installPath"]) else { continue };
        let install = PathBuf::from(install);
        let python = text_of(e, &["adoptedPythonPath", "pythonPath"])
            .map(PathBuf::from)
            .filter(|p| exists(p))
            .or_else(|| python_candidates(&install.join("ComfyUI")).into_iter().chain(python_candidates(&install)).find(|p| exists(p)));
        let Some(python) = python else { continue };
        // Started from the installation's folder with the script relative to it, as Desktop does.
        let Some(script) = ["ComfyUI/main.py", "main.py"].into_iter().find(|s| exists(&install.join(s))) else { continue };
        let mut args =
            vec![Path::new(script).components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join(std::path::MAIN_SEPARATOR_STR)];
        let base = text_of(e, &["adoptedBaseDir", "baseDir", "basePath"]);
        if let Some(base) = &base {
            args.extend([
                "--base-directory".to_string(),
                base.clone(),
                "--user-directory".to_string(),
                Path::new(base).join("user").to_string_lossy().into_owned(),
            ]);
        }
        for (flag, keys) in [("--input-directory", ["inputDir"]), ("--output-directory", ["outputDir"])] {
            if let Some(dir) = text_of(e, &keys) {
                args.extend([flag.to_string(), dir]);
            }
        }
        // Desktop's list of shared model folders for this installation.
        if let Some(id) = text_of(e, &["id"]).filter(|id| id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')) {
            let paths = config.join("instance-model-paths").join(format!("{id}.yaml"));
            if exists(&paths) {
                args.extend(["--extra-model-paths-config".to_string(), paths.to_string_lossy().into_owned()]);
            }
        }
        args.extend(without_port(split_args(&text_of(e, &["launchArgs"]).unwrap_or_default())));
        return Some(Launch { python: python.to_string_lossy().into_owned(), folder: install.to_string_lossy().into_owned(), args: join_args(&args) });
    }
    None
}

/// The installation folders in ComfyUI Desktop's record (`installations.json`), the one launched
/// last first: where a ComfyUI it runs is started from.
pub fn desktop_folders(record: &str) -> Vec<std::path::PathBuf> {
    let Ok(serde_json::Value::Array(entries)) = serde_json::from_str::<serde_json::Value>(record) else { return Vec::new() };
    let mut entries: Vec<&serde_json::Value> = entries.iter().take(64).collect();
    let stamp = |e: &serde_json::Value| e.get("lastLaunchedAt").and_then(serde_json::Value::as_f64).unwrap_or(0.0);
    entries.sort_by(|a, b| stamp(b).total_cmp(&stamp(a)));
    entries.iter().filter_map(|e| e.get("installPath")?.as_str()).filter(|p| !p.trim().is_empty()).map(std::path::PathBuf::from).collect()
}

/// The version a ComfyUI source folder says it is (`comfyui_version.py`: `__version__ = "0.42.0"`).
pub fn version_in(text: &str) -> Option<String> {
    let line = text.lines().find(|l| l.trim_start().starts_with("__version__"))?;
    let value = line.split_once('=')?.1.trim().trim_matches(['"', '\'']);
    (!value.is_empty() && value.len() <= 32).then(|| value.to_string())
}

/// The first of `candidates` that holds `script` (relative to it) and, when the running server
/// said its `version`, is that version: a stale copy beside the real one is never taken for it.
/// `read` reads a small text file.
pub fn pick_folder(
    candidates: &[std::path::PathBuf],
    script: &str,
    version: Option<&str>,
    read: &dyn Fn(&std::path::Path) -> Option<String>,
    is_file: &dyn Fn(&std::path::Path) -> bool,
) -> Option<std::path::PathBuf> {
    let holds = |d: &&std::path::PathBuf| is_file(&d.join(script));
    let same = |d: &&std::path::PathBuf| {
        let file = d.join(script).parent().map(|p| p.join("comfyui_version.py"));
        let found = file.and_then(|f| read(&f)).and_then(|t| version_in(&t));
        version.is_none_or(|want| found.as_deref() == Some(want))
    };
    candidates.iter().filter(holds).find(same).cloned()
}

/// `0.42.1` as numbers, for ordering versions (a part's leading digits; `0.42.1+9` is `0.42.1`).
fn version_parts(v: &str) -> Vec<u64> {
    v.split(['+', '-']).next().unwrap_or("").split('.').take(8).map(|p| p.chars().take_while(char::is_ascii_digit).collect::<String>().parse().unwrap_or(0)).collect()
}

/// A newer copy of the ComfyUI in `folder` to start instead: the first of `installed`
/// (ComfyUI Desktop's installations, [`desktop_folders`]) that holds `script` and is a newer
/// version than `folder`'s. Two copies can share a user folder, and the older one then quits on
/// the database the newer one moved on (settings saved by an older Find Installation pointed at
/// such a copy). None when `folder`'s version is unknown or nothing newer is there.
pub fn newer_copy(
    folder: &std::path::Path,
    script: &str,
    installed: &[std::path::PathBuf],
    read: &dyn Fn(&std::path::Path) -> Option<String>,
    is_file: &dyn Fn(&std::path::Path) -> bool,
) -> Option<std::path::PathBuf> {
    let version = |d: &std::path::Path| d.join(script).parent().and_then(|p| read(&p.join("comfyui_version.py"))).and_then(|t| version_in(&t));
    let own = version_parts(&version(folder)?);
    installed
        .iter()
        .filter(|d| d.as_path() != folder && is_file(&d.join(script)))
        .find(|d| version(d).is_some_and(|v| version_parts(&v) > own))
        .cloned()
}

/// The folders a model file can go in for the ComfyUI `launch` starts to see it, in ComfyUI's
/// own order: the first is where it saves new ones itself. Each is a models folder, the one
/// that holds `checkpoints`, `diffusion_models`, `vae` and the rest: ComfyUI's own (`models` in
/// its base folder, or `--models-directory`), and those its extra model paths files add (each
/// one passed with `--extra-model-paths-config`, and `extra_model_paths.yaml` beside its
/// script). Only folders that exist are listed. `read` reads a small text file; `is_dir`
/// answers "is this a folder?".
pub fn models_folders(launch: &Launch, read: &dyn Fn(&std::path::Path) -> Option<String>, is_dir: &dyn Fn(&std::path::Path) -> bool) -> Vec<String> {
    use std::path::{Path, PathBuf};
    let folder = PathBuf::from(launch.folder.trim());
    if folder.as_os_str().is_empty() {
        return Vec::new();
    }
    let args = split_args(&launch.args);
    // Relative paths are from the folder it is started in.
    let after = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).map(|p| folder.join(p));
    let script = args.iter().find(|a| a.to_ascii_lowercase().ends_with(".py")).map(|s| folder.join(s));
    let comfy = script.as_deref().and_then(Path::parent).map_or_else(|| folder.clone(), Path::to_path_buf);
    let base = after("--base-directory").unwrap_or_else(|| comfy.clone());
    let own = after("--models-directory").unwrap_or_else(|| base.join("models"));
    // ComfyUI reads the file beside its script first, then each one given, in order
    // (`--extra-model-paths-config a.yaml b.yaml`, as often as it is given).
    let mut configs = vec![comfy.join("extra_model_paths.yaml")];
    for (i, a) in args.iter().enumerate() {
        if a == "--extra-model-paths-config" {
            configs.extend(args.iter().skip(i + 1).take_while(|x| !x.starts_with("--")).map(|x| folder.join(x)));
        }
    }
    // As ComfyUI orders a folder type's paths: one marked `is_default` goes first, others last.
    let key = |p: &Path| p.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_lowercase();
    let mut order = vec![own];
    for config in configs.iter().take(16) {
        let Some(yaml) = read(config) else { continue };
        let dir = config.parent().unwrap_or(Path::new(""));
        for (root, default) in extra_model_roots(&yaml, dir) {
            let at = order.iter().position(|p| key(p) == key(&root));
            match (at, default) {
                (Some(i), true) => {
                    let p = order.remove(i);
                    order.insert(0, p);
                }
                (Some(_), false) => {}
                (None, true) => order.insert(0, root),
                (None, false) => order.push(root),
            }
        }
    }
    order.into_iter().filter(|p| is_dir(p)).map(|p| p.to_string_lossy().into_owned()).collect()
}

/// The models folders one extra model paths file (`extra_model_paths.yaml`) adds, in its order,
/// each with whether it is marked `is_default`. `dir` is the file's folder, which a relative
/// `base_path` is from. ComfyUI reads the file as YAML; this reads the shape such files have
/// (sections of `key: value`, values over several lines after `|`), which is all it needs.
fn extra_model_roots(yaml: &str, dir: &std::path::Path) -> Vec<(std::path::PathBuf, bool)> {
    use std::path::{Component, Path, PathBuf};
    #[derive(Default)]
    struct Section {
        base: Option<String>,
        default: bool,
        entries: Vec<(String, String)>,
    }
    let mut sections: Vec<Section> = Vec::new();
    // The key whose value is on the lines below it, and how far it is indented.
    let mut block: Option<(String, usize)> = None;
    for line in yaml.lines().take(10_000) {
        let body = line.trim();
        let indent = line.len() - line.trim_start().len();
        if body.is_empty() || body.starts_with('#') {
            continue;
        }
        if let Some((key, at)) = &block {
            if indent > *at {
                if let Some(s) = sections.last_mut() {
                    s.entries.push((key.clone(), body.to_string()));
                }
                continue;
            }
            block = None;
        }
        let Some((key, raw)) = yaml_pair(body) else { continue };
        if indent == 0 {
            sections.push(Section::default());
            continue;
        }
        let Some(s) = sections.last_mut() else { continue };
        match key.as_str() {
            "base_path" => s.base = Some(yaml_scalar(raw)),
            // A quoted string counts as true when it is not empty, as Python reads it.
            "is_default" if raw.starts_with(['\'', '"']) => s.default = !yaml_scalar(raw).is_empty(),
            "is_default" => s.default = !matches!(yaml_scalar(raw).as_str(), "" | "false" | "False" | "FALSE" | "no" | "No" | "off" | "null" | "~" | "0"),
            _ if raw.starts_with('|') || raw.starts_with('>') => block = Some((key, indent)),
            _ => s.entries.push((key, yaml_scalar(raw))),
        }
    }
    let tidy = |p: PathBuf| p.components().filter(|c| !matches!(c, Component::CurDir)).collect::<PathBuf>();
    let mut out = Vec::new();
    for s in sections {
        let base = match &s.base {
            Some(b) if !b.trim().is_empty() => dir.join(b.trim()),
            _ => dir.to_path_buf(),
        };
        // The folder that holds `diffusion_models` (or another standard one) is the models folder.
        let root = ["diffusion_models", "checkpoints", "vae", "text_encoders", "unet", "clip", "loras"].into_iter().find_map(|kind| {
            let (_, value) = s.entries.iter().find(|(k, v)| k.eq_ignore_ascii_case(kind) && !v.trim().is_empty())?;
            let path = base.join(value.trim().trim_end_matches(['/', '\\']));
            let named = path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.eq_ignore_ascii_case(kind));
            named.then(|| path.parent().map(Path::to_path_buf)).flatten()
        });
        if let Some(root) = root {
            out.push((tidy(root), s.default));
        }
    }
    out
}

/// `key: value` on one YAML line: the key without its quotes, and the value as written.
fn yaml_pair(body: &str) -> Option<(String, &str)> {
    let (key, rest) = match body.chars().next() {
        Some(q @ ('\'' | '"')) => {
            let inner = body.get(1..)?;
            let end = inner.find(q)?;
            (inner.get(..end)?.to_string(), inner.get(end + 1..)?.trim_start().strip_prefix(':')?)
        }
        _ => {
            // The first colon followed by a space or the end: `C:\` in a value is not one.
            let at = body.char_indices().find(|&(i, c)| c == ':' && body.get(i + 1..).is_some_and(|r| r.is_empty() || r.starts_with(' ')))?.0;
            (body.get(..at)?.trim().to_string(), body.get(at + 1..)?)
        }
    };
    Some((key, rest.trim()))
}

/// A YAML value as text: quotes removed (`''` in single quotes is one, `\\` in double quotes
/// one backslash), and a comment after a plain value dropped.
fn yaml_scalar(raw: &str) -> String {
    let raw = raw.trim();
    if let Some(inner) = raw.strip_prefix('\'') {
        return inner.rsplit_once('\'').map_or(inner, |(v, _)| v).replace("''", "'");
    }
    if let Some(inner) = raw.strip_prefix('"') {
        let mut out = String::new();
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => break,
                '\\' => match chars.next() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some(other) => out.push(other),
                    None => break,
                },
                c => out.push(c),
            }
        }
        return out;
    }
    raw.split(" #").next().unwrap_or("").trim().to_string()
}

/// The models folders the ComfyUI at `base` reports it reads (`/experiment/models`), the one it
/// saves new files to first; empty when it can't tell (an older ComfyUI, or no answer).
pub fn served_models_folders(transport: &dyn Transport, base: &str) -> Vec<String> {
    let Ok(base) = comfy::base_url(base) else { return Vec::new() };
    let Some(r) = transport.send(&Request::get(format!("{base}/experiment/models")).timeout(5)).ok().filter(|r| r.ok()) else { return Vec::new() };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&r.body) else { return Vec::new() };
    let kinds = v.as_array().map_or(&[][..], Vec::as_slice);
    let folders_of = |kind: &str| -> Vec<String> {
        let Some(entry) = kinds.iter().take(256).find(|e| e.get("name").and_then(serde_json::Value::as_str) == Some(kind)) else { return Vec::new() };
        let paths = entry.get("folders").and_then(serde_json::Value::as_array).map_or(&[][..], Vec::as_slice);
        let mut out: Vec<String> = Vec::new();
        for p in paths.iter().take(64).filter_map(serde_json::Value::as_str) {
            // `C:\…\models\diffusion_models` → `C:\…\models`; a folder of its own name is not one.
            let trimmed = p.trim_end_matches(['/', '\\']);
            let Some((root, name)) = trimmed.rsplit_once(['/', '\\']) else { continue };
            if name.eq_ignore_ascii_case(kind) && !root.is_empty() && !out.iter().any(|o| o.eq_ignore_ascii_case(root)) {
                out.push(root.to_string());
            }
        }
        out
    };
    let found = folders_of("diffusion_models");
    if found.is_empty() { folders_of("checkpoints") } else { found }
}

/// Where the launch settings are, for messages.
const WHERE: &str = "Preferences › Generative AI › Local Server";

/// Why nothing answers at `base` and what to do about it, given what the settings allow: the
/// message for a generation that found no server and may not start one.
pub fn not_running_message(settings: &Settings, base: &str) -> String {
    let (_, local) = port_of(base);
    let configured = !settings.comfy_launch.python.trim().is_empty() && !settings.comfy_launch.folder.trim().is_empty();
    if !local {
        return format!(
            "ComfyUI is not answering at {base}. It is on another computer, so PhotoCraft can't start it: check that it is running there and reachable from here."
        );
    }
    if settings.comfy_autostart {
        // Asked to start it, but the app has no launcher (the web build).
        return format!("ComfyUI is not running at {base}. Start ComfyUI, then try again.");
    }
    if configured {
        format!(
            "ComfyUI is not running at {base}, and starting it automatically is switched off. Start ComfyUI yourself, or switch on \"Start ComfyUI when a local model needs it\" in {WHERE}."
        )
    } else {
        format!(
            "ComfyUI is not running at {base}, and PhotoCraft is not set up to start it. Start ComfyUI yourself and try again. To have PhotoCraft start it from now on: with ComfyUI running, open {WHERE} and press Find Installation."
        )
    }
}

/// The address without a trailing slash, or the reason it can't be used to start a server.
#[cfg(not(target_arch = "wasm32"))]
fn local_base(settings: &Settings) -> Result<(String, u16)> {
    let base = comfy::base_url(&settings.comfy_url)?;
    let (port, local) = port_of(&base);
    if !local {
        return Err(Error::Config(format!("ComfyUI at {base} is not running, and PhotoCraft can only start it on this computer (127.0.0.1)")));
    }
    Ok((base, port))
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::ComfyProcess;

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::sync::{Mutex, PoisonError};
    use std::time::Duration;

    /// How long a started ComfyUI may take to answer (custom nodes can make it slow).
    /// A PC with no graphics card (a virtual machine running it on the processor) took minutes
    /// to import PyTorch; one that crashes says so at once, so this only ends a hung start.
    const STARTUP_SECONDS: u64 = 600;

    /// What a Windows crash code means, for the ones a Python program that crashes shows.
    pub(super) fn crash_name(code: u32) -> Option<&'static str> {
        match code {
            0xC000_0005 => Some("a crash (access violation)"),
            0xC000_001D => Some("an instruction this processor does not have (a virtual machine can hide some)"),
            0xC000_00FD => Some("a stack overflow"),
            0xC000_0135 => Some("a DLL that was not found"),
            0xC000_0139 => Some("a DLL without a function it should have"),
            0xC000_0409 => Some("a crash (stack buffer overrun)"),
            _ => None,
        }
    }

    /// Python's crash dump (`faulthandler`, which ComfyUI switches on) in `lines`: its first line
    /// ("Windows fatal exception: access violation") and the innermost call that is not Python's
    /// own import machinery. The dump lists the most recent call first.
    pub(super) fn crash_summary(lines: &[&str]) -> Option<String> {
        let start = lines.iter().rposition(|l| l.starts_with("Windows fatal exception:") || l.starts_with("Fatal Python error:"))?;
        let fault = lines.get(start)?;
        let frame = lines.iter().skip(start.saturating_add(1)).find(|l| l.starts_with("File ") && !l.contains("<frozen "));
        Some(match frame {
            Some(frame) => format!("{fault}, in {frame}"),
            None => (*fault).to_string(),
        })
    }

    /// ComfyUI as a child process of the app.
    pub struct ComfyProcess {
        child: Mutex<Option<Child>>,
        /// Where the server's output goes, for the error message when it fails to start.
        log: Option<PathBuf>,
        /// How the child ended when it stopped on its own ([`LocalServer::stopped`]).
        exited: Mutex<Option<String>>,
    }

    impl ComfyProcess {
        pub fn new(log: Option<PathBuf>) -> Self {
            Self { child: Mutex::new(None), log, exited: Mutex::new(None) }
        }

        fn exited(&self) -> std::sync::MutexGuard<'_, Option<String>> {
            self.exited.lock().unwrap_or_else(PoisonError::into_inner)
        }

        fn child(&self) -> std::sync::MutexGuard<'_, Option<Child>> {
            self.child.lock().unwrap_or_else(PoisonError::into_inner)
        }

        /// The child if it is still running (forgets one that has exited, noting how it ended).
        fn alive(&self) -> bool {
            let mut guard = self.child();
            let status = match guard.as_mut().map(Child::try_wait) {
                None => return false,
                Some(Ok(None)) => return true,
                Some(Ok(Some(status))) => Some(status),
                Some(Err(_)) => None,
            };
            *guard = None;
            drop(guard);
            let ended = self.ended(status);
            *self.exited() = Some(ended);
            false
        }

        /// How the child ended: its exit code (in hex where Windows reports a crash as one, such
        /// as `0xC0000005`), then its last output and where the rest of it is.
        fn ended(&self, status: Option<std::process::ExitStatus>) -> String {
            let mut out = match status.and_then(|s| s.code()) {
                Some(code) if !(0..=0xFFFF).contains(&code) => match crash_name(code as u32) {
                    Some(name) => format!("exit code {:#010X}, {name}", code as u32),
                    None => format!("exit code {:#010X}", code as u32),
                },
                Some(code) => format!("exit code {code}"),
                None => "stopped by the system".to_string(),
            };
            let tail = self.log_tail();
            if !tail.is_empty() {
                out = format!("{out}; it last printed: {tail}");
            }
            if let Some(log) = &self.log {
                out = format!("{out} (all of its output: {})", log.display());
            }
            out
        }

        /// The end of what the server printed (its last 64 KB: a long run's log can be large),
        /// read while it keeps writing.
        fn recent_text(&self) -> Option<String> {
            use std::io::{Read, Seek, SeekFrom};
            const RECENT: u64 = 64 << 10;
            let mut file = std::fs::File::open(self.log.as_ref()?).ok()?;
            let len = file.metadata().ok()?.len();
            file.seek(SeekFrom::Start(len.saturating_sub(RECENT))).ok()?;
            let mut bytes = Vec::with_capacity(RECENT.min(len) as usize);
            file.take(RECENT).read_to_end(&mut bytes).ok()?;
            let text = String::from_utf8_lossy(&bytes).into_owned();
            // Started mid-line: that first piece is not a whole line.
            Some(if len > RECENT { text.split_once('\n').map_or(String::new(), |(_, rest)| rest.to_string()) } else { text })
        }

        /// The last lines the server printed, for an error message.
        fn log_tail(&self) -> String {
            let Some(text) = self.log.as_ref().and_then(|p| std::fs::read(p).ok()) else { return String::new() };
            let text = String::from_utf8_lossy(&text);
            let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
            // A crash dump's last lines are its outermost calls, which say nothing: its own
            // first line, and the call it crashed in, do.
            // A start that failed says why somewhere above its last lines.
            // Only near the end: an error from a render long before is not why it stopped now.
            let recent = lines.get(lines.len().saturating_sub(80)..).unwrap_or(&[]);
            let tail = crash_summary(&lines)
                .or_else(|| startup_cause(recent))
                .unwrap_or_else(|| lines.iter().rev().take(3).rev().copied().collect::<Vec<_>>().join(" | "));
            comfy::clip(&tail, 600)
        }

        fn spawn(&self, settings: &Settings, port: u16) -> Result<Child> {
            let launch = &settings.comfy_launch;
            let python = launch.python.trim();
            let folder = launch.folder.trim();
            let hint = format!("Start ComfyUI yourself, then open {WHERE} and press Find Installation to fill it in.");
            if python.is_empty() && folder.is_empty() {
                return Err(Error::Config(format!("Starting it is switched on, but its Python and folder are not set. {hint}")));
            }
            if python.is_empty() || !Path::new(python).is_file() {
                return Err(Error::Config(format!("Its Python was not found at `{python}`. {hint}")));
            }
            if folder.is_empty() || !Path::new(folder).is_dir() {
                return Err(Error::Config(format!("Its folder `{folder}` does not exist. {hint}")));
            }
            let mut args = without_port(split_args(&launch.args));
            if args.is_empty() {
                args.push("main.py".into());
            }
            // An older copy than the one ComfyUI Desktop runs would quit on its database.
            let script = args.iter().find(|a| a.to_ascii_lowercase().ends_with(".py")).cloned().unwrap_or_else(|| "main.py".into());
            let installed = desktop_config_dir().map(|d| d.join("installations.json")).and_then(|f| small_text(&f)).map(|r| desktop_folders(&r)).unwrap_or_default();
            let folder = newer_copy(Path::new(folder), &script, &installed, &small_text, &|p| p.is_file()).unwrap_or_else(|| PathBuf::from(folder));
            args.push("--port".into());
            args.push(port.to_string());
            let mut cmd = Command::new(python);
            cmd.current_dir(&folder).args(&args).stdin(Stdio::null());
            let output = |log: &Option<PathBuf>| {
                log.as_ref().and_then(|p| std::fs::OpenOptions::new().create(true).append(true).open(p).ok()).map_or_else(Stdio::null, Stdio::from)
            };
            if let Some(p) = &self.log {
                // Start each run with a fresh log: only its tail is ever read.
                let _ = std::fs::write(p, b"");
            }
            *self.exited() = None;
            cmd.stdout(output(&self.log)).stderr(output(&self.log));
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                // CREATE_NO_WINDOW: no console window flashes up behind the app.
                cmd.creation_flags(0x0800_0000);
            }
            cmd.spawn().map_err(|e| Error::Config(format!("Could not start ComfyUI (`{python}`): {e}")))
        }
    }

    /// End `child` and the processes it started (a virtual environment's `python.exe` on Windows
    /// is a launcher for the real interpreter, which would otherwise live on).
    fn kill_tree(child: &mut Child) {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let _ = Command::new("taskkill")
                .args(["/PID", &child.id().to_string(), "/T", "/F"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(0x0800_0000)
                .status();
        }
        let _ = child.kill();
        let _ = child.wait();
    }

    impl LocalServer for ComfyProcess {
        fn ensure(&self, transport: &dyn Transport, settings: &Settings, control: &Control<'_>) -> Result<()> {
            let base = comfy::base_url(&settings.comfy_url)?;
            if is_running(transport, &base) {
                return Ok(());
            }
            let (_, local) = port_of(&base);
            if !settings.comfy_autostart || !local {
                // Not ours to start: say so, and what would make it work.
                return Err(Error::Network(not_running_message(settings, &base)));
            }
            let (base, port) = local_base(settings)?;
            if !self.alive() {
                // Lead with what happened: the reason alone ("Python was not found") reads like
                // a broken install when ComfyUI simply wasn't open.
                let child = self.spawn(settings, port).map_err(|e| match e {
                    Error::Config(why) => Error::Config(format!("ComfyUI is not running at {base}, and PhotoCraft could not start it. {why}")),
                    e => e,
                })?;
                *self.child() = Some(child);
            }
            // Half-second polls, but the time is what counts: a poll that waits on a busy server
            // takes longer than half a second, and polls alone once ran a start for 30 minutes.
            let polls = STARTUP_SECONDS * 2;
            let began = std::time::Instant::now();
            for n in 0..polls {
                if (control.cancelled)() {
                    // A half-started server is no use to anyone.
                    self.stop();
                    return Err(Error::Cancelled);
                }
                if began.elapsed().as_secs() >= STARTUP_SECONDS {
                    break;
                }
                // What it is doing, from what it last printed, and for how long.
                let clock = crate::activity::clock(began.elapsed().as_secs());
                let words = match crate::activity::describe(&self.output(80)) {
                    Some(doing) => format!("Starting ComfyUI: {doing} · {clock}"),
                    None => format!("Starting ComfyUI · {clock}"),
                };
                (control.progress)(0.01 + 0.03 * (n as f32 / polls as f32), &words);
                if !self.alive() {
                    // How it ended: the exit code (what a crash was), its last words and where
                    // the rest of its output is.
                    let ended = self.exited().clone().unwrap_or_else(|| self.log_tail());
                    let why = if ended.is_empty() { String::new() } else { format!(": {ended}") };
                    return Err(Error::Server(format!("ComfyUI stopped while starting{why}")));
                }
                if is_running(transport, &base) {
                    return Ok(());
                }
                (control.sleep)(Duration::from_millis(500));
            }
            self.stop();
            Err(Error::Server(format!("ComfyUI did not answer at {base} within {STARTUP_SECONDS} seconds of starting")))
        }

        fn stop(&self) -> bool {
            *self.exited() = None;
            let child = self.child().take();
            match child {
                Some(mut c) => {
                    kill_tree(&mut c);
                    true
                }
                None => false,
            }
        }

        fn state(&self) -> ServerState {
            if self.alive() { ServerState::Started } else { ServerState::NotStarted }
        }

        fn stopped(&self) -> Option<String> {
            self.alive();
            self.exited().clone()
        }

        fn output(&self, lines: usize) -> Vec<String> {
            let all = self.recent_text().map(|text| crate::activity::lines(&text)).unwrap_or_default();
            let skip = all.len().saturating_sub(lines.min(500));
            all.into_iter().skip(skip).collect()
        }

        fn log_file(&self) -> Option<String> {
            self.log.as_ref().map(|p| p.display().to_string())
        }

        fn find(&self, transport: &dyn Transport, url: &str) -> Result<Launch> {
            let base = comfy::base_url(url)?;
            // The running server knows exactly how it was started; without one, ComfyUI Desktop's
            // record of what it installed is the next best thing.
            if is_running(transport, &base) {
                return find_running(transport, url);
            }
            find_installed().ok_or_else(|| {
                Error::Config(format!(
                    "ComfyUI is not running at {base}, and no ComfyUI Desktop installation was found on this computer. Use Browse to choose its python.exe and main.py, or start ComfyUI and press Find Installation again."
                ))
            })
        }
    }

    impl Drop for ComfyProcess {
        fn drop(&mut self) {
            self.stop();
        }
    }

    /// ComfyUI Desktop's settings folder.
    fn desktop_config_dir() -> Option<PathBuf> {
        let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        let dir = if cfg!(windows) {
            env("APPDATA")
        } else if cfg!(target_os = "macos") {
            env("HOME").map(|h| h.join("Library/Application Support"))
        } else {
            env("XDG_CONFIG_HOME").or_else(|| env("HOME").map(|h| h.join(".config")))
        };
        dir.map(|d| d.join("Comfy Desktop"))
    }

    /// The launch settings of the ComfyUI that ComfyUI Desktop installed on this computer.
    pub fn find_installed() -> Option<Launch> {
        let config = desktop_config_dir()?;
        let file = config.join("installations.json");
        // A settings file, not a database: anything huge is not it.
        if std::fs::metadata(&file).ok()?.len() > 4 * 1024 * 1024 {
            return None;
        }
        desktop_install(&std::fs::read_to_string(file).ok()?, &config, &|p| p.is_file())
    }

    /// A small text file's contents (a settings or version file; anything over a megabyte is not one).
    fn small_text(p: &Path) -> Option<String> {
        std::fs::metadata(p).ok().filter(|m| m.is_file() && m.len() <= 1 << 20).and_then(|_| std::fs::read_to_string(p).ok())
    }

    /// [`models_folders`] on this computer's disk.
    pub fn models_folders_here(launch: &Launch) -> Vec<String> {
        // Settings files, not databases: anything big is not one.
        let read = |p: &Path| std::fs::metadata(p).ok().filter(|m| m.is_file() && m.len() <= 1 << 20).and_then(|_| std::fs::read_to_string(p).ok());
        models_folders(launch, &read, &|p| p.is_dir())
    }

    /// A running process: its executable, working folder (where the OS tells), command line and
    /// parent.
    #[derive(Clone, Debug, Default)]
    struct Proc {
        pid: u32,
        parent: u32,
        exe: String,
        cwd: String,
        cmd: String,
    }

    /// The Python processes on this machine.
    #[cfg(windows)]
    fn python_processes() -> Vec<Proc> {
        use std::os::windows::process::CommandExt;
        // One line per process, tab-separated. Parents are included for the folder heuristic.
        let script = "Get-CimInstance Win32_Process | ForEach-Object { \"$($_.ProcessId)`t$($_.ParentProcessId)`t$($_.ExecutablePath)`t$($_.CommandLine)\" }";
        let out = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x0800_0000)
            .output();
        let Ok(out) = out else { return Vec::new() };
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| {
                let mut f = l.splitn(4, '\t');
                Some(Proc {
                    pid: f.next()?.trim().parse().ok()?,
                    parent: f.next()?.trim().parse().ok()?,
                    exe: f.next()?.to_string(),
                    cwd: String::new(),
                    cmd: f.next().unwrap_or("").to_string(),
                })
            })
            .take(20_000)
            .collect()
    }

    /// The processes on this machine (Linux: `/proc`; elsewhere none, and the user fills the
    /// settings in by hand).
    #[cfg(not(windows))]
    fn python_processes() -> Vec<Proc> {
        let Ok(dir) = std::fs::read_dir("/proc") else { return Vec::new() };
        dir.flatten()
            .filter_map(|e| {
                let pid: u32 = e.file_name().to_str()?.parse().ok()?;
                let base = e.path();
                let cmd = std::fs::read(base.join("cmdline")).ok()?;
                let args: Vec<String> = cmd.split(|b| *b == 0).filter(|a| !a.is_empty()).map(|a| String::from_utf8_lossy(a).into_owned()).collect();
                let stat = std::fs::read_to_string(base.join("stat")).unwrap_or_default();
                // `pid (name) state ppid …`: the name may hold spaces, so read after the last `)`.
                let parent = stat.rsplit_once(')').and_then(|(_, r)| r.split_whitespace().nth(1)).and_then(|p| p.parse().ok()).unwrap_or(0);
                let link = |n: &str| std::fs::read_link(base.join(n)).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
                Some(Proc { pid, parent, exe: link("exe"), cwd: link("cwd"), cmd: join_args(&args) })
            })
            .take(20_000)
            .collect()
    }

    fn is_python(p: &Proc) -> bool {
        Path::new(&p.exe).file_stem().and_then(|s| s.to_str()).is_some_and(|s| s.to_ascii_lowercase().starts_with("python"))
    }

    /// The value after `flag` in `argv`.
    fn arg_after<'a>(argv: &'a [String], flag: &str) -> Option<&'a str> {
        argv.iter().position(|a| a == flag).and_then(|i| argv.get(i + 1)).map(String::as_str)
    }

    /// The launch settings of the ComfyUI running at `url`: its own report of its arguments, and
    /// from the process list the Python it runs in and the folder it was started from.
    pub fn find_running(transport: &dyn Transport, url: &str) -> Result<Launch> {
        let base = comfy::base_url(url)?;
        let (port, local) = port_of(&base);
        let argv = server_argv(transport, &base)
            .ok_or_else(|| Error::Network(format!("ComfyUI is not running at {base}. Start it once the way you usually do, then try again")))?;
        if !local {
            return Err(Error::Config(format!("ComfyUI at {base} runs on another computer; PhotoCraft can only start one on this computer")));
        }
        let script = argv.first().cloned().unwrap_or_else(|| "main.py".into());
        let args = join_args(&without_port(argv.clone()));
        let all = python_processes();
        let port_text = port.to_string();
        // The server's process: a Python whose command line names the script and the port.
        let script_name = Path::new(&script).file_name().and_then(|s| s.to_str()).unwrap_or("main.py").to_string();
        let mut servers: Vec<&Proc> =
            all.iter().filter(|p| is_python(p) && p.cmd.contains(&script_name) && (p.cmd.contains(&format!("--port {port_text}")) || port == 8188)).collect();
        // A virtual environment's python starts the real interpreter as its child: the outer one
        // is the one to run again.
        let pids: Vec<u32> = servers.iter().map(|p| p.pid).collect();
        servers.sort_by_key(|p| pids.contains(&p.parent));
        let Some(server) = servers.first() else {
            return Ok(Launch { python: String::new(), folder: String::new(), args });
        };
        // The folder: the OS's answer where it gives one, else the first candidate that holds
        // the script: the script's own folder, the starting app's `resources` (ComfyUI Desktop),
        // the base directory, the virtual environment's project.
        let parent_exe = all.iter().find(|p| p.pid == server.parent).map(|p| PathBuf::from(&p.exe));
        let venv_root = Path::new(&server.exe).ancestors().nth(3).map(Path::to_path_buf);
        let mut candidates: Vec<PathBuf> = Vec::new();
        if !server.cwd.is_empty() {
            candidates.push(PathBuf::from(&server.cwd));
        }
        if Path::new(&script).is_absolute() {
            candidates.extend(Path::new(&script).parent().map(Path::to_path_buf));
        }
        // ComfyUI Desktop's own record of where its installations are (it starts the one it runs
        // from there), before the folders around the app, which may hold an older copy.
        if let Some(record) = desktop_config_dir().map(|d| d.join("installations.json")).and_then(|f| small_text(&f)) {
            candidates.extend(desktop_folders(&record));
        }
        if let Some(exe) = &parent_exe {
            candidates.extend(exe.ancestors().skip(1).take(3).flat_map(|d| [d.join("resources"), d.to_path_buf()]));
        }
        candidates.extend(arg_after(&argv, "--base-directory").map(PathBuf::from));
        candidates.extend(venv_root);
        // The copy that is the version the server says it is: two copies of ComfyUI can share a
        // user folder, and starting the older one fails on the newer one's database.
        let version = server_version(transport, &base);
        let folder = pick_folder(&candidates, &script, version.as_deref(), &small_text, &|p| p.is_file());
        Ok(Launch { python: server.exe.clone(), folder: folder.map(|d| d.to_string_lossy().into_owned()).unwrap_or_default(), args })
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::{find_installed, find_running, models_folders_here};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::Response;

    #[test]
    fn ports_and_hosts() {
        assert_eq!(port_of("http://127.0.0.1:8000"), (8000, true));
        assert_eq!(port_of("http://localhost"), (8188, true));
        assert_eq!(port_of("http://[::1]:9000/"), (9000, true));
        assert_eq!(port_of("http://[::1]"), (8188, true));
        assert_eq!(port_of("https://comfy.example.com:443/api"), (443, false));
        assert_eq!(port_of("http://192.168.1.20:8188"), (8188, false));
        for odd in ["", "http://", "http://:0", "http://127.0.0.1:99999", "http://127.0.0.1:abc", "::::"] {
            assert_eq!(port_of(odd).0, 8188, "{odd}");
        }
    }

    #[test]
    fn addresses_try_the_other_local_port() {
        assert_eq!(addresses("http://127.0.0.1:8188/"), ["http://127.0.0.1:8188", "http://127.0.0.1:8000"]);
        assert_eq!(addresses(" http://localhost:8000"), ["http://localhost:8000", "http://localhost:8188"]);
        assert_eq!(addresses("http://localhost"), ["http://localhost", "http://localhost:8000"]);
        assert_eq!(addresses("http://[::1]:9000"), ["http://[::1]:9000", "http://[::1]:8000", "http://[::1]:8188"]);
        assert_eq!(addresses("http://192.168.1.20:8188"), ["http://192.168.1.20:8188"]);
        for odd in ["", "http://", "::::", "127.0.0.1"] {
            assert!(!addresses(odd).is_empty(), "{odd}");
        }
    }

    #[test]
    fn command_lines_round_trip() {
        let args = split_args(r#"ComfyUI\main.py --base-directory "C:\My Docs\ComfyUI" --enable-manager  --x """#);
        assert_eq!(args, vec!["ComfyUI\\main.py", "--base-directory", "C:\\My Docs\\ComfyUI", "--enable-manager", "--x", ""]);
        assert_eq!(split_args(&join_args(&args)), args);
        assert!(split_args("   ").is_empty());
        assert_eq!(split_args("\"unterminated quote"), vec!["unterminated quote"]);
        let v = |x: &[&str]| x.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(without_port(v(&["main.py", "--port", "8000", "--a", "--port=1", "--port"])), v(&["main.py", "--a"]));
    }

    #[test]
    fn browse_fills_in_what_it_can() {
        use std::path::Path;
        // Paths are joined with the platform's separator: compare them with forward slashes.
        let from_picked = |path: &str, current: &Launch, exists: &dyn Fn(&Path) -> bool| {
            let l = super::from_picked(path, current, exists);
            Launch { python: l.python.replace('\\', "/"), folder: l.folder.replace('\\', "/"), args: l.args }
        };
        let on_disk = |files: &'static [&'static str]| move |p: &Path| files.iter().any(|f| Path::new(f) == p);
        // main.py of a git clone with its own virtual environment: everything follows.
        let files = on_disk(&["/home/u/ComfyUI/main.py", "/home/u/ComfyUI/.venv/bin/python"]);
        let l = from_picked("/home/u/ComfyUI/main.py", &Launch::default(), &files);
        assert_eq!(l, Launch { python: "/home/u/ComfyUI/.venv/bin/python".into(), folder: "/home/u/ComfyUI".into(), args: "main.py".into() });
        // The portable build: the embedded Python one level up, and its flag.
        let files = on_disk(&["/p/ComfyUI/main.py", "/p/python_embeded/python.exe"]);
        let l = from_picked("/p/ComfyUI/main.py", &Launch::default(), &files);
        assert_eq!((l.python.as_str(), l.args.as_str()), ("/p/python_embeded/python.exe", "main.py --windows-standalone-build"));
        // Picking the Python finds the folder beside it; existing choices are kept.
        let l = from_picked("/p/python_embeded/python.exe", &Launch::default(), &files);
        assert_eq!((l.folder.as_str(), l.args.as_str()), ("/p/ComfyUI", "main.py"));
        let mine = Launch { python: "/my/python".into(), folder: "/old".into(), args: "main.py --lowvram".into() };
        let l = from_picked("/p/ComfyUI/main.py", &mine, &files);
        assert_eq!(l, Launch { python: "/my/python".into(), folder: "/p/ComfyUI".into(), args: "main.py --lowvram".into() });
        let l = from_picked("/other/python3", &mine, &files);
        assert_eq!(l, Launch { python: "/other/python3".into(), ..mine.clone() });
        // Arguments without a script get the picked one in front.
        let l = from_picked("/p/ComfyUI/main.py", &Launch { args: "--lowvram".into(), ..Launch::default() }, &on_disk(&[]));
        assert_eq!((l.python.as_str(), l.args.as_str()), ("", "main.py --lowvram"));
        // Nothing picked, or nothing recognisable: unchanged.
        assert_eq!(from_picked("", &mine, &files), mine);
        assert_eq!(from_picked("   ", &mine, &files), mine);
    }

    #[test]
    fn comfyui_desktops_record_gives_the_launch_settings() {
        use std::path::Path;
        let record = r#"[
          {"id":"cloud-1","name":"Comfy Cloud","sourceId":"cloud","remoteUrl":"https://cloud.comfy.org/"},
          {"id":"old","installPath":"/installs/Old","adoptedPythonPath":"/gone/python","lastLaunchedAt":5},
          {"id":"inst-1","installPath":"/installs/ComfyUI","adoptedPythonPath":"/docs/ComfyUI/.venv/bin/python","adoptedBaseDir":"/docs/ComfyUI",
           "launchArgs":"--port 8000 --enable-manager","inputDir":"/docs/ComfyUI/input","outputDir":"/docs/My Output","lastLaunchedAt":9}
        ]"#;
        let files = ["/installs/ComfyUI/ComfyUI/main.py", "/docs/ComfyUI/.venv/bin/python", "/cfg/instance-model-paths/inst-1.yaml", "/installs/Old/main.py"];
        let exists = |p: &Path| files.iter().any(|f| Path::new(f) == p);
        let l = desktop_install(record, Path::new("/cfg"), &exists).unwrap();
        assert_eq!(l.python, "/docs/ComfyUI/.venv/bin/python");
        assert_eq!(l.folder, "/installs/ComfyUI");
        let args = split_args(&l.args);
        assert_eq!(args[0], format!("ComfyUI{}main.py", std::path::MAIN_SEPARATOR));
        for pair in [["--base-directory", "/docs/ComfyUI"], ["--output-directory", "/docs/My Output"], ["--input-directory", "/docs/ComfyUI/input"]] {
            assert!(args.windows(2).any(|w| w == pair), "{pair:?} in {args:?}");
        }
        assert!(
            args.iter().any(|a| a.ends_with("inst-1.yaml")) && args.contains(&"--enable-manager".to_string()) && !args.contains(&"--port".to_string()),
            "{args:?}"
        );
        // Nothing usable: no Python on disk, no script, or not Desktop's file at all.
        assert_eq!(desktop_install(record, Path::new("/cfg"), &|_| false), None);
        for junk in ["", "{}", "[]", "[1, null, {\"installPath\": 7}]", "not json", "[{\"installPath\":\"/x\"}]"] {
            assert_eq!(desktop_install(junk, Path::new("/cfg"), &exists), None, "{junk}");
        }
        // An id that isn't a plain name can't reach outside the settings folder.
        let sneaky = r#"[{"id":"../../etc/passwd","installPath":"/installs/ComfyUI","adoptedPythonPath":"/docs/ComfyUI/.venv/bin/python"}]"#;
        assert!(!desktop_install(sneaky, Path::new("/cfg"), &|_| true).unwrap().args.contains("passwd"));
        // Its installations, the one launched last first: where a ComfyUI it runs is started from.
        assert_eq!(desktop_folders(record), vec![std::path::PathBuf::from("/installs/ComfyUI"), "/installs/Old".into()]);
        assert!(desktop_folders("not json").is_empty() && desktop_folders("[{\"installPath\":7}]").is_empty());
    }

    /// The user's machine, 2026-10-20: Comfy Desktop ran ComfyUI 0.42.0 from its installation,
    /// started as `ComfyUI\main.py`; an old 0.22.3 copy sat in the folder above the app. Find
    /// Installation took the old copy, which then failed on the newer one's database.
    #[test]
    fn the_running_servers_own_copy_is_chosen_not_an_older_one_beside_it() {
        use std::path::{Path, PathBuf};
        let script = "ComfyUI/main.py";
        let versions = [("/installs/ComfyUI/ComfyUI/comfyui_version.py", "0.42.0"), ("/desktop/resources/ComfyUI/comfyui_version.py", "0.22.3")];
        let read = |p: &Path| versions.iter().find(|(f, _)| Path::new(f) == p).map(|(_, v)| format!("# updated in pyproject.toml.\n__version__ = \"{v}\"\n"));
        let is_file = |p: &Path| ["/installs/ComfyUI/ComfyUI/main.py", "/desktop/resources/ComfyUI/main.py"].iter().any(|f| Path::new(f) == p);
        let around_the_app_first = [PathBuf::from("/desktop/resources"), "/installs/ComfyUI".into()];
        assert_eq!(pick_folder(&around_the_app_first, script, Some("0.42.0"), &read, &is_file), Some("/installs/ComfyUI".into()));
        // A server that doesn't say its version: the first that holds the script, as before.
        assert_eq!(pick_folder(&around_the_app_first, script, None, &read, &is_file), Some("/desktop/resources".into()));
        // None of its version: none, rather than the wrong one.
        assert_eq!(pick_folder(&around_the_app_first, script, Some("0.50.0"), &read, &is_file), None);
        assert_eq!((version_in("__version__ = '1.2'"), version_in("nothing"), version_in("__version__ = \"\"")), (Some("1.2".into()), None, None));
    }

    /// The user's machine, 2026-10-22: settings saved by the older Find Installation still named
    /// the 0.22.3 copy, and every start quit on the 0.42.0 one's database. The newer copy that
    /// Comfy Desktop records is started instead; never an older one or one without the script.
    #[test]
    fn a_saved_older_copy_starts_the_newer_installation_instead() {
        use std::path::{Path, PathBuf};
        let script = "ComfyUI/main.py";
        let versions = [
            ("/installs/ComfyUI/ComfyUI/comfyui_version.py", "0.42.0"),
            ("/installs/Older/ComfyUI/comfyui_version.py", "0.9.1"),
            ("/desktop/resources/ComfyUI/comfyui_version.py", "0.22.3"),
        ];
        let read = |p: &Path| versions.iter().find(|(f, _)| Path::new(f) == p).map(|(_, v)| format!("__version__ = \"{v}\"\n"));
        let is_file = |p: &Path| ["/installs/ComfyUI/ComfyUI/main.py", "/installs/Older/ComfyUI/main.py", "/desktop/resources/ComfyUI/main.py"].iter().any(|f| Path::new(f) == p);
        let stale = Path::new("/desktop/resources");
        let installed = [PathBuf::from("/installs/Older"), "/installs/ComfyUI".into()];
        assert_eq!(newer_copy(stale, script, &installed, &read, &is_file), Some("/installs/ComfyUI".into()));
        // The newest copy is kept, and so is a folder whose version is unknown.
        assert_eq!(newer_copy(Path::new("/installs/ComfyUI"), script, &[stale.into(), "/installs/Older".into()], &read, &is_file), None);
        assert_eq!(newer_copy(Path::new("/elsewhere"), script, &installed, &read, &is_file), None);
        // Without the same script it isn't a copy of the same thing.
        assert_eq!(newer_copy(stale, "main.py", &installed, &read, &is_file), None);
        assert_eq!(newer_copy(stale, script, &[], &read, &is_file), None);
        assert!(version_parts("0.42.1+9") > version_parts("0.42.0") && version_parts("0.10") > version_parts("0.9.9"));
        assert!(version_parts("") == version_parts("x"));
    }

    /// What that old copy printed before it quit: the cause is ten lines above the end.
    #[test]
    fn a_failed_start_names_its_cause_not_its_last_chatter() {
        let log = "Import times for custom nodes:\n   2.4 seconds: C:\\x\\comfyui-easy-use\nError upgrading database: \nTraceback (most recent call last):\n  File \"C:\\a.py\", line 213, in f\n\
                   alembic.script.revision.ResolutionError: No such revision or branch '0009_created_at_from_mtime'\n\
                   alembic.util.exc.CommandError: Can't locate revision identified by '0009_created_at_from_mtime'\n\
                   Failed to initialize database: Can't locate revision identified by '0009_created_at_from_mtime'\n\
                   The --enable-assets flag requires a working database connection.\nTo resolve this, try one of the following:\n\
                   Cannot connect to comfyregistry.\nFETCH DATA from: C:\\cache.json [DONE]\n[ComfyUI-Manager] All startup tasks have been completed.";
        let lines: Vec<&str> = log.lines().collect();
        let cause = startup_cause(&lines).unwrap();
        assert!(
            cause.starts_with("Failed to initialize database: Can't locate revision identified by '0009_created_at_from_mtime' | The --enable-assets flag requires"),
            "{cause}"
        );
        assert!(cause.contains("older than one that has used the same user folder") && cause.contains("Find Installation"), "{cause}");
        // An exception's own line, and argparse's complaint, name a cause too; chatter does not.
        assert_eq!(startup_cause(&["x", "ModuleNotFoundError: No module named 'yaml'"]).as_deref(), Some("ModuleNotFoundError: No module named 'yaml'"));
        assert!(startup_cause(&["main.py: error: unrecognized arguments: --bogus"]).is_some_and(|c| c.contains("unrecognized")));
        assert_eq!(startup_cause(&["Starting server", "To see the GUI go to: http://127.0.0.1:8188", "Some text: with a colon"]), None);
        assert_eq!(startup_cause(&[]), None);
    }

    #[test]
    fn the_models_folders_are_found_as_comfyui_finds_them() {
        use std::path::Path;
        let files = |list: &'static [(&'static str, &'static str)]| move |p: &Path| list.iter().find(|(f, _)| Path::new(f) == p).map(|(_, text)| text.to_string());
        let dirs = |list: &'static [&'static str]| move |p: &Path| list.iter().any(|d| Path::new(d) == p);
        let slashes = |v: Vec<String>| v.into_iter().map(|p| p.replace('\\', "/")).collect::<Vec<_>>();
        // ComfyUI Desktop: the shared folder it saves new models to, then its own base folder's.
        const SHARED: &str = "# Generated by Comfy Desktop\n\ncomfy.desktop_0:\n  base_path: '/shared/models'\n  is_default: true\n  'checkpoints': 'checkpoints/'\n  'controlnet': |-\n    controlnet/\n    t2i_adapter/\n  'diffusion_models': 'diffusion_models/'\n\ncomfy.desktop_1:\n  base_path: '/docs/ComfyUI/models'\n  'diffusion_models': 'diffusion_models/'\n";
        let desktop = Launch {
            python: "/docs/ComfyUI/.venv/bin/python".into(),
            folder: "/installs/ComfyUI".into(),
            args: "ComfyUI/main.py --base-directory /docs/ComfyUI --extra-model-paths-config /cfg/inst-1.yaml --enable-manager".into(),
        };
        let read = files(&[("/cfg/inst-1.yaml", SHARED)]);
        assert_eq!(slashes(models_folders(&desktop, &read, &dirs(&["/shared/models", "/docs/ComfyUI/models"]))), ["/shared/models", "/docs/ComfyUI/models"]);
        // Only folders that are there.
        assert_eq!(slashes(models_folders(&desktop, &read, &dirs(&["/docs/ComfyUI/models"]))), ["/docs/ComfyUI/models"]);
        // The portable build: models beside its script, then the file beside the script, written
        // the older way (`base_path` is a ComfyUI folder, the entries under `models/`). A folder of
        // one kind of file only is not a models folder.
        const BESIDE: &str = "comfyui:\n    base_path: /other/ComfyUI\n    checkpoints: models/checkpoints/\n    diffusion_models: |\n      models/diffusion_models\n      models/unet\n    is_default: false # not this one\ncustom:\n    diffusion_models: /loose/unets\n";
        let portable = Launch { python: "/p/python_embeded/python.exe".into(), folder: "/p".into(), args: "ComfyUI/main.py --windows-standalone-build".into() };
        let read = files(&[("/p/ComfyUI/extra_model_paths.yaml", BESIDE)]);
        assert_eq!(slashes(models_folders(&portable, &read, &dirs(&["/p/ComfyUI/models", "/other/ComfyUI/models", "/loose"]))), [
            "/p/ComfyUI/models",
            "/other/ComfyUI/models"
        ]);
        // `--models-directory`; files relative to where it starts, two after one flag; `is_default`
        // puts a folder first, and moves one already listed there.
        const ONE: &str = "a:\n  base_path: shared\n  diffusion_models: diffusion_models\nb:\n  base_path: \"/x/models\"\n  is_default: \"true\"\n  diffusion_models: diffusion_models/ # the big ones\n";
        const TWO: &str = "again:\n  base_path: /e/shared\n  is_default: yes\n  vae: vae\n";
        let own = Launch { folder: "/e".into(), args: "main.py --models-directory /m --extra-model-paths-config one.yaml two.yaml".into(), ..Launch::default() };
        let read = files(&[("/e/one.yaml", ONE), ("/e/two.yaml", TWO)]);
        assert_eq!(slashes(models_folders(&own, &read, &dirs(&["/m", "/e/shared", "/x/models"]))), ["/e/shared", "/x/models", "/m"]);
        // Nothing to go on.
        assert!(models_folders(&Launch::default(), &read, &|_| true).is_empty());
        for junk in ["", ":", "a:\n  b", "x:\n  diffusion_models: |", "'unterminated: x", "\"", "  base_path: /x", "a:\n  'k\n", "a:\n  diffusion_models: '/x/diffusion_models"] {
            let _ = extra_model_roots(junk, Path::new("/"));
        }
        // Values as YAML writes them.
        assert_eq!(yaml_scalar(r#""C:\\Users\\a" # c"#), r"C:\Users\a");
        assert_eq!(yaml_scalar("'it''s' # c"), "it's");
        assert_eq!(yaml_scalar(r"C:\models # c"), r"C:\models");
        assert_eq!(yaml_pair(r"base_path: C:\Users\a"), Some(("base_path".to_string(), r"C:\Users\a")));
        assert_eq!(yaml_pair("'diffusion_models': 'x/'"), Some(("diffusion_models".to_string(), "'x/'")));
        assert_eq!(yaml_pair(r"C:\no-key-here"), None);
    }

    #[test]
    fn a_running_comfyui_says_where_its_models_are() {
        let body = r#"[{"name":"checkpoints","folders":["C:\\Shared\\models\\checkpoints","C:\\Docs\\models\\checkpoints"]},
            {"name":"diffusion_models","folders":["C:\\Shared\\models\\diffusion_models","C:\\Docs\\models\\unet","c:\\shared\\models\\diffusion_models\\","C:\\Docs\\models\\diffusion_models","D:\\loose"]}]"#;
        assert_eq!(served_models_folders(&Fake(Some(body)), "http://127.0.0.1:8000"), ["C:\\Shared\\models", "C:\\Docs\\models"]);
        // Without diffusion models listed, the checkpoints say it.
        let older = r#"[{"name":"checkpoints","folders":["/home/u/ComfyUI/models/checkpoints"]}]"#;
        assert_eq!(served_models_folders(&Fake(Some(older)), "http://127.0.0.1:8188"), ["/home/u/ComfyUI/models"]);
        let odd = r#"[{"name":"diffusion_models","folders":["diffusion_models","/diffusion_models",7]}]"#;
        for junk in [None, Some("{}"), Some("[]"), Some("[1]"), Some(r#"[{"name":"diffusion_models","folders":"x"}]"#), Some(odd), Some("<html>")] {
            assert!(served_models_folders(&Fake(junk), "http://127.0.0.1:8000").is_empty(), "{junk:?}");
        }
        assert!(served_models_folders(&Fake(Some(body)), "not a url").is_empty());
    }

    struct Fake(Option<&'static str>);
    impl Transport for Fake {
        fn send(&self, _: &Request) -> Result<Response> {
            match self.0 {
                Some(body) => Ok(Response { status: 200, body: body.as_bytes().to_vec() }),
                None => Err(Error::Network("refused".into())),
            }
        }
    }

    #[test]
    fn only_a_comfyui_counts_as_running() {
        assert!(is_running(&Fake(Some(r#"{"system":{"os":"win32","argv":["main.py","--port","8000"]}}"#)), "http://127.0.0.1:8000"));
        assert!(is_running(&Fake(Some(r#"{"system":{}}"#)), "http://127.0.0.1:8000"));
        for other in [None, Some("<html>some other app</html>"), Some("{}"), Some(r#"{"system":7}"#), Some("[]")] {
            assert!(!is_running(&Fake(other), "http://127.0.0.1:8000"), "{other:?}");
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    mod process {
        use super::*;
        use std::sync::atomic::{AtomicU32, Ordering};

        fn control<'a>(cancelled: &'a (dyn Fn() -> bool + Sync)) -> Control<'a> {
            Control { cancelled, progress: &|_, _| {}, sleep: &|_| {} }
        }

        fn settings(autostart: bool) -> Settings {
            Settings { comfy_url: "http://127.0.0.1:8000".into(), comfy_autostart: autostart, ..Settings::default() }
        }

        /// What Find Installation saves for the ComfyUI running at `PHOTOCRAFT_COMFY_URL`
        /// (default ComfyUI Desktop's address): `cargo test -p photocraft-aigen --lib
        /// this_machines_running -- --ignored --nocapture`. Changes nothing.
        #[test]
        #[ignore = "looks at the ComfyUI running on this machine"]
        fn this_machines_running_comfyui_is_found() {
            let url = std::env::var("PHOTOCRAFT_COMFY_URL").unwrap_or_else(|_| "http://127.0.0.1:8000".into());
            let transport = crate::UreqTransport::new();
            println!("server version: {:?}", server_version(&transport, &url));
            let found = find_running(&transport, &url).unwrap();
            println!("python: {}\nfolder: {}\nargs:   {}", found.python, found.folder, found.args);
            let script = split_args(&found.args).into_iter().next().unwrap_or_default();
            let version = std::fs::read_to_string(std::path::Path::new(&found.folder).join(&script).with_file_name("comfyui_version.py")).ok().and_then(|t| version_in(&t));
            println!("its copy's version: {version:?}");
            assert!(!found.folder.is_empty() && version == server_version(&transport, &url), "{found:?}");
        }

        #[test]
        fn a_running_server_is_used_and_never_stopped() {
            let server = ComfyProcess::new(None);
            let running = Fake(Some(r#"{"system":{"argv":["main.py"]}}"#));
            // Even with nothing configured to start: there is nothing to start.
            server.ensure(&running, &settings(true), &control(&|| false)).unwrap();
            assert_eq!(server.state(), ServerState::NotStarted);
            assert!(!server.stop(), "it was not ours");
        }

        #[test]
        fn without_autostart_nothing_is_started_and_the_error_says_what_to_do() {
            let server = ComfyProcess::new(None);
            // Not set up at all: start it yourself, and how to set it up.
            let mut s = settings(false);
            let e = server.ensure(&Fake(None), &s, &control(&|| false)).unwrap_err().to_string();
            assert!(e.contains("not running at http://127.0.0.1:8000") && e.contains("not set up to start it") && e.contains("Find Installation"), "{e}");
            // Set up but switched off: name the switch.
            s.comfy_launch = Launch { python: "C:/comfy/python.exe".into(), folder: "C:/comfy".into(), args: "main.py".into() };
            let e = server.ensure(&Fake(None), &s, &control(&|| false)).unwrap_err().to_string();
            assert!(e.contains("switched off") && e.contains("Start ComfyUI when a local model needs it") && !e.contains("Find Installation"), "{e}");
            // Another computer: nothing here can start it, whatever the switch says.
            for autostart in [false, true] {
                let mut remote = settings(autostart);
                remote.comfy_url = "http://192.168.1.50:8188".into();
                let e = server.ensure(&Fake(None), &remote, &control(&|| false)).unwrap_err().to_string();
                assert!(e.contains("another computer"), "{e}");
            }
            assert_eq!(server.state(), ServerState::NotStarted);
        }

        #[test]
        fn starting_needs_a_local_address_and_a_real_python() {
            let server = ComfyProcess::new(None);
            let mut s = settings(true);
            // Switched on with nothing filled in: the whole story in one message.
            let e = server.ensure(&Fake(None), &s, &control(&|| false)).unwrap_err().to_string();
            assert!(
                e.contains("is not running at http://127.0.0.1:8000")
                    && e.contains("could not start it")
                    && e.contains("not set")
                    && e.contains("Find Installation"),
                "{e}"
            );
            s.comfy_launch = Launch { python: "/no/such/python".into(), folder: "/no/such/folder".into(), args: "main.py".into() };
            let e = server.ensure(&Fake(None), &s, &control(&|| false)).unwrap_err().to_string();
            assert!(e.contains("could not start it") && e.contains("Python was not found at `/no/such/python`"), "{e}");
            s.comfy_url = "not a url".into();
            assert!(server.ensure(&Fake(None), &s, &control(&|| false)).is_err());
            assert_eq!(server.state(), ServerState::NotStarted);
        }

        /// A program that exists everywhere the tests run, standing in for Python: it starts,
        /// never serves the port, and exits (or is killed).
        fn stand_in() -> Launch {
            let exe = std::env::current_exe().unwrap();
            Launch { python: exe.to_string_lossy().into_owned(), folder: exe.parent().unwrap().to_string_lossy().into_owned(), args: "--list".into() }
        }

        #[test]
        fn a_server_that_exits_while_starting_is_reported() {
            let log = std::env::temp_dir().join(format!("photocraft-comfy-test-{}.log", std::process::id()));
            let server = ComfyProcess::new(Some(log.clone()));
            let mut s = settings(true);
            s.comfy_launch = stand_in();
            // The stand-in prints its test list and exits; real time passes while it does.
            let c = Control { cancelled: &|| false, progress: &|_, _| {}, sleep: &|d| std::thread::sleep(d.min(std::time::Duration::from_millis(50))) };
            let e = server.ensure(&Fake(None), &s, &c).unwrap_err().to_string();
            // The error itself says how it ended and where the whole output is.
            assert!(e.contains("stopped while starting: exit code ") && e.contains(&log.display().to_string()), "{e}");
            assert_eq!(server.state(), ServerState::NotStarted);
            // How it ended stays known, for a render that loses it: the code, what it printed
            // last, and where the rest is.
            let why = server.stopped().unwrap();
            assert!(why.starts_with("exit code ") && why.contains("; it last printed: ") && why.contains(&log.display().to_string()), "{why}");
            // Stopping on purpose is not a crash.
            server.stop();
            assert_eq!(server.stopped(), None);
            let _ = std::fs::remove_file(log);
        }

        #[test]
        fn what_the_server_printed_is_read_from_its_end() {
            let log = std::env::temp_dir().join(format!("photocraft-comfy-output-{}.log", std::process::id()));
            let server = ComfyProcess::new(Some(log.clone()));
            assert!(server.output(10).is_empty(), "no log yet");
            std::fs::write(&log, "\u{1b}[32m[INFO]\u{1b}[0m Total VRAM 0 MB\nChecking\r  5%|5    | 1/20 [00:05<01:40]\r 10%|#    | 2/20 [00:10<01:30]\n\n").unwrap();
            assert_eq!(server.output(10), ["[INFO] Total VRAM 0 MB", " 10%|#    | 2/20 [00:10<01:30]"]);
            assert_eq!(server.output(1), [" 10%|#    | 2/20 [00:10<01:30]"]);
            assert_eq!(server.log_file(), Some(log.display().to_string()));
            // A long log: only its end is read, from a whole line on.
            let long = format!("{}\nlast line\n", "x".repeat(200_000));
            std::fs::write(&log, long).unwrap();
            assert_eq!(server.output(10), ["last line"]);
            let _ = std::fs::remove_file(log);
            assert!(ComfyProcess::new(None).output(10).is_empty() && ComfyProcess::new(None).log_file().is_none());
        }

        #[test]
        fn a_crash_dump_is_told_by_its_cause_not_its_last_lines() {
            use super::super::native::{crash_name, crash_summary};
            // What ComfyUI printed when PyTorch crashed loading in a virtual machine (trimmed).
            let dump = "Windows fatal exception: access violation\n\nCurrent thread 0x00001a2c (most recent call first):\n  \
File \"C:\\e\\python_embeded\\Lib\\site-packages\\torch\\__init__.py\", line 281 in _load_dll_libraries\n  \
File \"<frozen importlib._bootstrap>\", line 488 in _call_with_frames_removed\n  \
File \"<frozen importlib._bootstrap>\", line 1331 in _find_and_load_unlocked\n  \
File \"<frozen importlib._bootstrap>\", line 1360 in _find_and_load\n  \
File \"C:\\e\\ComfyUI\\main.py\", line 254 in <module>\n";
            let lines: Vec<&str> = dump.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
            let got = crash_summary(&lines).unwrap();
            assert!(got.starts_with("Windows fatal exception: access violation, in File ") && got.contains("_load_dll_libraries"), "{got}");
            // A Python error is not a crash dump: its own last line says it.
            assert_eq!(crash_summary(&["Traceback (most recent call last):", "ImportError: DLL load failed"]), None);
            assert_eq!(crash_summary(&["Fatal Python error: Aborted"]).as_deref(), Some("Fatal Python error: Aborted"));
            assert_eq!(crash_summary(&[]), None);
            assert!(crash_name(0xC000_001D).is_some_and(|n| n.contains("processor")) && crash_name(1).is_none());
        }

        #[test]
        fn cancelling_a_start_stops_what_was_started() {
            let server = ComfyProcess::new(None);
            let mut s = settings(true);
            s.comfy_launch = stand_in();
            let asked = AtomicU32::new(0);
            let cancelled = || {
                asked.fetch_add(1, Ordering::Relaxed);
                true
            };
            assert_eq!(server.ensure(&Fake(None), &s, &control(&cancelled)).unwrap_err(), Error::Cancelled);
            assert_eq!(asked.load(Ordering::Relaxed), 1);
            assert_eq!(server.state(), ServerState::NotStarted);
            assert!(!server.stop());
        }

        #[test]
        fn finding_needs_a_running_local_server() {
            assert!(find_running(&Fake(None), "http://127.0.0.1:8000").unwrap_err().to_string().contains("not running"));
            // The launcher's Find falls back to an installation on disk, or says how to go on.
            match ComfyProcess::new(None).find(&Fake(None), "http://127.0.0.1:8000") {
                Ok(found) => assert!(!found.python.is_empty() && !found.folder.is_empty(), "{found:?}"),
                Err(e) => assert!(e.to_string().contains("Browse"), "{e}"),
            }
            let up = Fake(Some(r#"{"system":{"argv":["main.py","--port","8000","--enable-manager"]}}"#));
            assert!(find_running(&up, "http://10.0.0.5:8000").unwrap_err().to_string().contains("another computer"));
            // No such process on the test machine: the arguments are still reported.
            let found = find_running(&up, "http://127.0.0.1:1").unwrap();
            assert_eq!(found.args, "main.py --enable-manager");
        }
    }
}
