//! The file tools: `read`, `write`, `edit`, with the same rules as Claude Code's own.

use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};

/// Lines `read` returns when no limit is given, as the built-in does.
const DEFAULT_LIMIT: usize = 2000;
/// Longer lines are cut, as the built-in cuts them.
const MAX_LINE: usize = 2000;

/// An absolute, normalised path for `path`, relative paths taken against `cwd`,
/// refused when it lies outside every root. Symlinks in the existing part of the
/// path are followed so a link out of a root does not get through.
pub fn resolve(path: &str, cwd: &Path, roots: &[PathBuf]) -> Result<PathBuf> {
    let raw = Path::new(path);
    let joined = if raw.is_absolute() { raw.to_path_buf() } else { cwd.join(raw) };
    let normal = normalise(&joined);
    // Canonicalise the deepest ancestor that exists, then re-append the rest,
    // so a file about to be created is checked where it will really land.
    let mut existing = normal.as_path();
    let mut rest = Vec::new();
    while !existing.exists() {
        let Some(parent) = existing.parent() else { break };
        if let Some(name) = existing.file_name() {
            rest.push(name.to_owned());
        }
        existing = parent;
    }
    let mut real = std::fs::canonicalize(existing).unwrap_or_else(|_| existing.to_path_buf());
    for name in rest.iter().rev() {
        real.push(name);
    }
    if !roots.is_empty() && !roots.iter().any(|r| real.starts_with(r)) {
        bail!(
            "{} is outside the directories this server serves ({})",
            real.display(),
            roots.iter().map(|r| r.display().to_string()).collect::<Vec<_>>().join(", ")
        );
    }
    Ok(real)
}

/// `.` and `..` folded without touching the filesystem.
fn normalise(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The file's lines from `offset` (1-based), `limit` of them, numbered like `cat -n`.
pub fn read(path: &Path, offset: Option<usize>, limit: Option<usize>) -> Result<String> {
    if path.is_dir() {
        bail!("{} is a directory, not a file", path.display());
    }
    let bytes = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    if bytes.is_empty() {
        bail!("{} is empty", path.display());
    }
    let text = String::from_utf8_lossy(&bytes);
    let start = offset.unwrap_or(1).max(1);
    let limit = limit.unwrap_or(DEFAULT_LIMIT);
    let total = text.lines().count();
    if start > total {
        bail!("{} has {} lines; offset {} is past its end", path.display(), total, start);
    }
    let mut out = String::new();
    for (i, line) in text.lines().enumerate().skip(start - 1).take(limit) {
        let line = if line.chars().count() > MAX_LINE {
            let cut: String = line.chars().take(MAX_LINE).collect();
            format!("{cut}… [line cut at {MAX_LINE} characters]")
        } else {
            line.to_string()
        };
        out.push_str(&format!("{:>6}\t{}\n", i + 1, line));
    }
    let shown_to = (start - 1 + limit).min(total);
    if shown_to < total {
        out.push_str(&format!("[{} more lines; read from offset {}]\n", total - shown_to, shown_to + 1));
    }
    Ok(out)
}

/// The whole file replaced, parents created, written beside and renamed into place.
pub fn write(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("cannot create {}", parent.display()))?;
    }
    let tmp =
        path.with_extension(format!("{}.remote-claude.tmp", path.extension().and_then(|e| e.to_str()).unwrap_or("")));
    std::fs::write(&tmp, content).with_context(|| format!("cannot write {}", tmp.display()))?;
    if let Ok(meta) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(&tmp, meta.permissions());
    }
    std::fs::rename(&tmp, path).with_context(|| format!("cannot replace {}", path.display()))?;
    Ok(())
}

/// `old` replaced by `new` in the file: exactly once unless `replace_all`, an error
/// when it is absent, ambiguous, or equal to `new`. Returns how many were replaced.
pub fn edit(path: &Path, old: &str, new: &str, replace_all: bool) -> Result<usize> {
    if old == new {
        bail!("old_string and new_string are the same");
    }
    if old.is_empty() {
        bail!("old_string is empty");
    }
    let text = std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let count = text.matches(old).count();
    if count == 0 {
        bail!("old_string was not found in {}", path.display());
    }
    if count > 1 && !replace_all {
        bail!(
            "old_string matches {} places in {}; include more context to make it unique, or set replace_all",
            count,
            path.display()
        );
    }
    let edited = if replace_all { text.replace(old, new) } else { text.replacen(old, new, 1) };
    write(path, &edited)?;
    Ok(if replace_all { count } else { 1 })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("remote-claude-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn edit_rules() {
        let p = tmp("edit.txt");
        std::fs::write(&p, "a b a\n").unwrap();
        assert!(edit(&p, "a", "a", false).is_err(), "same strings");
        assert!(edit(&p, "z", "y", false).is_err(), "absent");
        assert!(edit(&p, "a", "c", false).is_err(), "ambiguous");
        assert_eq!(edit(&p, "a", "c", true).unwrap(), 2);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "c b c\n");
        assert_eq!(edit(&p, "b", "d", false).unwrap(), 1);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "c d c\n");
    }

    #[test]
    fn read_window() {
        let p = tmp("read.txt");
        std::fs::write(&p, "one\ntwo\nthree\n").unwrap();
        let all = read(&p, None, None).unwrap();
        assert_eq!(all, "     1\tone\n     2\ttwo\n     3\tthree\n");
        let mid = read(&p, Some(2), Some(1)).unwrap();
        assert_eq!(mid, "     2\ttwo\n[1 more lines; read from offset 3]\n");
        assert!(read(&p, Some(9), None).is_err());
        let empty = tmp("empty.txt");
        std::fs::write(&empty, "").unwrap();
        assert!(read(&empty, None, None).is_err());
    }

    #[test]
    fn roots_hold() {
        let root = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        let roots = std::slice::from_ref(&root);
        let ok = resolve("x/y.txt", &root, roots).unwrap();
        assert!(ok.starts_with(&root));
        assert!(resolve("/etc/passwd", &root, roots).is_err());
        assert!(resolve("../../../../etc/passwd", &root, roots).is_err());
        assert!(resolve("/etc/passwd", &root, &[]).is_ok(), "no roots means anywhere");
    }
}
