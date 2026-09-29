//! `osascript`, as far as Claude Code uses it to paste an image on macOS, answered from the
//! clipboard of the machine you ssh in from: the macOS counterpart of `clip.rs`. Claude Code runs
//! `osascript -e 'the clipboard as «class PNGf»'` to ask whether the clipboard holds an image,
//! then
//!
//! ```text
//! osascript -e 'set png_data to (the clipboard as «class PNGf»)'
//!           -e 'set fp to open for access POSIX file "<path>" with write permission'
//!           -e 'write png_data to fp' -e 'close access fp'
//! ```
//!
//! to save it to a file it then reads. Any other script, and every call while no `rclaude`
//! connection is open, goes to the real osascript.

use std::ffi::OsString;

use crate::clip::{Fetched, fetch_png};
use crate::link;

const CHECK: &str = "the clipboard as «class PNGf»";
const SAVE_FIRST: &str = "set png_data to (the clipboard as «class PNGf»)";
const SAVE_WRITE: &str = "write png_data to fp";
const SAVE_CLOSE: &str = "close access fp";
const OPEN_PREFIX: &str = "set fp to open for access POSIX file \"";
const OPEN_SUFFIX: &str = "\" with write permission";

#[derive(Debug, PartialEq)]
enum Call {
    Check,
    Save(String),
}

/// Runs as `osascript`; returns the exit status.
pub fn main(args: Vec<OsString>) -> i32 {
    let Some(call) = recognise(&args) else {
        return link::run_real("osascript", &args);
    };
    let image = match fetch_png() {
        Fetched::NotConnected => return link::run_real("osascript", &args),
        Fetched::NoImage => return 1,
        Fetched::Png(image) => image,
    };
    match call {
        Call::Check => 0,
        Call::Save(path) => match std::fs::write(&path, &image) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("osascript: cannot write {path}: {e}");
                1
            }
        },
    }
}

/// Which of Claude Code's two clipboard calls this is, if either: nothing but `-e` scripts, in
/// exactly the shape above.
fn recognise(args: &[OsString]) -> Option<Call> {
    let mut scripts = Vec::new();
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        if flag != "-e" {
            return None;
        }
        scripts.push(it.next()?.to_str()?);
    }
    match scripts.as_slice() {
        [CHECK] => Some(Call::Check),
        [SAVE_FIRST, open, SAVE_WRITE, SAVE_CLOSE] => {
            let quoted = open.strip_prefix(OPEN_PREFIX)?.strip_suffix(OPEN_SUFFIX)?;
            Some(Call::Save(unescape(quoted)?))
        }
        _ => None,
    }
}

/// An AppleScript string's contents, as Claude Code escapes a path into one: `\\` and `\"`.
fn unescape(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push(chars.next().filter(|n| *n == '\\' || *n == '"')?),
            '"' => return None,
            c => out.push(c),
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &[&str]) -> Vec<OsString> {
        s.iter().map(OsString::from).collect()
    }

    fn save(path_literal: &str) -> Vec<OsString> {
        let open = format!("{OPEN_PREFIX}{path_literal}{OPEN_SUFFIX}");
        args(&["-e", SAVE_FIRST, "-e", &open, "-e", SAVE_WRITE, "-e", SAVE_CLOSE])
    }

    #[test]
    fn recognises_the_check() {
        assert_eq!(recognise(&args(&["-e", CHECK])), Some(Call::Check));
    }

    #[test]
    fn recognises_the_save_and_its_path() {
        let path = "/var/folders/xy/T/claude_cli_latest_screenshot.png";
        assert_eq!(recognise(&save(path)), Some(Call::Save(path.to_string())));
    }

    #[test]
    fn unescapes_the_path_as_claude_code_escapes_it() {
        assert_eq!(recognise(&save(r#"/tmp/a \"b\" \\c.png"#)), Some(Call::Save(r#"/tmp/a "b" \c.png"#.to_string())));
    }

    #[test]
    fn leaves_every_other_script_to_the_real_osascript() {
        assert_eq!(recognise(&args(&["-e", "display notification \"hi\""])), None);
        assert_eq!(recognise(&args(&["-e", CHECK, "-e", "beep"])), None);
        assert_eq!(recognise(&args(&["script.scpt"])), None);
        assert_eq!(recognise(&args(&["-e"])), None);
        assert_eq!(recognise(&args(&[])), None);
        // An unescaped quote would end the AppleScript string early: not Claude Code's shape.
        assert_eq!(recognise(&save(r#"/tmp/a"b.png"#)), None);
    }
}
