//! `cargo xtask ai-manifest`: compare the Local AI setup's manifest with the hosts.
//!
//! `crates/aisetup/manifest.json` pins every file the wizard downloads by size and sha256. Files
//! move and are replaced upstream, so this asks each host what it has now (a `HEAD` per file,
//! nothing is downloaded) and fails on any difference. Hugging Face reports a file's size and
//! sha256 in its headers; GitHub reports a release asset's in its API. A gated file answers 401
//! without a sign-in, whether or not it is still there, so it is compared through Hugging Face's
//! tree API instead, which lists a gated repository's files and sizes to anyone (the sha256 only
//! to someone signed in).
//!
//! Run it before a release and whenever the pinned ComfyUI version changes. The unpacked size
//! and longest path of a new ComfyUI build are measured with
//! `cargo run -p photocraft-aisetup --example archive_contents -- <archive.7z>`.

use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::pinned::USER_AGENT;

/// The response headers of `url`, following no redirects: the first answer is the one that
/// carries Hugging Face's `X-Linked-*` headers.
fn head(url: &str) -> Result<String, String> {
    let out = Command::new("curl")
        .args(["-sS", "-I", "--max-time", "40", "-A", USER_AGENT, url])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("curl: {e} (is curl installed?)"))?;
    if !out.status.success() {
        return Err(format!("curl failed for {url}: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn get(url: &str) -> Result<Vec<u8>, String> {
    let out = Command::new("curl")
        .args(["-sS", "-L", "--max-time", "40", "--max-filesize", "8000000", "-A", USER_AGENT, url])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("curl: {e} (is curl installed?)"))?;
    if !out.status.success() {
        return Err(format!("curl failed for {url}: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(out.stdout)
}

/// What a `HEAD` says: the status of the first answer, and a header's value by its lowercase name.
struct Headers(String);

impl Headers {
    fn status(&self) -> u16 {
        self.0.lines().next().and_then(|l| l.split_whitespace().nth(1)).and_then(|s| s.parse().ok()).unwrap_or(0)
    }

    fn get(&self, name: &str) -> Option<String> {
        self.0.lines().find_map(|l| {
            let (key, value) = l.split_once(':')?;
            key.trim().eq_ignore_ascii_case(name).then(|| value.trim().trim_matches('"').to_string())
        })
    }
}

/// A Hugging Face file address, `https://huggingface.co/<owner>/<repo>/resolve/<revision>/<path>`,
/// as the tree API's address for the file's folder, and the file's path in the repository.
fn tree_of(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("https://huggingface.co/")?;
    let mut parts = rest.splitn(5, '/');
    let (owner, repo, resolve, revision, path) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    if resolve != "resolve" || path.is_empty() {
        return None;
    }
    let folder = path.rsplit_once('/').map_or("", |(dir, _)| dir);
    Some((format!("https://huggingface.co/api/models/{owner}/{repo}/tree/{revision}/{folder}"), path.to_string()))
}

/// A gated file compared through the tree API: `None` when the folder could not be listed.
fn gated_line(key: &str, url: &str, size: u64, sha256: &str) -> Option<(String, bool)> {
    let (api, path) = tree_of(url)?;
    let listed: Value = serde_json::from_slice(&get(&api).ok()?).ok()?;
    let entries = listed.as_array()?;
    let Some(found) = entries.iter().find(|e| e.get("path").and_then(Value::as_str) == Some(path.as_str())) else {
        let others: Vec<&str> = entries.iter().filter_map(|e| e.get("path").and_then(Value::as_str)).filter(|p| p.ends_with(".safetensors")).collect();
        return Some((format!("MISSING  {key}: the repository has no {path} (it has {})", others.join(", ")), true));
    };
    let found_size = found.get("size").and_then(Value::as_u64);
    // Hugging Face shows a gated file's sha256 only to someone signed in (`****` otherwise): then
    // the size is what can be compared, and a rename or removal is still caught above.
    let found_sha = found.pointer("/lfs/oid").and_then(Value::as_str).filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()));
    if found_sha.is_none() {
        return Some(match found_size {
            Some(s) if s == size => (format!("ok       {key} (gated: the size, through the tree API; its sha256 shows only signed in)"), false),
            Some(s) => (format!("DIFFERS  {key}: size {s}, the manifest says {size}"), true),
            None => (format!("UNKNOWN  {key}: the tree API did not report its size"), true),
        });
    }
    let (line, differs) = compare(key, size, sha256, found_size, found_sha);
    Some((format!("{line} (gated: compared through the tree API)"), differs))
}

/// One line of the report, and whether it is a difference.
fn compare(what: &str, size: u64, sha256: &str, found_size: Option<u64>, found_sha: Option<&str>) -> (String, bool) {
    match (found_size, found_sha) {
        (Some(s), _) if s != size => (format!("DIFFERS  {what}: size {s}, the manifest says {size}"), true),
        (_, Some(h)) if !h.eq_ignore_ascii_case(sha256) => (format!("DIFFERS  {what}: sha256 {h}, the manifest says {sha256}"), true),
        (Some(_), Some(_)) => (format!("ok       {what}"), false),
        _ => (format!("UNKNOWN  {what}: the host did not report its size and sha256"), true),
    }
}

pub fn run(root: &Path) -> Result<(), String> {
    let path = root.join("crates/aisetup/manifest.json");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let manifest: Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut differences = 0usize;
    let mut gated = 0usize;

    // The runtime: GitHub's API lists each asset of the release with its size and digest.
    let version = manifest.pointer("/comfy/version").and_then(Value::as_str).ok_or("the manifest has no comfy.version")?;
    let assets = manifest.pointer("/comfy/assets").and_then(Value::as_object).ok_or("the manifest has no comfy.assets")?;
    let first = assets.values().next().and_then(|a| a.get("url")).and_then(Value::as_str).unwrap_or("");
    // https://github.com/<owner>/<repo>/releases/download/<tag>/<file>
    let mut parts = first.trim_start_matches("https://github.com/").split('/');
    let (owner, repo) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let api = format!("https://api.github.com/repos/{owner}/{repo}/releases/tags/v{version}");
    let release: Value = serde_json::from_slice(&get(&api)?).map_err(|e| format!("{api}: {e}"))?;
    let listed = release.get("assets").and_then(Value::as_array).ok_or_else(|| format!("{api}: no such release (is ComfyUI v{version} published?)"))?;
    println!("ComfyUI v{version} ({owner}/{repo})");
    for (name, asset) in assets {
        let url = asset.get("url").and_then(Value::as_str).unwrap_or("");
        let file = url.rsplit('/').next().unwrap_or("");
        let found = listed.iter().find(|a| a.get("name").and_then(Value::as_str) == Some(file));
        let found_size = found.and_then(|a| a.get("size")).and_then(Value::as_u64);
        let digest = found.and_then(|a| a.get("digest")).and_then(Value::as_str).and_then(|d| d.strip_prefix("sha256:"));
        let size = asset.get("size").and_then(Value::as_u64).unwrap_or(0);
        let sha = asset.get("sha256").and_then(Value::as_str).unwrap_or("");
        let (line, differs) = match found {
            Some(_) => compare(&format!("{name} ({file})"), size, sha, found_size, digest),
            None => (format!("MISSING  {name}: the release has no {file}"), true),
        };
        println!("  {line}");
        differences += usize::from(differs);
    }

    // The model files: Hugging Face says in the headers of a file's own address what it holds.
    println!("Model files");
    let files = manifest.get("files").and_then(Value::as_object).ok_or("the manifest has no files")?;
    for (key, file) in files {
        let url = file.get("url").and_then(Value::as_str).unwrap_or("");
        let size = file.get("size").and_then(Value::as_u64).unwrap_or(0);
        let sha = file.get("sha256").and_then(Value::as_str).unwrap_or("");
        let headers = Headers(head(url)?);
        let (line, differs) = match headers.status() {
            401 | 403 => {
                let marked = file.get("gated").and_then(Value::as_bool).unwrap_or(false);
                match gated_line(key, url, size, sha) {
                    Some((line, differs)) if marked => (line, differs),
                    Some((line, _)) => (format!("{line}; the manifest does not mark it gated"), true),
                    None => {
                        gated += 1;
                        (
                            format!(
                                "gated    {key}: not compared (its folder could not be listed){}",
                                if marked { "" } else { "; the manifest does not mark it gated" }
                            ),
                            !marked,
                        )
                    }
                }
            }
            // A renamed repository: the manifest should name where the file is now.
            301 | 307 | 308 if headers.get("x-linked-size").is_none() => {
                (format!("MOVED    {key}: now at {}", headers.get("location").unwrap_or_default()), true)
            }
            200 | 302 => {
                let found_size = headers.get("x-linked-size").and_then(|s| s.parse().ok());
                let found_sha = headers.get("x-linked-etag");
                let (line, differs) = compare(key, size, sha, found_size, found_sha.as_deref());
                let marked = file.get("gated").and_then(Value::as_bool).unwrap_or(false);
                if marked && !differs { (format!("{line} (marked gated, but it answered without a sign-in)"), true) } else { (line, differs) }
            }
            status => (format!("ERROR    {key}: the host answered {status}"), true),
        };
        println!("  {line}");
        differences += usize::from(differs);
    }
    println!("\n{} files, {} builds, {gated} gated and not compared", files.len(), assets.len());
    if differences > 0 {
        return Err(format!("{differences} entries of {} differ from what the hosts have", path.display()));
    }
    println!("The manifest matches the hosts. Set its \"checked\" date to today.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_are_read_from_the_first_answer_whatever_their_case() {
        let h = Headers("HTTP/1.1 302 Found\r\nX-Linked-Size: 335304388\r\nx-linked-etag: \"ABC\"\r\nLocation: https://cdn.example/x\r\n\r\n".into());
        assert_eq!(h.status(), 302);
        assert_eq!(h.get("x-linked-size").as_deref(), Some("335304388"));
        assert_eq!(h.get("X-Linked-ETag").as_deref(), Some("ABC"));
        assert_eq!(h.get("location").as_deref(), Some("https://cdn.example/x"));
        assert_eq!(h.get("etag"), None);
        assert_eq!(Headers(String::new()).status(), 0);
        assert_eq!(Headers("garbage".into()).status(), 0);
    }

    #[test]
    fn a_hugging_face_address_names_its_folder_in_the_tree_api() {
        let (api, path) = tree_of("https://huggingface.co/bfl/klein/resolve/main/diffusion_models/a.safetensors").unwrap();
        assert_eq!(api, "https://huggingface.co/api/models/bfl/klein/tree/main/diffusion_models");
        assert_eq!(path, "diffusion_models/a.safetensors");
        assert_eq!(tree_of("https://huggingface.co/bfl/klein/resolve/main/a.safetensors").unwrap().0, "https://huggingface.co/api/models/bfl/klein/tree/main/");
        assert_eq!(tree_of("https://huggingface.co/bfl/klein/blob/main/a.safetensors"), None);
        assert_eq!(tree_of("https://github.com/bfl/klein/resolve/main/a"), None);
    }

    #[test]
    fn a_difference_in_size_or_checksum_is_reported_and_an_unknown_one_too() {
        assert_eq!(compare("a", 10, "ab", Some(10), Some("AB")), ("ok       a".to_string(), false));
        assert!(compare("a", 10, "ab", Some(11), Some("ab")).1);
        assert!(compare("a", 10, "ab", Some(10), Some("cd")).0.contains("sha256 cd"));
        assert!(compare("a", 10, "ab", None, Some("ab")).1);
        assert!(compare("a", 10, "ab", Some(10), None).0.starts_with("UNKNOWN"));
    }
}
