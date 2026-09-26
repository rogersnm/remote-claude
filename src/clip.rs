//! `xclip`, as far as Claude Code uses it to paste an image, answered from the clipboard of the
//! machine you ssh in from. Claude Code on Linux runs `xclip -selection clipboard -t TARGETS -o` to
//! ask whether the clipboard holds an image, then `xclip -selection clipboard -t image/png -o` to
//! read it. Anything else, and everything while no `rclaude` connection is open, goes to the real
//! xclip.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::time::Duration;

use crate::link;

/// A clipboard image larger than this is refused rather than buffered.
const MAX_IMAGE: u64 = 64 << 20;
const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

/// Runs as `xclip`; returns the exit status.
pub fn main(args: Vec<OsString>) -> i32 {
    let target = target_of(&args);
    let wants_output = args.iter().any(|a| a == "-o" || a == "-out");
    let targets = match target.as_deref() {
        Some("TARGETS") if wants_output => true,
        Some("image/png") if wants_output => false,
        _ => return link::run_real("xclip", &args),
    };
    let Some(stream) = link::request("clip", b"", Duration::from_secs(10)) else {
        return link::run_real("xclip", &args);
    };
    let mut image = Vec::new();
    let read = stream.take(MAX_IMAGE + 1).read_to_end(&mut image);
    if read.is_err() || image.len() as u64 > MAX_IMAGE || !image.starts_with(PNG_MAGIC) {
        return 1;
    }
    let mut out = std::io::stdout().lock();
    let written = if targets { out.write_all(b"image/png\n") } else { out.write_all(&image) };
    if written.and_then(|_| out.flush()).is_ok() { 0 } else { 1 }
}

/// The value of `-t`/`-target`.
fn target_of(args: &[OsString]) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "-t" || a == "-target" {
            return it.next().map(|t| t.to_string_lossy().into_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &[&str]) -> Vec<OsString> {
        s.iter().map(OsString::from).collect()
    }

    #[test]
    fn reads_the_target() {
        assert_eq!(target_of(&args(&["-selection", "clipboard", "-t", "TARGETS", "-o"])).as_deref(), Some("TARGETS"));
        assert_eq!(target_of(&args(&["-selection", "clipboard", "-o"])), None);
    }
}
