//! The link back to the machine you ssh in from. `rclaude` forwards a loopback port on this host
//! to a responder there and writes the port and a token to `~/.rclaude-link` on every connect;
//! each request is one connection that starts with the line `<token> <verb> [args]`. The programs
//! that use it, `xclip` and `open`, are this binary run under those names.

use std::ffi::OsString;
use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::time::Duration;

const LINK_FILE: &str = ".rclaude-link";

/// A request sent, its stream ready for the reply; None when no `rclaude` connection is open.
pub fn request(verb: &str, body: &[u8], read_timeout: Duration) -> Option<TcpStream> {
    let (port, token) = link()?;
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).ok()?;
    stream.set_read_timeout(Some(read_timeout)).ok()?;
    stream.write_all(format!("{token} {verb}\n").as_bytes()).ok()?;
    stream.write_all(body).ok()?;
    stream.shutdown(std::net::Shutdown::Write).ok()?;
    Some(stream)
}

/// The port and token from `~/.rclaude-link`.
fn link() -> Option<(u16, String)> {
    let home = std::env::var_os("HOME")?;
    let text = std::fs::read_to_string(Path::new(&home).join(LINK_FILE)).ok()?;
    let mut parts = text.split_whitespace();
    Some((parts.next()?.parse().ok()?, parts.next()?.to_string()))
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
