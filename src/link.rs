//! The link back to the machine you ssh in from. Each `rclaude` connection forwards a loopback port
//! on this host to a responder there, and records it as `~/.rclaude-links/<port>`, holding a token.
//! A request is one connection: the line `<token> <verb> [args]`, the responder's hello line
//! (`rclaude 1`), then the body if any, then the reply. The programs that use it, `xclip` and
//! `open`, are this binary run under those names.
//!
//! Several connections can be open at once, one per pane, and one that has ended leaves its file
//! behind, so a request tries the newest first and moves on from one that does not answer. A port
//! nothing listens on is a connection that has ended, and its file is removed.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

const LINKS_DIR: &str = ".rclaude-links";
/// Written by launchers before per-connection files: one link, no hello line.
const LEGACY_LINK_FILE: &str = ".rclaude-link";
const HELLO: &str = "rclaude 1";

struct Link {
    port: u16,
    token: String,
    /// The file to remove when nothing listens on the port.
    file: Option<PathBuf>,
    /// Whether the responder answers with the hello line.
    hello: bool,
}

/// A request sent, its stream ready for the reply; None when no `rclaude` connection answers.
pub fn request(verb: &str, body: &[u8], read_timeout: Duration) -> Option<BufReader<TcpStream>> {
    links().into_iter().find_map(|link| try_link(&link, verb, body, read_timeout))
}

fn try_link(link: &Link, verb: &str, body: &[u8], read_timeout: Duration) -> Option<BufReader<TcpStream>> {
    let addr = SocketAddr::from(([127, 0, 0, 1], link.port));
    let mut stream = match TcpStream::connect_timeout(&addr, Duration::from_secs(2)) {
        Ok(stream) => stream,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::ConnectionRefused
                && let Some(file) = &link.file
            {
                let _ = std::fs::remove_file(file);
            }
            return None;
        }
    };
    stream.set_read_timeout(Some(read_timeout)).ok()?;
    stream.write_all(format!("{} {verb}\n", link.token).as_bytes()).ok()?;
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    if link.hello {
        let mut line = String::new();
        reader.read_line(&mut line).ok()?;
        if line.trim_end() != HELLO {
            return None;
        }
    }
    stream.write_all(body).ok()?;
    stream.shutdown(std::net::Shutdown::Write).ok()?;
    Some(reader)
}

/// Every recorded link, newest first, then the legacy one.
fn links() -> Vec<Link> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return Vec::new() };
    let mut links: Vec<(std::time::SystemTime, Link)> = std::fs::read_dir(home.join(LINKS_DIR))
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let port = entry.file_name().to_str()?.parse().ok()?;
            let token = std::fs::read_to_string(entry.path()).ok()?.trim().to_string();
            let modified = entry.metadata().and_then(|m| m.modified()).ok()?;
            (!token.is_empty()).then(|| (modified, Link { port, token, file: Some(entry.path()), hello: true }))
        })
        .collect();
    links.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    let mut links: Vec<Link> = links.into_iter().map(|(_, link)| link).collect();
    if let Some(legacy) = legacy_link(&home) {
        links.push(legacy);
    }
    links
}

fn legacy_link(home: &Path) -> Option<Link> {
    let text = std::fs::read_to_string(home.join(LEGACY_LINK_FILE)).ok()?;
    let mut parts = text.split_whitespace();
    Some(Link { port: parts.next()?.parse().ok()?, token: parts.next()?.to_string(), file: None, hello: false })
}

/// Hands the call to the next program called `name` on PATH that is not this one; the status of a
/// missing program when there is none.
pub fn run_real(name: &str, args: &[OsString]) -> i32 {
    let me = std::env::current_exe().ok().and_then(|p| p.canonicalize().ok());
    let path = std::env::var_os("PATH").unwrap_or_default();
    let real = std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|c| c.is_file() && c.canonicalize().ok() != me && !is_this_program(c));
    match real {
        Some(real) => {
            let err = std::process::Command::new(&real).args(args).exec();
            eprintln!("{name}: cannot run {}: {err}", real.display());
            126
        }
        None => 127,
    }
}

/// A link to this program under another name, as `rclaude` installs them.
fn is_this_program(candidate: &Path) -> bool {
    candidate.canonicalize().ok().and_then(|p| p.file_name().map(|n| n == "remote-claude")).unwrap_or(false)
}
