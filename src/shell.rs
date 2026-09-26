//! The shell tool: one `bash -c` per call with a working directory that persists
//! between calls, output capped, a timeout, and detached background jobs.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::io::AsyncReadExt;

/// Output past this is cut in the middle, keeping the head and the tail.
const MAX_OUTPUT: usize = 30_000;

pub struct Run {
    pub output: String,
    pub status: i32,
    pub timed_out: bool,
    /// Where the shell ended up, when it could be read.
    pub cwd: Option<PathBuf>,
}

/// `command` under bash in `cwd`; stdout and stderr together; the shell's final
/// directory read back through a file so `cd` persists to the next call.
pub async fn run(command: &str, cwd: &Path, timeout_ms: u64) -> Result<Run> {
    let marker = std::env::temp_dir().join(format!("remote-claude-cwd-{}-{}", std::process::id(), nanos()));
    let script = format!("{command}\n__rc=$?; pwd > {} 2>/dev/null; exit $__rc", shell_quote(&marker));
    let mut child = tokio::process::Command::new("bash")
        .arg("-c")
        .arg(&script)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .context("cannot start bash")?;
    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let out = tokio::spawn(async move {
        let mut v = Vec::new();
        let _ = stdout.read_to_end(&mut v).await;
        v
    });
    let err = tokio::spawn(async move {
        let mut v = Vec::new();
        let _ = stderr.read_to_end(&mut v).await;
        v
    });
    let pid = child.id().map(|p| p as i32);
    let (status, timed_out) = match tokio::time::timeout(Duration::from_millis(timeout_ms), child.wait()).await {
        Ok(status) => (status.context("bash did not report a status")?.code().unwrap_or(-1), false),
        Err(_) => {
            if let Some(pid) = pid {
                // The whole group: the command's own children too.
                unsafe { libc::kill(-pid, libc::SIGKILL) };
            }
            let _ = child.wait().await;
            (-1, true)
        }
    };
    let mut output = String::from_utf8_lossy(&out.await.unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&err.await.unwrap_or_default()).into_owned();
    if !stderr.is_empty() {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(&stderr);
    }
    let new_cwd = std::fs::read_to_string(&marker).ok().map(|s| PathBuf::from(s.trim())).filter(|p| p.is_dir());
    let _ = std::fs::remove_file(&marker);
    Ok(Run { output: cap(output), status, timed_out, cwd: new_cwd })
}

/// Detached background jobs: each runs under `setsid`, writes its output to a
/// file and its exit code to another, so it outlives the server process (and an
/// ssh drop) and any later server instance can still read it.
pub struct Jobs {
    dir: PathBuf,
}

pub struct Job {
    pub id: String,
    pub out: PathBuf,
}

impl Jobs {
    pub fn open() -> Result<Self> {
        let base = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let dir = base.join("remote-claude").join("jobs");
        std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
        Ok(Self { dir })
    }

    pub fn start(&self, command: &str, cwd: &Path, description: Option<&str>) -> Result<Job> {
        let id = format!("{:x}", nanos() & 0xffff_ffff);
        let out = self.dir.join(format!("{id}.out"));
        let status = self.dir.join(format!("{id}.status"));
        let meta = self.dir.join(format!("{id}.cmd"));
        std::fs::write(&meta, format!("{}\n{}\n", description.unwrap_or(""), command))?;
        // bash under setsid: its own session and group; the pid file names the
        // shell so `stop` can kill the whole group later.
        let script = format!(
            "echo $$ > {pidf}; ( {command} ) > {out} 2>&1; echo $? > {status}",
            pidf = shell_quote(&self.dir.join(format!("{id}.pid"))),
            out = shell_quote(&out),
            status = shell_quote(&status),
        );
        std::process::Command::new("setsid")
            .arg("bash")
            .arg("-c")
            .arg(&script)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("cannot start the background job (is setsid installed?)")?;
        Ok(Job { id, out })
    }

    fn status(&self, id: &str) -> String {
        match std::fs::read_to_string(self.dir.join(format!("{id}.status"))) {
            Ok(s) => format!("finished, exit code {}", s.trim()),
            Err(_) => "running".to_string(),
        }
    }

    pub fn list(&self) -> String {
        let mut ids: Vec<String> = std::fs::read_dir(&self.dir)
            .map(|d| {
                d.filter_map(|e| e.ok())
                    .filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".cmd")).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        ids.sort();
        if ids.is_empty() {
            return "no background jobs".to_string();
        }
        ids.iter()
            .map(|id| {
                let cmd = std::fs::read_to_string(self.dir.join(format!("{id}.cmd"))).unwrap_or_default();
                let mut lines = cmd.lines();
                let desc = lines.next().unwrap_or("");
                let command = lines.next().unwrap_or("");
                format!("{id}  {}  {}", self.status(id), if desc.is_empty() { command } else { desc })
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn show(&self, id: &str, tail: usize) -> Result<String> {
        let out = self.dir.join(format!("{id}.out"));
        if !self.dir.join(format!("{id}.cmd")).exists() {
            bail!("no job {id}");
        }
        let text = std::fs::read_to_string(&out).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        let start = lines.len().saturating_sub(tail);
        let mut s = format!("job {id}: {}\noutput: {} ({} lines", self.status(id), out.display(), lines.len());
        if start > 0 {
            s.push_str(&format!(", last {tail}"));
        }
        s.push_str(")\n");
        s.push_str(&lines[start..].join("\n"));
        if !lines.is_empty() {
            s.push('\n');
        }
        Ok(cap(s))
    }

    pub fn stop(&self, id: &str) -> Result<()> {
        let pid: i32 = std::fs::read_to_string(self.dir.join(format!("{id}.pid")))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .with_context(|| format!("no job {id}"))?;
        // setsid made the shell a session leader, so its pid is the group.
        unsafe { libc::kill(-pid, libc::SIGTERM) };
        Ok(())
    }
}

fn cap(s: String) -> String {
    if s.len() <= MAX_OUTPUT {
        return s;
    }
    let half = MAX_OUTPUT / 2;
    let head_end = (0..=half).rev().find(|&i| s.is_char_boundary(i)).unwrap_or(0);
    let tail_start = (s.len() - half..s.len()).find(|&i| s.is_char_boundary(i)).unwrap_or(s.len());
    format!(
        "{}\n\n[… {} bytes cut from the middle of the output …]\n\n{}",
        &s[..head_end],
        tail_start - head_end,
        &s[tail_start..]
    )
}

fn shell_quote(p: &Path) -> String {
    format!("'{}'", p.display().to_string().replace('\'', "'\\''"))
}

fn nanos() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cwd_persists_and_output_combines() {
        let tmp = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        let r = run("cd / && echo out && echo err >&2", &tmp, 5000).await.unwrap();
        assert_eq!(r.status, 0);
        assert_eq!(r.output, "out\nerr\n");
        assert_eq!(r.cwd.as_deref(), Some(Path::new("/")));
        let r = run("exit 3", &tmp, 5000).await.unwrap();
        assert_eq!(r.status, 3);
    }

    #[tokio::test]
    async fn timeout_kills() {
        let tmp = std::env::temp_dir();
        let r = run("sleep 5; echo late", &tmp, 200).await.unwrap();
        assert!(r.timed_out);
        assert!(!r.output.contains("late"));
    }

    #[test]
    fn cap_keeps_both_ends() {
        let s = "a".repeat(20_000) + &"b".repeat(20_000);
        let c = cap(s);
        assert!(c.starts_with("aaaa") && c.ends_with("bbbb") && c.contains("cut from the middle"));
    }
}
