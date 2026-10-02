//! `remote-claude paste-sync <pane>`: ctrl+v on a macOS remote, bound in tmux by `rclaude --on-host`.
//!
//! Claude Code on macOS reads a pasted image natively from the remote's own pasteboard, falling
//! back to `osascript` only when that read fails rather than when it finds no image, so the
//! `osascript` shim never sees the call. The image is instead put on the remote's pasteboard at the
//! moment it is pasted: this fetches the clipboard image of the machine you ssh in from, sets it as
//! the remote's clipboard, then sends the ctrl+v on to the pane. Without an image there, or without
//! a connection, the remote's clipboard is left alone and the key is sent all the same.

use std::process::Command;

use crate::clip::{Fetched, fetch_png};

/// The real osascript, by path: the one on PATH may be this program.
const OSASCRIPT: &str = "/usr/bin/osascript";

pub fn main(pane: Option<&str>) -> i32 {
    if let Fetched::Png(image) = fetch_png()
        && let Err(e) = set_clipboard(&image)
    {
        eprintln!("paste-sync: {e}");
    }
    match pane {
        Some(pane) => match Command::new("tmux").args(["send-keys", "-t", pane, "C-v"]).status() {
            Ok(status) if status.success() => 0,
            _ => 1,
        },
        None => 0,
    }
}

fn set_clipboard(image: &[u8]) -> Result<(), String> {
    let file = std::env::temp_dir().join(format!("rclaude-paste-{}.png", std::process::id()));
    std::fs::write(&file, image).map_err(|e| format!("cannot write {}: {e}", file.display()))?;
    let path = file.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!("set the clipboard to (read (POSIX file \"{path}\") as «class PNGf»)");
    let status = Command::new(OSASCRIPT).args(["-e", &script]).status();
    let _ = std::fs::remove_file(&file);
    match status {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!("osascript exited with {status}")),
        Err(e) => Err(format!("cannot run {OSASCRIPT}: {e}")),
    }
}
