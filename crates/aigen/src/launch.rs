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
    const STARTUP_SECONDS: u64 = 300;

    /// ComfyUI as a child process of the app.
    pub struct ComfyProcess {
        child: Mutex<Option<Child>>,
        /// Where the server's output goes, for the error message when it fails to start.
        log: Option<PathBuf>,
    }

    impl ComfyProcess {
        pub fn new(log: Option<PathBuf>) -> Self {
            Self { child: Mutex::new(None), log }
        }

        fn child(&self) -> std::sync::MutexGuard<'_, Option<Child>> {
            self.child.lock().unwrap_or_else(PoisonError::into_inner)
        }

        /// The child if it is still running (forgets one that has exited).
        fn alive(&self) -> bool {
            let mut guard = self.child();
            let running = guard.as_mut().is_some_and(|c| matches!(c.try_wait(), Ok(None)));
            if !running {
                *guard = None;
            }
            running
        }

        /// The last lines the server printed, for an error message.
        fn log_tail(&self) -> String {
            let Some(text) = self.log.as_ref().and_then(|p| std::fs::read(p).ok()) else { return String::new() };
            let text = String::from_utf8_lossy(&text);
            let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
            let tail = lines.iter().rev().take(3).rev().copied().collect::<Vec<_>>().join(" | ");
            comfy::clip(&tail, 400)
        }

        fn spawn(&self, settings: &Settings, port: u16) -> Result<Child> {
            let launch = &settings.comfy_launch;
            let python = launch.python.trim();
            let folder = launch.folder.trim();
            let hint = "set it on the Local Server page of Preferences › Integrations (Find Installation fills it in while ComfyUI is running)";
            if python.is_empty() || !Path::new(python).is_file() {
                return Err(Error::Config(format!("ComfyUI's Python was not found at `{python}`: {hint}")));
            }
            if folder.is_empty() || !Path::new(folder).is_dir() {
                return Err(Error::Config(format!("ComfyUI's folder `{folder}` does not exist: {hint}")));
            }
            let mut args = without_port(split_args(&launch.args));
            if args.is_empty() {
                args.push("main.py".into());
            }
            args.push("--port".into());
            args.push(port.to_string());
            let mut cmd = Command::new(python);
            cmd.current_dir(folder).args(&args).stdin(Stdio::null());
            let output = |log: &Option<PathBuf>| {
                log.as_ref().and_then(|p| std::fs::OpenOptions::new().create(true).append(true).open(p).ok()).map_or_else(Stdio::null, Stdio::from)
            };
            if let Some(p) = &self.log {
                // Start each run with a fresh log: only its tail is ever read.
                let _ = std::fs::write(p, b"");
            }
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
            if !settings.comfy_autostart {
                // Not ours to start: the request that follows reports that it isn't reachable.
                return Ok(());
            }
            let (base, port) = local_base(settings)?;
            if !self.alive() {
                let child = self.spawn(settings, port)?;
                *self.child() = Some(child);
            }
            let polls = STARTUP_SECONDS * 2;
            for n in 0..polls {
                if (control.cancelled)() {
                    // A half-started server is no use to anyone.
                    self.stop();
                    return Err(Error::Cancelled);
                }
                (control.progress)(0.01 + 0.03 * (n as f32 / polls as f32), "Starting ComfyUI…");
                if !self.alive() {
                    let tail = self.log_tail();
                    let why = if tail.is_empty() { String::new() } else { format!(": {tail}") };
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

        fn find(&self, transport: &dyn Transport, url: &str) -> Result<Launch> {
            find_running(transport, url)
        }
    }

    impl Drop for ComfyProcess {
        fn drop(&mut self) {
            self.stop();
        }
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
        if let Some(exe) = &parent_exe {
            candidates.extend(exe.ancestors().skip(1).take(3).flat_map(|d| [d.join("resources"), d.to_path_buf()]));
        }
        candidates.extend(arg_after(&argv, "--base-directory").map(PathBuf::from));
        candidates.extend(venv_root);
        let folder = candidates.into_iter().find(|d| d.join(&script).is_file()).map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
        Ok(Launch { python: server.exe.clone(), folder, args })
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::find_running;

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
    fn command_lines_round_trip() {
        let args = split_args(r#"ComfyUI\main.py --base-directory "C:\My Docs\ComfyUI" --enable-manager  --x """#);
        assert_eq!(args, vec!["ComfyUI\\main.py", "--base-directory", "C:\\My Docs\\ComfyUI", "--enable-manager", "--x", ""]);
        assert_eq!(split_args(&join_args(&args)), args);
        assert!(split_args("   ").is_empty());
        assert_eq!(split_args("\"unterminated quote"), vec!["unterminated quote"]);
        let v = |x: &[&str]| x.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(without_port(v(&["main.py", "--port", "8000", "--a", "--port=1", "--port"])), v(&["main.py", "--a"]));
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
        fn without_autostart_nothing_is_started() {
            let server = ComfyProcess::new(None);
            server.ensure(&Fake(None), &settings(false), &control(&|| false)).unwrap();
            assert_eq!(server.state(), ServerState::NotStarted);
        }

        #[test]
        fn starting_needs_a_local_address_and_a_real_python() {
            let server = ComfyProcess::new(None);
            let mut s = settings(true);
            let e = server.ensure(&Fake(None), &s, &control(&|| false)).unwrap_err().to_string();
            assert!(e.contains("Python was not found") && e.contains("Find Installation"), "{e}");
            s.comfy_launch = Launch { python: "/no/such/python".into(), folder: "/no/such/folder".into(), args: "main.py".into() };
            assert!(server.ensure(&Fake(None), &s, &control(&|| false)).unwrap_err().to_string().contains("Python was not found"));
            s.comfy_url = "http://192.168.1.50:8188".into();
            let e = server.ensure(&Fake(None), &s, &control(&|| false)).unwrap_err().to_string();
            assert!(e.contains("only start it on this computer"), "{e}");
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
            assert!(e.contains("stopped while starting"), "{e}");
            assert_eq!(server.state(), ServerState::NotStarted);
            let _ = std::fs::remove_file(log);
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
            let up = Fake(Some(r#"{"system":{"argv":["main.py","--port","8000","--enable-manager"]}}"#));
            assert!(find_running(&up, "http://10.0.0.5:8000").unwrap_err().to_string().contains("another computer"));
            // No such process on the test machine: the arguments are still reported.
            let found = find_running(&up, "http://127.0.0.1:1").unwrap();
            assert_eq!(found.args, "main.py --enable-manager");
        }
    }
}
