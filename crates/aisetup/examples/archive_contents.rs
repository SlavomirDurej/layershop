//! What a ComfyUI portable archive holds, from its header alone:
//! `cargo run -p photocraft-aisetup --example archive_contents -- <archive.7z> [path to look for…]`
//!
//! Prints the numbers the manifest records for a build (`unpackedBytes`, `longestPath`) and
//! whether the unpacker accepts the archive. The archive's data is not read, so a file holding
//! only its first 32 bytes and its last few megabytes (the header) is enough.

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: archive_contents <archive.7z> [path to look for…]");
        std::process::exit(2);
    };
    match photocraft_aisetup::unpack::contents(std::path::Path::new(&path), &photocraft_aisetup::unpack::Limits::default()) {
        Ok(c) => {
            println!("root: {:?}", c.root);
            println!("files: {}  folders: {}", c.files, c.folders);
            println!("unpackedBytes: {}", c.bytes);
            println!("longestPath: {}", c.longest_path);
            println!("methods: {:?}", c.methods);
            let mut top: Vec<&str> = c.paths.iter().filter(|p| !p.contains('/')).map(String::as_str).collect();
            top.sort_unstable();
            println!("top level: {top:?}");
            if let Some(longest) = c.paths.iter().max_by_key(|p| p.chars().count()) {
                println!("longest: {longest}");
            }
            for wanted in args {
                println!("{wanted}: {}", if c.paths.contains(&wanted) { "present" } else { "MISSING" });
            }
        }
        Err(e) => {
            eprintln!("refused: {e}");
            std::process::exit(1);
        }
    }
}
