//! The Microsoft C++ runtime the engine's PyTorch needs (`msvcp140.dll` and its kin), put beside
//! the engine's Python, so a PC without the Visual C++ Redistributable runs it.
//!
//! ComfyUI's portable build ships the Python embeddable package, which carries
//! `vcruntime140.dll` and `vcruntime140_1.dll` but not `msvcp140.dll`, which PyTorch's `c10.dll`
//! imports, nor `vcruntime140_threads.dll`, which its `torch_cpu.dll` imports (a newer part of the
//! runtime, from 14.40). On a Windows that has never had the Redistributable installed (a new PC,
//! Windows Sandbox) `import torch` fails with WinError 126 and ComfyUI stops while starting. Microsoft
//! allows these DLLs to be deployed app-locally. The packaged app carries them in [`BUNDLE`]
//! beside `photocraft.exe`, and [`ensure`] copies them next to the engine's `python.exe` before
//! the engine starts, without administrator rights. A copy there is never replaced by an older
//! one, and the newest copy on hand wins (PyTorch is built with a recent compiler: an older
//! `msvcp140.dll` than it was built against can crash it).

use std::path::{Path, PathBuf};

use crate::probe::VcRuntime;
use crate::{Error, Result};

/// The runtime's DLLs that are put beside the engine's Python: the C++ library, the compiler
/// runtime (with its threads part) and the concurrency runtime, as Visual Studio's
/// `Microsoft.VC14x.CRT` has them.
pub const DLLS: [&str; 9] = [
    "msvcp140.dll",
    "msvcp140_1.dll",
    "msvcp140_2.dll",
    "msvcp140_atomic_wait.dll",
    "msvcp140_codecvt_ids.dll",
    "vcruntime140.dll",
    "vcruntime140_1.dll",
    "vcruntime140_threads.dll",
    "concrt140.dll",
];

/// The ones PyTorch can't load without.
pub const NEEDED: [&str; 4] = ["msvcp140.dll", "vcruntime140.dll", "vcruntime140_1.dll", "vcruntime140_threads.dll"];

/// The folder beside `photocraft.exe` the packaged app carries the runtime in.
pub const BUNDLE: &str = "vc-runtime";

/// Microsoft's installer for the runtime: the fallback when the app has no copy to give.
pub const REDIST_URL: &str = "https://aka.ms/vs/17/release/vc_redist.x64.exe";

/// The runtime DLLs are a megabyte or two; anything far larger is not one.
const MAX_DLL: u64 = 16 << 20;

/// Where the runtime can be had on this PC.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sources {
    /// The app's own copy ([`BUNDLE`] beside the running exe).
    pub bundled: Option<PathBuf>,
    /// Windows' System32, where the Redistributable installs it.
    pub system: Option<PathBuf>,
}

impl Sources {
    /// This PC's: the app's copy, and System32. Empty off Windows.
    pub fn this_pc() -> Sources {
        if !cfg!(windows) {
            return Sources::default();
        }
        let bundled = std::env::current_exe().ok().and_then(|exe| exe.parent().map(|dir| dir.join(BUNDLE))).filter(|d| d.is_dir());
        let system = std::env::var_os("SystemRoot").map(|root| PathBuf::from(root).join("System32")).filter(|d| d.is_dir());
        Sources { bundled, system }
    }

    /// Whether the engine will find the runtime: the app has it to give, or Windows has it.
    pub fn found(&self) -> VcRuntime {
        let has = |dir: &Option<PathBuf>| dir.as_ref().is_some_and(|d| NEEDED.iter().all(|n| d.join(n).is_file()));
        if has(&self.bundled) {
            VcRuntime::Bundled
        } else if has(&self.system) {
            VcRuntime::Installed
        } else {
            VcRuntime::Missing
        }
    }
}

/// A DLL's file version (`14.42.34438.0` as `[14, 42, 34438, 0]`), read from its version
/// resource: the `VS_FIXEDFILEINFO` record, found by its signature. `None` when the file can't be
/// read or has none.
pub fn file_version(path: &Path) -> Option<[u16; 4]> {
    let size = std::fs::metadata(path).ok()?.len();
    if size > MAX_DLL {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let u32_at = |at: usize| bytes.get(at..at.checked_add(4)?).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes);
    // The record is 32-bit aligned: signature, structure version, then the file version.
    let at = (0..bytes.len()).step_by(4).find(|&i| u32_at(i) == Some(0xFEEF_04BD))?;
    let (ms, ls) = (u32_at(at.checked_add(8)?)?, u32_at(at.checked_add(12)?)?);
    Some([(ms >> 16) as u16, ms as u16, (ls >> 16) as u16, ls as u16])
}

/// Put the runtime beside the engine's Python in `python_dir`: each DLL that is missing there, or
/// older there than the newest copy in `sources`. Returns the names copied. Fails, saying how to
/// fix it, when what PyTorch needs is neither there afterwards nor in System32 (where Windows
/// would find it anyway).
pub fn ensure(python_dir: &Path, sources: &Sources) -> Result<Vec<String>> {
    let mut copied = Vec::new();
    for name in DLLS {
        let target = python_dir.join(name);
        let newest = [&sources.bundled, &sources.system]
            .into_iter()
            .flatten()
            .map(|dir| dir.join(name))
            .filter_map(|p| file_version(&p).map(|v| (v, p)))
            .max_by_key(|(v, _)| *v);
        let Some((version, from)) = newest else { continue };
        let there = target.is_file();
        // One whose version can't be read is left as it is: it may be what the engine was made with.
        if there && file_version(&target).is_none_or(|have| have >= version) {
            continue;
        }
        match replace(&from, &target) {
            Ok(()) => copied.push(name.to_string()),
            // An older one in use (the engine running) still works: it is replaced next time.
            Err(_) if there => {}
            Err(e) => return Err(Error::Engine(format!("Could not put {name} beside the engine's Python ({}): {e}", python_dir.display()))),
        }
    }
    let missing: Vec<&str> =
        NEEDED.iter().copied().filter(|n| !python_dir.join(n).is_file() && !sources.system.as_ref().is_some_and(|s| s.join(n).is_file())).collect();
    if !missing.is_empty() {
        return Err(Error::Engine(format!(
            "The engine needs the Microsoft Visual C++ runtime ({}), and this PC doesn't have it. Install the Visual C++ Redistributable (x64) from {REDIST_URL}, then press Continue.",
            missing.join(", ")
        )));
    }
    Ok(copied)
}

/// Copy `from` over `to` by way of a file beside it, so a copy cut short never stands in for the DLL.
fn replace(from: &Path, to: &Path) -> std::io::Result<()> {
    let part = to.with_extension("dll.part");
    std::fs::copy(from, &part)?;
    std::fs::rename(&part, to).inspect_err(|_| {
        let _ = std::fs::remove_file(&part);
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// A stand-in DLL: some bytes, then a version resource saying `version`.
    fn dll(path: &Path, version: [u16; 4]) {
        let mut bytes = vec![0x4Du8, 0x5A, 0, 0, 7, 7, 7, 7];
        bytes.extend_from_slice(&0xFEEF_04BDu32.to_le_bytes());
        bytes.extend_from_slice(&0x0001_0000u32.to_le_bytes());
        bytes.extend_from_slice(&((u32::from(version[0]) << 16) | u32::from(version[1])).to_le_bytes());
        bytes.extend_from_slice(&((u32::from(version[2]) << 16) | u32::from(version[3])).to_le_bytes());
        bytes.extend_from_slice(&[0u8; 40]);
        std::fs::write(path, bytes).unwrap();
    }

    fn folder(root: &Path, name: &str, version: [u16; 4], names: &[&str]) -> PathBuf {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        for n in names {
            dll(&dir.join(n), version);
        }
        dir
    }

    #[test]
    fn versions_are_read_from_the_version_resource() {
        let dir = tempfile::tempdir().unwrap();
        dll(&dir.path().join("a.dll"), [14, 42, 34438, 0]);
        assert_eq!(file_version(&dir.path().join("a.dll")), Some([14, 42, 34438, 0]));
        std::fs::write(dir.path().join("b.dll"), b"MZ no version here").unwrap();
        assert_eq!(file_version(&dir.path().join("b.dll")), None);
        assert_eq!(file_version(&dir.path().join("missing.dll")), None);
        // Short and odd-sized files are no answer, not a panic.
        for len in 0..12 {
            std::fs::write(dir.path().join("c.dll"), &0xFEEF_04BDu32.to_le_bytes().repeat(3)[..len]).unwrap();
            assert_eq!(file_version(&dir.path().join("c.dll")), None, "{len}");
        }
        // A real one: Windows' own kernel32.dll is 10.0.x.
        if let Some(root) = std::env::var_os("SystemRoot").filter(|_| cfg!(windows)) {
            let version = file_version(&PathBuf::from(root).join("System32").join("kernel32.dll"));
            assert!(version.is_some_and(|v| v[0] >= 10), "{version:?}");
        }
    }

    #[test]
    fn a_clean_pc_gets_the_bundled_runtime_beside_python() {
        let dir = tempfile::tempdir().unwrap();
        // The portable build's Python: vcruntime140 and vcruntime140_1, no msvcp140.
        let python = folder(dir.path(), "python_embeded", [14, 42, 34438, 0], &["vcruntime140.dll", "vcruntime140_1.dll"]);
        let bundled = folder(dir.path(), "vc-runtime", [14, 51, 36247, 0], &DLLS);
        let sources = Sources { bundled: Some(bundled), system: Some(dir.path().join("System32-without-it")) };
        assert_eq!(sources.found(), VcRuntime::Bundled);
        let copied = ensure(&python, &sources).unwrap();
        assert_eq!(copied.len(), DLLS.len(), "all of it, the older vcruntime140 too: {copied:?}");
        for n in DLLS {
            assert_eq!(file_version(&python.join(n)), Some([14, 51, 36247, 0]), "{n}");
        }
        assert!(!python.join("msvcp140.dll.part").exists());
        // Again: nothing to do.
        assert!(ensure(&python, &sources).unwrap().is_empty());
    }

    #[test]
    fn a_newer_copy_is_never_replaced_by_an_older_one() {
        let dir = tempfile::tempdir().unwrap();
        let python = folder(dir.path(), "python_embeded", [14, 60, 1, 0], &["vcruntime140.dll", "vcruntime140_1.dll"]);
        dll(&python.join("concrt140.dll"), [14, 51, 36247, 0]);
        std::fs::write(python.join("msvcp140_1.dll"), b"no version").unwrap();
        let bundled = folder(dir.path(), "vc-runtime", [14, 42, 34438, 0], &DLLS);
        let system = folder(dir.path(), "System32", [14, 51, 36247, 0], &["msvcp140.dll", "concrt140.dll"]);
        let copied = ensure(&python, &Sources { bundled: Some(bundled), system: Some(system) }).unwrap();
        // The newest copy on hand wins: System32's msvcp140 over the bundle's.
        assert_eq!(file_version(&python.join("msvcp140.dll")), Some([14, 51, 36247, 0]));
        assert_eq!(file_version(&python.join("vcruntime140.dll")), Some([14, 60, 1, 0]), "newer: kept");
        assert_eq!(file_version(&python.join("msvcp140_1.dll")), None, "unreadable: left alone");
        assert!(!copied.iter().any(|n| n == "concrt140.dll" || n == "vcruntime140.dll" || n == "msvcp140_1.dll"), "{copied:?}");
    }

    #[test]
    fn without_any_copy_it_says_how_to_get_the_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let python = folder(dir.path(), "python_embeded", [14, 42, 34438, 0], &["vcruntime140.dll", "vcruntime140_1.dll"]);
        let sources = Sources { bundled: None, system: Some(dir.path().join("empty")) };
        assert_eq!(sources.found(), VcRuntime::Missing);
        let e = ensure(&python, &sources).unwrap_err().to_string();
        assert!(e.contains("msvcp140.dll") && !e.contains("vcruntime140.dll") && e.contains(REDIST_URL), "{e}");
        // Installed in System32: Windows finds it there, nothing has to be copied to work.
        let system = folder(dir.path(), "System32", [14, 51, 36247, 0], &NEEDED);
        let sources = Sources { bundled: None, system: Some(system) };
        assert_eq!(sources.found(), VcRuntime::Installed);
        assert!(ensure(&python, &sources).is_ok());
    }
}
