//! `open` and `xdg-open`, for files: each one is copied to the machine you ssh in from and opened
//! there (or, with `open -R`, revealed in its Finder). That machine decides what it will open (documents and images, never programs), names the
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
    // On a macOS remote, `open` is also how programs start apps and URLs on that machine (`open -na
    // "Google Chrome.app" --args ...` for a browser to automate), which must stay there. Only plain
    // files, and `open -R` of files, are for the person at the other end.
    let real_here = cfg!(target_os = "macos") && name == "open";
    let (reveal, files) = match args.first() {
        Some(first) if first == "-R" => (true, &args[1..]),
        _ => (false, &args[..]),
    };
    let forwarded = !files.is_empty()
        && files.iter().all(|a| {
            let text = a.to_string_lossy();
            !text.starts_with('-') && !is_url(&text) && Path::new(a).is_file()
        });
    if !forwarded && real_here {
        return link::run_real(name, &args);
    }
    if files.is_empty() || files.iter().any(|a| a.to_string_lossy().starts_with('-')) {
        eprintln!("usage: {name} [-R] <file>...  (opens or reveals each file on the machine you ssh in from)");
        return 2;
    }
    let mut status = 0;
    for arg in files {
        if let Err(e) = open(Path::new(arg), reveal) {
            if e == NOT_CONNECTED && (name == "xdg-open" || real_here) {
                return link::run_real(name, &args);
            }
            eprintln!("{name}: {}: {e}", arg.to_string_lossy());
            status = 1;
        }
    }
    status
}

fn is_url(text: &str) -> bool {
    text.contains("://") || text.starts_with("mailto:")
}

const NOT_CONNECTED: &str = "no rclaude connection is open to the machine you ssh in from";

fn open(path: &Path, reveal: bool) -> Result<(), String> {
    if is_url(&path.to_string_lossy()) {
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
    let verb = if reveal { "reveal" } else { "open" };
    let stream = link::request(&format!("{verb} {} {name}", body.len()), &body, Duration::from_secs(60))
        .ok_or_else(|| NOT_CONNECTED.to_string())?;
    let mut reply = String::new();
    let mut stream = stream;
    stream.read_line(&mut reply).map_err(|e| e.to_string())?;
    match reply.trim_end().split_once(' ') {
        Some(("ok", saved)) => {
            let done = if reveal { "Revealed" } else { "Opened" };
            println!("{done} {name} on the other machine, saved as {saved}");
            Ok(())
        }
        Some(("error", why)) => Err(why.to_string()),
        _ => Err("the other machine did not answer; is rclaude still connected?".into()),
    }
}
