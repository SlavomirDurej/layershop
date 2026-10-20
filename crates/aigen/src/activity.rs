//! What a ComfyUI server is doing, in words, from the lines it last printed: so a step that
//! takes minutes (starting the engine, loading a model, rendering on the processor) says where it
//! is, rather than only that it is busy.

/// The lines of a server's output as a terminal shows them: colour codes removed, and a line a
/// progress bar rewrote in place (`\r`) as its last state. Empty lines are dropped.
pub fn lines(text: &str) -> Vec<String> {
    text.split('\n')
        .filter_map(|line| line.split('\r').map(str::trim_end).rfind(|part| !part.trim().is_empty()))
        .map(plain)
        .filter(|line| !line.trim().is_empty())
        .collect()
}

/// `line` without terminal colour codes (`ESC [ … m`).
pub fn plain(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // A control sequence: `[`, parameters, then a letter that ends it.
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// What the server is doing, going by the newest line that says (`lines` oldest first): "loading
/// PyTorch", "step 7 of 20, about 3:50 left". `None` when no line says anything known.
pub fn describe(lines: &[String]) -> Option<String> {
    lines.iter().rev().take(400).find_map(|line| what(line))
}

fn what(line: &str) -> Option<String> {
    let line = line.trim();
    if let Some(step) = steps(line) {
        return Some(step);
    }
    let known: &[(&str, &str)] = &[
        ("Prompt executed in", "finished the render"),
        ("loaded completely", "model loaded, rendering"),
        ("loaded partially", "model loaded in part, rendering (memory is tight)"),
        ("got prompt", "starting the render"),
        ("To see the GUI go to", "ready"),
        ("Starting server", "starting its web server"),
        ("Import times for custom nodes", "loading add-ons"),
        ("Checkpoint files will always be loaded safely", "loading its nodes"),
        ("comfy_kitchen backend", "loading PyTorch"),
        ("Total VRAM", "loading PyTorch"),
        ("Device:", "loading PyTorch"),
        ("pytorch version", "loading PyTorch"),
        ("Adding extra search path", "finding the models"),
        ("Python version", "starting Python"),
        ("ComfyUI version", "starting Python"),
        ("setup plugin", "starting Python"),
    ];
    if let Some(rest) = line.split_once("Requested to load ").map(|(_, rest)| rest.trim()) {
        let name: String = rest.chars().take(60).collect();
        return Some(if name.is_empty() { "loading the model into memory".into() } else { format!("loading {name} into memory") });
    }
    known.iter().find(|(says, _)| line.contains(says)).map(|(_, means)| (*means).to_string())
}

/// A progress bar's line (tqdm: ` 35%|███▌      | 7/20 [00:41<03:50,  5.20s/it]`): the step,
/// and the time left when it is known.
fn steps(line: &str) -> Option<String> {
    let (_, after_bar) = line.rsplit_once('|')?;
    let (count, timing) = after_bar.split_once('[')?;
    let (done, total) = count.trim().split_once('/')?;
    let (done, total) = (done.trim().parse::<u64>().ok()?, total.trim().parse::<u64>().ok()?);
    if total == 0 || done > total {
        return None;
    }
    let left = timing.split_once('<').map(|(_, rest)| rest.split([',', ']']).next().unwrap_or("").trim()).filter(|l| !l.is_empty() && !l.contains('?'));
    Some(match left {
        Some(left) if done < total => format!("step {done} of {total}, about {left} left"),
        _ => format!("step {done} of {total}"),
    })
}

/// `seconds` as a clock: `0:45`, `12:03`, `1:02:03`.
pub fn clock(seconds: u64) -> String {
    let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(text: &str) -> Option<String> {
        describe(&lines(text))
    }

    #[test]
    fn colour_codes_and_rewritten_lines_read_as_a_terminal_shows_them() {
        let out = lines("\u{1b}[32m[INFO]\u{1b}[0m Total VRAM 0 MB\r\n  5%|5    | 1/20 [00:05<01:40]\r 10%|#    | 2/20 [00:10<01:30]\r\n\n");
        assert_eq!(out, ["[INFO] Total VRAM 0 MB", " 10%|#    | 2/20 [00:10<01:30]"]);
        assert!(lines("").is_empty() && lines("\r\r\n\n").is_empty());
    }

    /// What the VM's engine printed while starting, newest last: each stage reads as one.
    #[test]
    fn starting_reads_as_its_stage() {
        assert_eq!(said("setup plugin alembic.autogenerate.schemas").as_deref(), Some("starting Python"));
        assert_eq!(said("Adding extra search path vae C:\\models\\vae").as_deref(), Some("finding the models"));
        assert_eq!(said("Found comfy_kitchen backend eager: {}").as_deref(), Some("loading PyTorch"));
        assert_eq!(said("Checkpoint files will always be loaded safely.").as_deref(), Some("loading its nodes"));
        assert_eq!(said("Import times for custom nodes:\n   0.0 seconds: C:\\x").as_deref(), Some("loading add-ons"));
        assert_eq!(said("Starting server\n\nTo see the GUI go to: http://127.0.0.1:8190").as_deref(), Some("ready"));
        // A line that says nothing known leaves the last stage that did.
        assert_eq!(said("Total VRAM 0 MB\nsomething else entirely").as_deref(), Some("loading PyTorch"));
        assert_eq!(said("nothing known here"), None);
    }

    #[test]
    fn rendering_reads_as_its_step_and_the_time_left() {
        assert_eq!(said("got prompt").as_deref(), Some("starting the render"));
        assert_eq!(said("Requested to load QwenImage").as_deref(), Some("loading QwenImage into memory"));
        assert_eq!(said("loaded completely 9.0 7000 True").as_deref(), Some("model loaded, rendering"));
        let bar = " 35%|\u{2588}\u{2588}\u{2588}\u{258c}      | 7/20 [00:41<03:50,  5.20s/it]";
        assert_eq!(said(bar).as_deref(), Some("step 7 of 20, about 03:50 left"));
        assert_eq!(said("100%|##########| 20/20 [01:40<00:00,  5.00s/it]").as_deref(), Some("step 20 of 20"));
        assert_eq!(said("  0%|          | 0/20 [00:00<?, ?it/s]").as_deref(), Some("step 0 of 20"));
        assert_eq!(said("Prompt executed in 101.52 seconds").as_deref(), Some("finished the render"));
        // Not a progress bar, though it has the marks of one.
        for odd in ["a|b 3/0 [x]", "a|b 9/4 [x]", "a|b x/y [x]", "|[", "a|b 3/", "|"] {
            assert_eq!(said(odd), None, "{odd}");
        }
    }

    #[test]
    fn a_clock_reads_like_one() {
        assert_eq!((clock(0), clock(45), clock(725), clock(3723)), ("0:00".into(), "0:45".into(), "12:05".into(), "1:02:03".into()));
    }
}
