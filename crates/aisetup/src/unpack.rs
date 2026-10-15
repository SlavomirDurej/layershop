//! Unpacking the runtime: a 7z archive, read in pure Rust.
//!
//! The archive is checked against its pinned sha256 before it gets here, but it is still read
//! as hostile: every entry's path is looked at before anything is written ([`entry_path`]), and
//! the archive is refused whole if any entry would land outside the target folder, is a link,
//! or if together they are larger or more numerous than a ComfyUI build has any reason to be.
//!
//! Files are written to a staging folder beside the target, which takes the target's name only
//! when every file is out: an unpack that is interrupted leaves no half an engine behind.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use sevenz_rust2::{ArchiveReader, Password};

use crate::{Error, Result};

/// What an archive may hold at most.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Bytes once unpacked.
    pub max_bytes: u64,
    pub max_files: usize,
}

impl Default for Limits {
    /// ComfyUI's portable build unpacks to about 6 GB in under a hundred thousand files.
    fn default() -> Self {
        Self { max_bytes: 32 << 30, max_files: 600_000 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unpacked {
    pub files: usize,
    pub bytes: u64,
}

fn refuse(name: &str, why: &str) -> Error {
    Error::Archive(format!("The engine archive was not unpacked: its entry `{}` {why}.", crate::fetch::clip(name, 120)))
}

/// Names Windows gives to devices, whatever their extension.
fn is_device(component: &str) -> bool {
    let stem = component.split('.').next().unwrap_or("").to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.ends_with(|c: char| c.is_ascii_digit() && c != '0'))
}

/// Where an archive entry named `name` goes, relative to the target folder; or why the archive
/// is not one to unpack. Both `/` and `\` separate, as on Windows.
pub fn entry_path(name: &str) -> Result<PathBuf> {
    if name.is_empty() || name.len() > 1024 {
        return Err(refuse(name, "has no usable name"));
    }
    if name.chars().any(|c| c.is_control()) {
        return Err(refuse(name, "has control characters in its name"));
    }
    if name.starts_with(['/', '\\']) {
        return Err(refuse(name, "has an absolute path"));
    }
    if name.contains(':') {
        // `C:\…`, and `file:stream` on NTFS.
        return Err(refuse(name, "names a drive or a stream"));
    }
    // A folder may be listed with its trailing separator.
    let trimmed = name.strip_suffix(['/', '\\']).unwrap_or(name);
    let mut out = PathBuf::new();
    for part in trimmed.split(['/', '\\']) {
        match part {
            "" => return Err(refuse(name, "has an empty folder name")),
            "." | ".." => return Err(refuse(name, "climbs out of the folder")),
            _ => {}
        }
        if part.contains(['<', '>', '"', '|', '?', '*']) || part.ends_with([' ', '.']) || is_device(part) {
            return Err(refuse(name, "has a name Windows can't store as written"));
        }
        out.push(part);
    }
    Ok(out)
}

/// The attribute bits that make an entry a link: an NTFS reparse point, or a Unix symlink.
fn is_link(has_attributes: bool, attributes: u32) -> bool {
    const REPARSE_POINT: u32 = 0x400;
    const UNIX_EXTENSION: u32 = 0x8000;
    has_attributes && (attributes & REPARSE_POINT != 0 || (attributes & UNIX_EXTENSION != 0 && (attributes >> 16) & 0o170_000 == 0o120_000))
}

fn io(path: &Path, what: &str, e: &std::io::Error) -> Error {
    match e.kind() {
        std::io::ErrorKind::StorageFull | std::io::ErrorKind::QuotaExceeded => {
            Error::Disk(format!("The drive is full: could not {what} `{}`. Free some space, or choose another engine folder, then continue.", path.display()))
        }
        _ => Error::Disk(format!(
            "Could not {what} `{}`: {e}. If an antivirus is holding it, exclude the engine folder from its scans, then continue.",
            path.display()
        )),
    }
}

/// What an archive holds, read from its header alone (nothing is unpacked).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Contents {
    pub files: usize,
    pub folders: usize,
    /// Bytes once unpacked.
    pub bytes: u64,
    /// The longest path it unpacks to, in characters, without the top folder that is left out.
    pub longest_path: usize,
    /// The top folder every entry is inside, when there is one.
    pub root: Option<String>,
    /// How the data is compressed (`LZMA2`, `BCJ`, …), as the archive names it.
    pub methods: Vec<String>,
    /// The paths, as they unpack.
    pub paths: Vec<String>,
}

/// Read what the 7z at `archive` holds, with the same checks [`unpack`] makes: an archive this
/// refuses, that refuses. `cargo xtask ai-manifest` uses it to measure a new ComfyUI build.
pub fn contents(archive: &Path, limits: &Limits) -> Result<Contents> {
    let damaged = |e: sevenz_rust2::Error| Error::Archive(format!("The archive could not be read ({e})."));
    let reader = ArchiveReader::open(archive, Password::empty()).map_err(damaged)?;
    let entries = &reader.archive().files;
    if entries.len() > limits.max_files {
        return Err(Error::Archive(format!("The archive holds {} files, more than a ComfyUI build has.", entries.len())));
    }
    let mut paths = Vec::with_capacity(entries.len());
    let mut bytes = 0u64;
    for e in entries {
        if e.is_anti_item || is_link(e.has_windows_attributes, e.windows_attributes) {
            return Err(refuse(&e.name, "is a link or a deletion"));
        }
        paths.push((entry_path(&e.name)?, e.is_directory));
        bytes =
            bytes.checked_add(e.size).filter(|t| *t <= limits.max_bytes).ok_or_else(|| Error::Archive("The archive is larger than a ComfyUI build.".into()))?;
    }
    let root = common_root(&paths);
    let relative: Vec<String> = paths
        .iter()
        .map(|(p, _)| root.as_ref().and_then(|r| p.strip_prefix(r).ok()).unwrap_or(p).to_string_lossy().replace('\\', "/"))
        .filter(|p| !p.is_empty())
        .collect();
    let mut methods: Vec<String> = Vec::new();
    for block in &reader.archive().blocks {
        for coder in &block.coders {
            let id = coder.encoder_method_id();
            let name = sevenz_rust2::EncoderMethod::by_id(id).map_or_else(|| format!("{id:x?}"), |m| m.name().to_string());
            if !methods.contains(&name) {
                methods.push(name);
            }
        }
    }
    Ok(Contents {
        files: paths.iter().filter(|(_, dir)| !dir).count(),
        folders: paths.iter().filter(|(_, dir)| *dir).count(),
        bytes,
        longest_path: relative.iter().map(|p| p.chars().count()).max().unwrap_or(0),
        root: root.map(|r| r.to_string_lossy().into_owned()),
        methods,
        paths: relative,
    })
}

/// The staging folder of `dest`.
pub fn staging_of(dest: &Path) -> PathBuf {
    let mut name = dest.as_os_str().to_os_string();
    name.push(".unpacking");
    PathBuf::from(name)
}

/// The folder every entry is inside, when there is exactly one: the build's top-level folder,
/// which is left out so the paths stay short.
fn common_root(paths: &[(PathBuf, bool)]) -> Option<PathBuf> {
    let first = paths.first()?.0.components().next()?;
    let root = PathBuf::from(first.as_os_str());
    // A lone file that is its own "root" is not a folder to strip.
    let all = paths.iter().all(|(p, is_dir)| p.starts_with(&root) && (*is_dir || p.components().count() > 1));
    all.then_some(root)
}

/// Unpack the 7z at `archive` into `dest`, which must not exist yet (an empty folder is fine).
/// With `finish`, the staging folder is handed to it before it takes `dest`'s name: what it
/// writes there (the marker that says the engine is whole) appears together with the files.
///
/// `progress` is told the bytes written and their total.
pub fn unpack(
    archive: &Path,
    dest: &Path,
    limits: &Limits,
    cancelled: &dyn Fn() -> bool,
    progress: &dyn Fn(u64, u64),
    finish: &dyn Fn(&Path) -> Result<()>,
) -> Result<Unpacked> {
    let damaged = |e: sevenz_rust2::Error| {
        Error::Archive(format!(
            "The engine archive `{}` could not be read ({e}). It will be downloaded again.",
            archive.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
        ))
    };
    let mut reader = ArchiveReader::open(archive, Password::empty()).map_err(damaged)?;
    // Everything is decided before the first byte is written.
    let entries = &reader.archive().files;
    if entries.len() > limits.max_files {
        return Err(Error::Archive(format!("The engine archive was not unpacked: it holds {} files, more than a ComfyUI build has.", entries.len())));
    }
    let mut total = 0u64;
    let mut paths = Vec::with_capacity(entries.len());
    for e in entries {
        if e.is_anti_item {
            return Err(refuse(&e.name, "deletes a file instead of adding one"));
        }
        if is_link(e.has_windows_attributes, e.windows_attributes) {
            return Err(refuse(&e.name, "is a link"));
        }
        paths.push((entry_path(&e.name)?, e.is_directory));
        total = total.checked_add(e.size).filter(|t| *t <= limits.max_bytes).ok_or_else(|| {
            Error::Archive(format!("The engine archive was not unpacked: it would take more than {} on disk.", crate::human_bytes(limits.max_bytes)))
        })?;
    }
    let root = common_root(&paths);
    if let Ok(mut inside) = std::fs::read_dir(dest) {
        if inside.next().is_some() {
            return Err(Error::Disk(format!(
                "The folder `{}` already exists and is not empty, and this setup did not put it there. Remove it, or choose another engine folder.",
                dest.display()
            )));
        }
        std::fs::remove_dir(dest).map_err(|e| io(dest, "replace", &e))?;
    }
    let staging = staging_of(dest);
    if staging.exists() {
        // An earlier unpack that was interrupted: its name says it is ours to clear.
        std::fs::remove_dir_all(&staging).map_err(|e| io(&staging, "clear", &e))?;
    }
    std::fs::create_dir_all(&staging).map_err(|e| io(&staging, "create", &e))?;

    let mut failed: Option<Error> = None;
    let mut written = 0u64;
    let mut files = 0usize;
    let mut buf = vec![0u8; 1 << 20];
    let stop = || sevenz_rust2::Error::Other("stopped".into());
    let outcome = reader.for_each_entries(|entry, data| {
        let mut one = || -> Result<()> {
            if cancelled() {
                return Err(Error::Cancelled);
            }
            // Checked again where it is used: the list above is what was promised, this is what
            // is written.
            let relative = entry_path(&entry.name)?;
            let relative = match &root {
                Some(root) => relative.strip_prefix(root).map(Path::to_path_buf).unwrap_or(relative),
                None => relative,
            };
            if relative.as_os_str().is_empty() {
                return Ok(());
            }
            let target = staging.join(&relative);
            if entry.is_directory {
                return std::fs::create_dir_all(&target).map_err(|e| io(&target, "create", &e));
            }
            if let Some(dir) = target.parent() {
                std::fs::create_dir_all(dir).map_err(|e| io(dir, "create", &e))?;
            }
            let mut file = File::create(&target).map_err(|e| io(&target, "write", &e))?;
            let mut left = entry.size;
            loop {
                let n = data
                    .read(&mut buf)
                    .map_err(|e| Error::Archive(format!("The engine archive is damaged at `{}` ({e}). It will be downloaded again.", entry.name)))?;
                let Some(chunk) = buf.get(..n).filter(|c| !c.is_empty()) else { break };
                // More than the entry declared is more than was budgeted for.
                left = left.checked_sub(n as u64).ok_or_else(|| refuse(&entry.name, "is larger than it says"))?;
                file.write_all(chunk).map_err(|e| io(&target, "write", &e))?;
                written = written.saturating_add(n as u64);
                progress(written, total);
                if cancelled() {
                    return Err(Error::Cancelled);
                }
            }
            files += 1;
            Ok(())
        };
        match one() {
            Ok(()) => Ok(true),
            Err(e) => {
                failed = Some(e);
                Err(stop())
            }
        }
    });
    let result = match (failed, outcome) {
        (Some(e), _) => Err(e),
        (None, Err(e)) => Err(damaged(e)),
        (None, Ok(())) => finish(&staging).and_then(|()| std::fs::rename(&staging, dest).map_err(|e| io(dest, "finish", &e))),
    };
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }
    Ok(Unpacked { files, bytes: written })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sevenz_rust2::{ArchiveEntry, ArchiveWriter};
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A small archive: (name, contents); a name ending in `/` is a folder.
    fn archive(dir: &Path, entries: &[(&str, &[u8])]) -> PathBuf {
        let path = dir.join("build.7z");
        let mut writer = ArchiveWriter::create(&path).unwrap();
        for (name, data) in entries {
            if let Some(folder) = name.strip_suffix('/') {
                writer.push_archive_entry::<&[u8]>(ArchiveEntry::new_directory(folder), None).unwrap();
            } else {
                writer.push_archive_entry(ArchiveEntry::new_file(name), Some(*data)).unwrap();
            }
        }
        writer.finish().unwrap();
        path
    }

    fn go(archive: &Path, dest: &Path, limits: &Limits) -> Result<Unpacked> {
        unpack(archive, dest, limits, &|| false, &|_, _| {}, &|_| Ok(()))
    }

    const BUILD: &[(&str, &[u8])] = &[
        ("ComfyUI_windows_portable/", b""),
        ("ComfyUI_windows_portable/python_embeded/python.exe", b"MZ python"),
        ("ComfyUI_windows_portable/ComfyUI/main.py", b"print('comfy')"),
        ("ComfyUI_windows_portable/ComfyUI/models/", b""),
        ("ComfyUI_windows_portable/run_nvidia_gpu.bat", b"python_embeded\\python.exe -s ComfyUI\\main.py"),
        ("ComfyUI_windows_portable/empty.txt", b""),
    ];

    #[test]
    fn a_build_unpacks_without_its_top_folder_and_appears_whole() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("engine").join("0.39.0");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        let seen = AtomicU64::new(0);
        let marked = |staging: &Path| std::fs::write(staging.join("marker.json"), b"{}").map_err(|e| Error::Disk(e.to_string()));
        let done =
            unpack(&archive(dir.path(), BUILD), &dest, &Limits::default(), &|| false, &|n, total| seen.store(n * 1000 + total, Ordering::SeqCst), &marked)
                .unwrap();
        let bytes: u64 = BUILD.iter().map(|(_, d)| d.len() as u64).sum();
        assert_eq!(done, Unpacked { files: 4, bytes });
        assert_eq!(seen.load(Ordering::SeqCst), bytes * 1000 + bytes);
        assert_eq!(std::fs::read(dest.join("python_embeded").join("python.exe")).unwrap(), b"MZ python");
        assert_eq!(std::fs::read(dest.join("ComfyUI").join("main.py")).unwrap(), b"print('comfy')");
        assert!(dest.join("ComfyUI").join("models").is_dir() && dest.join("empty.txt").is_file() && dest.join("marker.json").is_file());
        assert!(!staging_of(&dest).exists() && !dest.join("ComfyUI_windows_portable").exists());
    }

    #[test]
    fn an_archive_without_one_top_folder_keeps_its_paths() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out");
        go(&archive(dir.path(), &[("a/x.txt", b"x"), ("b/y.txt", b"y"), ("z.txt", b"z")]), &dest, &Limits::default()).unwrap();
        assert!(dest.join("a").join("x.txt").is_file() && dest.join("b").join("y.txt").is_file() && dest.join("z.txt").is_file());
        // A single file is not its own folder.
        let dest = dir.path().join("single");
        go(&archive(dir.path(), &[("only.txt", b"1")]), &dest, &Limits::default()).unwrap();
        assert_eq!(std::fs::read(dest.join("only.txt")).unwrap(), b"1");
    }

    /// The regression test the never-crash rules ask for: an entry that climbs out of the
    /// folder is refused, and nothing at all is written.
    #[test]
    fn an_archive_that_reaches_outside_its_folder_is_refused_whole() {
        for hostile in ["../evil.txt", "a/../../evil.txt", "/evil.txt", "\\evil.txt", "C:\\evil.txt", "a\\..\\..\\evil.txt", "ok/evil.txt:stream"] {
            let dir = tempfile::tempdir().unwrap();
            let dest = dir.path().join("deep").join("engine");
            std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
            let e = go(&archive(dir.path(), &[("good.txt", b"fine"), (hostile, b"evil")]), &dest, &Limits::default()).unwrap_err();
            assert!(matches!(&e, Error::Archive(m) if m.contains("not unpacked")), "{hostile}: {e:?}");
            assert!(!dest.exists() && !staging_of(&dest).exists(), "{hostile}: something was written");
            assert!(!dir.path().join("evil.txt").exists() && !dir.path().join("deep").join("evil.txt").exists(), "{hostile}");
        }
    }

    #[test]
    fn entry_names_are_plain_relative_paths_or_refused() {
        assert_eq!(entry_path("a/b/c.py").unwrap(), PathBuf::from("a").join("b").join("c.py"));
        assert_eq!(entry_path("a\\b\\c.py").unwrap(), PathBuf::from("a").join("b").join("c.py"));
        assert_eq!(entry_path("folder/").unwrap(), PathBuf::from("folder"));
        assert_eq!(entry_path("python_embeded/Lib/site-packages/torch/_C.cp313-win_amd64.pyd").unwrap().components().count(), 5);
        assert_eq!(entry_path("ünïcode/файл.py").unwrap().components().count(), 2);
        for bad in [
            "",
            "..",
            ".",
            "a/./b",
            "a//b",
            "//server/share/x",
            "a/b/..",
            "nul",
            "a/CON.txt",
            "com1",
            "LPT9.dat",
            "a/b.",
            "a /b",
            "a<b",
            "what?",
            "star*",
            "pipe|",
            "quote\"",
            "tab\there",
            "nul\0byte",
            &"x".repeat(2000),
        ] {
            assert!(entry_path(bad).is_err(), "{bad:?}");
        }
        // Not devices: the names only start like one.
        for fine in ["console.py", "com10.txt", "auxiliary/nul_check.py", "COM0", "lpt"] {
            assert!(entry_path(fine).is_ok(), "{fine}");
        }
        assert!(is_link(true, 0x400) && is_link(true, 0x8000 | (0o120_777 << 16)));
        assert!(!is_link(false, 0x400) && !is_link(true, 0x20) && !is_link(true, 0x8000 | (0o100_644 << 16)));
    }

    #[test]
    fn an_archive_larger_or_fuller_than_a_build_is_refused_before_it_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let big = archive(dir.path(), &[("a/one.bin", &[7u8; 5000]), ("a/two.bin", &[9u8; 5000])]);
        let dest = dir.path().join("out");
        let e = go(&big, &dest, &Limits { max_bytes: 9_999, max_files: 10 }).unwrap_err();
        assert!(matches!(&e, Error::Archive(m) if m.contains("more than")), "{e:?}");
        let e = go(&big, &dest, &Limits { max_bytes: 1 << 30, max_files: 1 }).unwrap_err();
        assert!(matches!(&e, Error::Archive(m) if m.contains("files")), "{e:?}");
        assert!(!dest.exists() && !staging_of(&dest).exists());
        go(&big, &dest, &Limits { max_bytes: 10_000, max_files: 2 }).unwrap();
    }

    #[test]
    fn an_interrupted_unpack_leaves_nothing_and_can_be_run_again() {
        let dir = tempfile::tempdir().unwrap();
        let build = archive(dir.path(), BUILD);
        let dest = dir.path().join("engine");
        // Cancelled part way: no engine, no staging folder.
        let asked = AtomicU64::new(0);
        let e = unpack(&build, &dest, &Limits::default(), &|| asked.fetch_add(1, Ordering::SeqCst) >= 3, &|_, _| {}, &|_| Ok(())).unwrap_err();
        assert_eq!(e, Error::Cancelled);
        assert!(!dest.exists() && !staging_of(&dest).exists());
        // A staging folder left by a crash is cleared; a finish that fails leaves no engine.
        std::fs::create_dir_all(staging_of(&dest).join("stale")).unwrap();
        let e = unpack(&build, &dest, &Limits::default(), &|| false, &|_, _| {}, &|_| Err(Error::Disk("no marker".into()))).unwrap_err();
        assert_eq!(e, Error::Disk("no marker".into()));
        assert!(!dest.exists() && !staging_of(&dest).exists());
        go(&build, &dest, &Limits::default()).unwrap();
        assert!(dest.join("ComfyUI").join("main.py").is_file() && !dest.join("stale").exists());
        // A folder that is there and not ours is left alone.
        let e = go(&build, &dest, &Limits::default()).unwrap_err();
        assert!(matches!(&e, Error::Disk(m) if m.contains("not empty")), "{e:?}");
        assert!(dest.join("ComfyUI").join("main.py").is_file());
        // An empty one is taken.
        let empty = dir.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        go(&build, &empty, &Limits::default()).unwrap();
    }

    #[test]
    fn what_is_not_a_7z_is_reported_as_damaged() {
        let dir = tempfile::tempdir().unwrap();
        let junk = dir.path().join("junk.7z");
        for bytes in [&b""[..], b"not an archive at all", &[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C, 0, 4, 0, 0, 0, 0]] {
            std::fs::write(&junk, bytes).unwrap();
            let e = go(&junk, &dir.path().join("out"), &Limits::default()).unwrap_err();
            assert!(matches!(&e, Error::Archive(m) if m.contains("could not be read")), "{e:?}");
        }
        // A real archive cut short.
        let build = archive(dir.path(), &[("a/big.bin", &[5u8; 40_000]), ("a/more.bin", &[6u8; 40_000])]);
        let bytes = std::fs::read(&build).unwrap();
        std::fs::write(&junk, &bytes[..bytes.len() / 2]).unwrap();
        assert!(matches!(go(&junk, &dir.path().join("out"), &Limits::default()), Err(Error::Archive(_))));
        assert!(matches!(go(&dir.path().join("missing.7z"), &dir.path().join("out"), &Limits::default()), Err(Error::Archive(_))));
        assert!(!dir.path().join("out").exists());
    }
}
