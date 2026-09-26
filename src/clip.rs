//! `xclip`, as far as Claude Code uses it to paste an image, answered from the clipboard of the
//! machine you ssh in from. `rclaude --on-host` forwards a loopback port back to a responder there
//! and writes the port and a token to `~/.rclaude-clip`; Claude Code on this machine runs
//! `xclip -selection clipboard -t TARGETS -o` to ask whether the clipboard holds an image, then
//! `xclip -selection clipboard -t image/png -o` to read it. Anything else goes to the real xclip.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::time::Duration;

/// The file `rclaude --on-host` writes on every connect: `<port> <token>`.
const CONNECTION_FILE: &str = ".rclaude-clip";
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
        _ => return real_xclip(&args),
    };
    let image = match fetch() {
        Fetch::NotConnected => return real_xclip(&args),
        Fetch::NoImage => return 1,
        Fetch::Image(image) => image,
    };
    let mut out = std::io::stdout().lock();
    let written = if targets { out.write_all(b"image/png\n") } else { out.write_all(&image) };
    if written.and_then(|_| out.flush()).is_ok() { 0 } else { 1 }
}

enum Fetch {
    /// No `rclaude --on-host` session is forwarding a clipboard here.
    NotConnected,
    /// One is, and its clipboard holds no image (or the answer was unusable).
    NoImage,
    Image(Vec<u8>),
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

/// The image on the other machine's clipboard.
fn fetch() -> Fetch {
    let Some((port, token)) = connection() else { return Fetch::NotConnected };
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_secs(2)) else {
        return Fetch::NotConnected;
    };
    let mut image = Vec::new();
    let read = stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .and_then(|_| stream.write_all(format!("{token}\n").as_bytes()))
        .and_then(|_| stream.take(MAX_IMAGE + 1).read_to_end(&mut image));
    if read.is_ok() && image.len() as u64 <= MAX_IMAGE && image.starts_with(PNG_MAGIC) {
        Fetch::Image(image)
    } else {
        Fetch::NoImage
    }
}

/// The port and token from `~/.rclaude-clip`.
fn connection() -> Option<(u16, String)> {
    let home = std::env::var_os("HOME")?;
    let text = std::fs::read_to_string(Path::new(&home).join(CONNECTION_FILE)).ok()?;
    let mut parts = text.split_whitespace();
    Some((parts.next()?.parse().ok()?, parts.next()?.to_string()))
}

/// Hands the call to the next `xclip` on PATH that is not this one, or fails as a missing xclip would.
fn real_xclip(args: &[OsString]) -> i32 {
    let me = std::env::current_exe().ok().and_then(|p| p.canonicalize().ok());
    let path = std::env::var_os("PATH").unwrap_or_default();
    let real = std::env::split_paths(&path)
        .map(|dir| dir.join("xclip"))
        .find(|c| c.is_file() && c.canonicalize().ok() != me && !is_shim(c));
    match real {
        Some(real) => {
            let err = std::process::Command::new(&real).args(args).exec();
            eprintln!("xclip: cannot run {}: {err}", real.display());
            1
        }
        None => 1,
    }
}

/// A symlink to this program under another name, as `rclaude --on-host` installs it.
fn is_shim(candidate: &Path) -> bool {
    candidate.canonicalize().ok().and_then(|p| p.file_name().map(|n| n == "remote-claude")).unwrap_or(false)
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
