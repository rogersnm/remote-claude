//! `open` and `xdg-open`, for files: each one is copied to the machine you ssh in from and opened
//! there. That machine decides what it will open (documents and images, never programs), names the
//! copy, and quarantines it; this side only sends. URLs are refused, and so is everything while no
//! `rclaude` connection is open, except that `xdg-open` then goes to the real one.

use std::ffi::OsString;
use std::io::BufRead;
use std::path::Path;
use std::time::Duration;

use crate::link;

/// Larger files are refused; the other side refuses them too.
const MAX_FILE: u64 = 100 << 20;

/// Runs as `open` or `xdg-open` (`name`); returns the exit status.
pub fn main(name: &str, args: Vec<OsString>) -> i32 {
    if args.is_empty() || args.iter().any(|a| a.to_string_lossy().starts_with('-')) {
        eprintln!("usage: {name} <file>...  (opens each file on the machine you ssh in from)");
        return 2;
    }
    let mut status = 0;
    for arg in &args {
        if let Err(e) = open(Path::new(arg)) {
            if e == NOT_CONNECTED && name == "xdg-open" {
                return link::run_real(name, &args);
            }
            eprintln!("{name}: {}: {e}", arg.to_string_lossy());
            status = 1;
        }
    }
    status
}

const NOT_CONNECTED: &str = "no rclaude connection is open to the machine you ssh in from";

fn open(path: &Path) -> Result<(), String> {
    let text = path.to_string_lossy();
    if text.contains("://") || text.starts_with("mailto:") {
        return Err("only files are opened on the other machine, not URLs".into());
    }
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("not a file".into());
    }
    if meta.len() > MAX_FILE {
        return Err(format!("larger than {} MB", MAX_FILE >> 20));
    }
    let name = path.file_name().map(|n| n.to_string_lossy().replace(['\n', '\r'], "_")).unwrap_or_default();
    let body = std::fs::read(path).map_err(|e| e.to_string())?;
    let stream = link::request(&format!("open {} {name}", body.len()), &body, Duration::from_secs(60))
        .ok_or_else(|| NOT_CONNECTED.to_string())?;
    let mut reply = String::new();
    let mut stream = stream;
    stream.read_line(&mut reply).map_err(|e| e.to_string())?;
    match reply.trim_end().split_once(' ') {
        Some(("ok", saved)) => {
            println!("Opened {name} on the other machine, saved as {saved}");
            Ok(())
        }
        Some(("error", why)) => Err(why.to_string()),
        _ => Err("the other machine did not answer; is rclaude still connected?".into()),
    }
}
