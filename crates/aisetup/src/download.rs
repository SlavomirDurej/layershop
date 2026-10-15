//! One file, downloaded so that it can be trusted and continued.
//!
//! Bytes go to `<name>.part` beside the target and are hashed as they are written. A download
//! that stops (network, cancel, the app closing) is continued from the part's length the next
//! time: the part is hashed again first, then the rest is asked for with a `Range`. Only when
//! the whole file is there and its sha256 is the one the manifest pins is the part renamed to
//! its final name, so a file under its final name is always a verified one.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::fetch::{Begin, Fetch, Request, Sink};
use crate::plan::part_of;
use crate::{Error, Result, human_bytes};

/// A network failure is retried this many times in a row; progress in between starts the count
/// again. After that the setup pauses and says so.
pub const MAX_TRIES: u32 = 5;
/// Kept free beyond a file's own size.
const FILE_HEADROOM: u64 = 64 << 20;

/// The file to download, as the manifest pins it.
#[derive(Clone, Copy, Debug)]
pub struct Spec<'a> {
    pub url: &'a str,
    pub size: u64,
    pub sha256: &'a str,
    pub token: Option<&'a str>,
}

/// What a download needs from its surroundings.
pub struct Ctx<'a> {
    pub fetch: &'a dyn Fetch,
    pub cancelled: &'a (dyn Fn() -> bool + Sync),
    pub sleep: &'a (dyn Fn(Duration) + Sync),
    /// Free bytes on the drive `path` is on, when it can be told.
    pub free_space: &'a (dyn Fn(&Path) -> Option<u64> + Sync),
}

/// The validator a part was downloaded under, kept beside it so a resume after a restart can
/// still ask "only if the file is the same".
fn validator_of(part: &Path) -> PathBuf {
    let mut name = part.as_os_str().to_os_string();
    name.push(".etag");
    PathBuf::from(name)
}

/// Free bytes on the drive of `path` (or of the nearest folder above it that exists).
pub fn free_space(path: &Path) -> Option<u64> {
    path.ancestors().find(|p| p.exists()).and_then(|p| fs4::available_space(p).ok())
}

fn disk(path: &Path, what: &str, e: &std::io::Error) -> Error {
    let name = path.display();
    match e.kind() {
        std::io::ErrorKind::StorageFull | std::io::ErrorKind::QuotaExceeded => {
            Error::Disk(format!("The drive is full: could not {what} `{name}`. Free some space, or choose another models folder, then continue."))
        }
        std::io::ErrorKind::PermissionDenied => Error::Disk(format!(
            "Could not {what} `{name}`: access was denied. If an antivirus is holding the file, exclude the folder from its scans; otherwise choose a folder you can write to."
        )),
        std::io::ErrorKind::NotFound => Error::Disk(format!(
            "`{name}` vanished while it was being written. An antivirus may have removed it: exclude the folder from its scans, then continue."
        )),
        _ => Error::Disk(format!("Could not {what} `{name}`: {e}")),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The sha256 of `bytes`, as lowercase hex: how the manifest writes checksums.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// Feed the first `len` bytes of `file` to `hasher`.
fn hash_prefix(file: &mut File, path: &Path, len: u64, hasher: &mut Sha256, cancelled: &dyn Fn() -> bool, progress: &dyn Fn(u64)) -> Result<()> {
    file.seek(SeekFrom::Start(0)).map_err(|e| disk(path, "read", &e))?;
    let mut buf = vec![0u8; 1 << 20];
    let mut left = len;
    while left > 0 {
        if cancelled() {
            return Err(Error::Cancelled);
        }
        let want = buf.len().min(usize::try_from(left).unwrap_or(usize::MAX));
        let n = file.read(buf.get_mut(..want).unwrap_or(&mut [])).map_err(|e| disk(path, "read", &e))?;
        let Some(read) = buf.get(..n).filter(|r| !r.is_empty()) else {
            return Err(Error::Disk(format!("`{}` is shorter than it was a moment ago", path.display())));
        };
        hasher.update(read);
        left = left.saturating_sub(n as u64);
        progress(len.saturating_sub(left));
    }
    Ok(())
}

/// Is the file at `path` exactly the one pinned: `size` bytes with this sha256? `progress` is
/// told how many bytes have been read.
pub fn verify(path: &Path, size: u64, sha256: &str, cancelled: &dyn Fn() -> bool, progress: &dyn Fn(u64)) -> Result<bool> {
    let Ok(mut file) = File::open(path) else { return Ok(false) };
    if file.metadata().map(|m| m.len()).ok() != Some(size) {
        return Ok(false);
    }
    let mut hasher = Sha256::new();
    hash_prefix(&mut file, path, size, &mut hasher, cancelled, progress)?;
    Ok(hex(&hasher.finalize()) == sha256.to_ascii_lowercase())
}

/// The part file being written, hashed as it grows.
struct Part<'a> {
    file: File,
    path: &'a Path,
    hasher: Sha256,
    written: u64,
    size: u64,
    progress: &'a dyn Fn(u64),
}

impl Sink for Part<'_> {
    fn begin(&mut self, begin: &Begin) -> Result<()> {
        if let Some(total) = begin.total.filter(|t| *t != self.size) {
            return Err(Error::Integrity(format!(
                "The server's file is {} ({total} bytes), and this version of the app expects {} ({} bytes): the model was changed upstream. Check for a newer version of the app.",
                human_bytes(total),
                human_bytes(self.size),
                self.size
            )));
        }
        if begin.offset != self.written {
            if begin.offset != 0 {
                return Err(Error::Server("the server sent a part of the file that was not asked for".into()));
            }
            // The whole file again (the server ignores ranges, or the file is not the one the
            // part came from): start over rather than stitch two files together.
            self.file.set_len(0).map_err(|e| disk(self.path, "write", &e))?;
            self.hasher = Sha256::new();
            self.written = 0;
        }
        self.file.seek(SeekFrom::Start(self.written)).map_err(|e| disk(self.path, "write", &e))?;
        if !begin.validator.is_empty() && begin.validator.len() <= 512 {
            let _ = std::fs::write(validator_of(self.path), begin.validator.as_bytes());
        }
        (self.progress)(self.written);
        Ok(())
    }

    fn write(&mut self, bytes: &[u8]) -> Result<()> {
        let after = self.written.saturating_add(bytes.len() as u64);
        if after > self.size {
            return Err(Error::Integrity(format!("The server sent more than the file's {} bytes.", self.size)));
        }
        self.file.write_all(bytes).map_err(|e| disk(self.path, "write", &e))?;
        self.hasher.update(bytes);
        self.written = after;
        (self.progress)(self.written);
        Ok(())
    }
}

/// Wait `total`, looking for a cancel every moment.
fn pause(ctx: &Ctx<'_>, total: Duration) -> Result<()> {
    let step = Duration::from_millis(200);
    let mut left = total;
    while !left.is_zero() {
        if (ctx.cancelled)() {
            return Err(Error::Cancelled);
        }
        let nap = left.min(step);
        (ctx.sleep)(nap);
        left = left.saturating_sub(nap);
    }
    Ok(())
}

fn remove_part(part: &Path) {
    let _ = std::fs::remove_file(part);
    let _ = std::fs::remove_file(validator_of(part));
}

/// Download `spec` to `dest`, continuing a part that is there. `progress` is told how many bytes
/// of the file are on disk (it starts at the part's length on a resume).
///
/// Returns once `dest` holds the file with the pinned sha256. A file already at `dest` is
/// replaced; whether one that is there needs downloading at all is the caller's to decide
/// ([`verify`]).
pub fn download(ctx: &Ctx<'_>, spec: &Spec<'_>, dest: &Path, progress: &dyn Fn(u64)) -> Result<()> {
    let part = part_of(dest);
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| disk(dir, "create the folder", &e))?;
    }
    let mut tries = 0u32;
    // A part whose checksum comes out wrong is thrown away and fetched once more, whole.
    let mut refetched = false;
    loop {
        if (ctx.cancelled)() {
            return Err(Error::Cancelled);
        }
        let mut file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&part).map_err(|e| disk(&part, "write", &e))?;
        let mut existing = file.metadata().map_err(|e| disk(&part, "read", &e))?.len();
        if existing > spec.size {
            // Not a part of this file.
            file.set_len(0).map_err(|e| disk(&part, "write", &e))?;
            let _ = std::fs::remove_file(validator_of(&part));
            existing = 0;
        }
        let needed = spec.size.saturating_sub(existing);
        if let Some(free) = (ctx.free_space)(&part).filter(|free| *free < needed.saturating_add(FILE_HEADROOM)) {
            let drive = part.ancestors().last().map(|p| p.display().to_string()).unwrap_or_default();
            return Err(Error::Disk(format!(
                "Not enough room on {drive}: {} is free, and `{}` needs {} more. Free some space, or choose another models folder, then continue.",
                human_bytes(free),
                dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                human_bytes(needed)
            )));
        }
        let mut hasher = Sha256::new();
        hash_prefix(&mut file, &part, existing, &mut hasher, ctx.cancelled, &|_| {})?;
        let validator = std::fs::read_to_string(validator_of(&part)).ok().filter(|v| !v.trim().is_empty() && v.len() <= 512);
        let mut sink = Part { file, path: &part, hasher, written: existing, size: spec.size, progress };
        (sink.progress)(existing);
        let fetched = if existing == spec.size {
            Ok(())
        } else {
            let request = Request { url: spec.url, token: spec.token, offset: existing, validator: validator.as_deref().map(str::trim) };
            ctx.fetch.fetch(&request, &mut sink, ctx.cancelled)
        };
        let (written, digest) = (sink.written, hex(&sink.hasher.clone().finalize()));
        let short = fetched.is_ok() && written < spec.size;
        match fetched {
            Ok(()) if !short => {}
            // What arrived is kept; the next try continues from it.
            Ok(()) | Err(Error::Network(_)) => {
                let why = match fetched {
                    Err(Error::Network(why)) => why,
                    _ => "the connection closed before the file was whole".to_string(),
                };
                // Progress means the network works: only failures in a row count.
                tries = if written > existing { 0 } else { tries.saturating_add(1) };
                if tries >= MAX_TRIES {
                    return Err(Error::Network(format!("The download kept failing ({why}). What arrived is kept: check the connection, then continue.")));
                }
                drop(sink);
                pause(ctx, Duration::from_secs(1u64 << tries.min(5)))?;
                continue;
            }
            Err(Error::Integrity(why)) => {
                drop(sink);
                remove_part(&part);
                return Err(Error::Integrity(why));
            }
            Err(e) => return Err(e),
        }
        sink.file.sync_all().map_err(|e| disk(&part, "write", &e))?;
        drop(sink);
        if digest != spec.sha256.to_ascii_lowercase() {
            remove_part(&part);
            if refetched {
                return Err(Error::Integrity(format!(
                    "`{}` arrived twice with the wrong checksum, so it was not kept. Expected sha256 {}, got {digest}. The file may have changed upstream, or something on the way is altering downloads.",
                    dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                    spec.sha256
                )));
            }
            refetched = true;
            tries = 0;
            continue;
        }
        // A leftover under the final name (a wrong length, a failed check) gives way.
        if dest.exists() {
            std::fs::remove_file(dest).map_err(|e| disk(dest, "replace", &e))?;
        }
        std::fs::rename(&part, dest).map_err(|e| disk(dest, "finish", &e))?;
        let _ = std::fs::remove_file(validator_of(&part));
        progress(spec.size);
        return Ok(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::Head;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

    /// What the fake does with the next request.
    #[derive(Clone, Debug)]
    enum Act {
        /// Serve the rest of the file from the offset asked for.
        Serve,
        /// Serve `n` bytes from the offset, then lose the connection.
        Cut(usize),
        /// Fail without sending anything.
        Fail,
        /// Send the whole file again, whatever was asked for.
        Whole,
        /// Say the file has another length.
        OtherLength(u64),
        /// Send the right amount of the wrong bytes.
        Corrupt,
        /// End early, as if that were the whole body.
        Short(usize),
        Refuse,
        /// Send more than the file.
        TooMuch,
    }

    struct Fake {
        data: Vec<u8>,
        script: Mutex<Vec<Act>>,
        /// (offset, validator) of each request.
        asked: Mutex<Vec<(u64, Option<String>)>>,
    }

    impl Fake {
        fn new(data: &[u8], script: &[Act]) -> Self {
            Self { data: data.to_vec(), script: Mutex::new(script.iter().rev().cloned().collect()), asked: Mutex::new(Vec::new()) }
        }

        fn offsets(&self) -> Vec<u64> {
            self.asked.lock().unwrap().iter().map(|a| a.0).collect()
        }
    }

    impl Fetch for Fake {
        fn head(&self, _: &str, _: Option<&str>) -> Result<Head> {
            Ok(Head { status: 200, size: Some(self.data.len() as u64), sha256: None })
        }

        fn fetch(&self, request: &Request<'_>, sink: &mut dyn Sink, _: &dyn Fn() -> bool) -> Result<()> {
            self.asked.lock().unwrap().push((request.offset, request.validator.map(str::to_string)));
            let act = self.script.lock().unwrap().pop().unwrap_or(Act::Serve);
            let total = Some(self.data.len() as u64);
            let from = request.offset as usize;
            let begin = |offset: u64| Begin { offset, total, validator: "\"v1\"".into() };
            match act {
                Act::Serve => {
                    sink.begin(&begin(request.offset))?;
                    sink.write(&self.data[from..])
                }
                Act::Cut(n) => {
                    sink.begin(&begin(request.offset))?;
                    sink.write(&self.data[from..(from + n).min(self.data.len())])?;
                    Err(Error::Network("the connection broke".into()))
                }
                Act::Short(n) => {
                    sink.begin(&begin(request.offset))?;
                    sink.write(&self.data[from..(from + n).min(self.data.len())])
                }
                Act::Fail => Err(Error::Network("no route to host".into())),
                Act::Whole => {
                    sink.begin(&begin(0))?;
                    sink.write(&self.data)
                }
                Act::OtherLength(n) => sink.begin(&Begin { offset: request.offset, total: Some(n), validator: String::new() }),
                Act::Corrupt => {
                    sink.begin(&begin(request.offset))?;
                    sink.write(&vec![0xAA; self.data.len() - from])
                }
                Act::Refuse => Err(crate::fetch::status_error(401, request.url)),
                Act::TooMuch => {
                    sink.begin(&begin(request.offset))?;
                    sink.write(&self.data[from..])?;
                    sink.write(b"extra")
                }
            }
        }
    }

    fn data() -> Vec<u8> {
        (0..200_000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8).collect()
    }

    fn sha(bytes: &[u8]) -> String {
        hex(&Sha256::digest(bytes))
    }

    struct Run {
        dir: tempfile::TempDir,
        dest: PathBuf,
        slept: AtomicU64,
        seen: AtomicU64,
    }

    impl Run {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let dest = dir.path().join("vae").join("ae.safetensors");
            Self { dir, dest, slept: AtomicU64::new(0), seen: AtomicU64::new(0) }
        }

        fn go_with(&self, fake: &Fake, sha256: &str, cancelled: &(dyn Fn() -> bool + Sync), free: Option<u64>) -> Result<()> {
            let sleep = |d: Duration| {
                self.slept.fetch_add(d.as_millis() as u64, Ordering::SeqCst);
            };
            let ctx = Ctx { fetch: fake, cancelled, sleep: &sleep, free_space: &|_| free };
            let spec = Spec { url: "https://huggingface.co/x/resolve/main/ae.safetensors", size: fake.data.len() as u64, sha256, token: None };
            download(&ctx, &spec, &self.dest, &|n| self.seen.store(n, Ordering::SeqCst))
        }

        fn go(&self, fake: &Fake) -> Result<()> {
            self.go_with(fake, &sha(&fake.data), &|| false, None)
        }

        fn part(&self) -> PathBuf {
            part_of(&self.dest)
        }
    }

    #[test]
    fn a_file_arrives_under_its_final_name_only_when_its_checksum_is_right() {
        let run = Run::new();
        let fake = Fake::new(&data(), &[]);
        run.go(&fake).unwrap();
        assert_eq!(std::fs::read(&run.dest).unwrap(), data());
        assert!(!run.part().exists() && !validator_of(&run.part()).exists());
        assert_eq!(run.seen.load(Ordering::SeqCst), data().len() as u64);
        assert!(verify(&run.dest, data().len() as u64, &sha(&data()), &|| false, &|_| {}).unwrap());
        assert!(!verify(&run.dest, data().len() as u64, &sha(b"other"), &|| false, &|_| {}).unwrap());
        assert!(!verify(&run.dest, 5, &sha(&data()), &|| false, &|_| {}).unwrap());
        assert!(!verify(&run.dir.path().join("nope"), 5, "", &|| false, &|_| {}).unwrap());
    }

    #[test]
    fn a_broken_connection_is_continued_from_what_arrived() {
        let run = Run::new();
        let fake = Fake::new(&data(), &[Act::Cut(50_000), Act::Fail, Act::Short(30_000), Act::Serve]);
        run.go(&fake).unwrap();
        assert_eq!(std::fs::read(&run.dest).unwrap(), data());
        // Each try asks for the rest, with the validator of the file the part came from.
        assert_eq!(fake.offsets(), [0, 50_000, 50_000, 80_000]);
        assert_eq!(fake.asked.lock().unwrap()[1].1.as_deref(), Some("\"v1\""));
        assert!(run.slept.load(Ordering::SeqCst) >= 2000, "it waits between tries");
    }

    #[test]
    fn a_download_killed_half_way_resumes_after_a_restart() {
        let run = Run::new();
        let fake = Fake::new(&data(), &[Act::Cut(120_000)]);
        // Cancelled right after the connection broke: the part stays.
        let tries = AtomicU32::new(0);
        let e = run.go_with(&fake, &sha(&data()), &|| tries.fetch_add(1, Ordering::SeqCst) >= 2, None).unwrap_err();
        assert_eq!(e, Error::Cancelled);
        assert_eq!(std::fs::metadata(run.part()).unwrap().len(), 120_000);
        assert!(!run.dest.exists());
        // A new process: the part is hashed again, and only the rest is asked for.
        let again = Fake::new(&data(), &[]);
        run.go(&again).unwrap();
        assert_eq!(again.offsets(), [120_000]);
        assert_eq!(std::fs::read(&run.dest).unwrap(), data());
    }

    #[test]
    fn a_file_that_changed_upstream_restarts_instead_of_being_stitched() {
        let run = Run::new();
        std::fs::create_dir_all(run.dest.parent().unwrap()).unwrap();
        std::fs::write(run.part(), vec![7u8; 90_000]).unwrap();
        let fake = Fake::new(&data(), &[Act::Whole]);
        run.go(&fake).unwrap();
        assert_eq!(std::fs::read(&run.dest).unwrap(), data());
        // A part longer than the file is not a part of it.
        let run = Run::new();
        std::fs::create_dir_all(run.dest.parent().unwrap()).unwrap();
        std::fs::write(run.part(), vec![7u8; 900_000]).unwrap();
        let fake = Fake::new(&data(), &[]);
        run.go(&fake).unwrap();
        assert_eq!((fake.offsets(), std::fs::read(&run.dest).unwrap()), (vec![0], data()));
        // A part that is already the whole file is checked and finished without a request.
        let run = Run::new();
        std::fs::create_dir_all(run.dest.parent().unwrap()).unwrap();
        std::fs::write(run.part(), data()).unwrap();
        let fake = Fake::new(&data(), &[]);
        run.go(&fake).unwrap();
        assert!(fake.offsets().is_empty() && run.dest.exists());
    }

    #[test]
    fn a_wrong_checksum_is_fetched_once_more_then_refused_with_both_hashes() {
        let run = Run::new();
        // A stale part with the right length but other bytes: thrown away, fetched whole.
        std::fs::create_dir_all(run.dest.parent().unwrap()).unwrap();
        std::fs::write(run.part(), vec![1u8; data().len()]).unwrap();
        let fake = Fake::new(&data(), &[]);
        run.go(&fake).unwrap();
        assert_eq!((fake.offsets(), std::fs::read(&run.dest).unwrap()), (vec![0], data()));
        // Wrong twice: nothing is kept, and the message is worth pasting into a bug report.
        let run = Run::new();
        let fake = Fake::new(&data(), &[Act::Corrupt, Act::Corrupt]);
        let e = run.go(&fake).unwrap_err();
        let Error::Integrity(why) = &e else { panic!("{e:?}") };
        assert!(why.contains(&sha(&data())) && why.contains(&sha(&vec![0xAA; data().len()])) && why.contains("ae.safetensors"), "{why}");
        assert!(!run.dest.exists() && !run.part().exists());
        assert_eq!(fake.offsets(), [0, 0]);
    }

    #[test]
    fn a_network_that_stays_down_pauses_and_keeps_the_part() {
        let run = Run::new();
        let fake = Fake::new(&data(), &[Act::Cut(10_000), Act::Fail, Act::Fail, Act::Fail, Act::Fail, Act::Fail, Act::Fail]);
        let e = run.go(&fake).unwrap_err();
        assert!(matches!(&e, Error::Network(m) if m.contains("kept failing") && m.contains("continue")), "{e:?}");
        assert_eq!(std::fs::metadata(run.part()).unwrap().len(), 10_000);
        // The try that made progress does not count against the limit.
        assert_eq!(fake.offsets().len() as u32, MAX_TRIES + 1);
    }

    #[test]
    fn refusals_and_lies_about_the_file_are_not_retried() {
        let run = Run::new();
        let fake = Fake::new(&data(), &[Act::Refuse]);
        assert!(matches!(run.go(&fake).unwrap_err(), Error::Access(m) if m.contains("gated")));
        assert_eq!(fake.offsets(), [0]);
        let fake = Fake::new(&data(), &[Act::OtherLength(12)]);
        assert!(matches!(run.go(&fake).unwrap_err(), Error::Integrity(m) if m.contains("changed upstream")));
        assert!(!run.part().exists());
        let fake = Fake::new(&data(), &[Act::TooMuch]);
        assert!(matches!(run.go(&fake).unwrap_err(), Error::Integrity(m) if m.contains("more than")));
        assert!(!run.dest.exists());
        assert_eq!(run.slept.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_full_drive_stops_before_the_write_and_names_what_is_needed() {
        let run = Run::new();
        let fake = Fake::new(&data(), &[]);
        let e = run.go_with(&fake, &sha(&data()), &|| false, Some(1000)).unwrap_err();
        let Error::Disk(why) = &e else { panic!("{e:?}") };
        assert!(why.contains("Not enough room") && why.contains("ae.safetensors") && why.contains("200 KB"), "{why}");
        assert!(fake.offsets().is_empty(), "nothing was asked for");
        // Enough room: it goes ahead.
        run.go_with(&fake, &sha(&data()), &|| false, Some(u64::MAX)).unwrap();
        // A wrong-length leftover under the final name is replaced.
        std::fs::write(&run.dest, b"stale").unwrap();
        run.go(&Fake::new(&data(), &[])).unwrap();
        assert_eq!(std::fs::read(&run.dest).unwrap(), data());
    }

    #[test]
    fn cancelling_keeps_what_arrived() {
        let run = Run::new();
        let fake = Fake::new(&data(), &[]);
        assert_eq!(run.go_with(&fake, &sha(&data()), &|| true, None).unwrap_err(), Error::Cancelled);
        assert!(fake.offsets().is_empty());
        assert!(free_space(run.dir.path()).is_some_and(|free| free > 0));
        assert!(free_space(&run.dir.path().join("not").join("yet").join("there")).is_some());
    }
}
