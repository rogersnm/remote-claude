//! remote-claude: Claude Code's `Read`, `Edit`, `Write` and `Bash` tools, served
//! over MCP on stdio so that `ssh <host> remote-claude serve` gives a Claude Code
//! session on one machine those tools on another. The server knows nothing about
//! ssh; the ssh session's stdin and stdout are the MCP stream.

use std::path::PathBuf;
use std::sync::Arc;

use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars, tool, tool_handler, tool_router,
    transport::stdio,
};
use tokio::sync::Mutex;

mod clip;
mod files;
mod link;
mod open;
mod shell;

/// Parameters of `read`, the same as Claude Code's `Read`.
#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ReadArgs {
    /// Absolute path of the file to read, or a path relative to the shell's working directory.
    pub file_path: String,
    /// Line number to start from (1-based). Only when the file is too large to read at once.
    pub offset: Option<usize>,
    /// Number of lines to read. Only when the file is too large to read at once.
    pub limit: Option<usize>,
}

/// Parameters of `write`, the same as Claude Code's `Write`.
#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct WriteArgs {
    /// Absolute path of the file to write (creates or overwrites it, and its parent directories).
    pub file_path: String,
    /// The whole content of the file.
    pub content: String,
}

/// Parameters of `edit`, the same as Claude Code's `Edit`.
#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct EditArgs {
    /// Absolute path of the file to edit.
    pub file_path: String,
    /// The exact text to replace. Must match the file exactly, including indentation, and be unique unless replace_all is set.
    pub old_string: String,
    /// The text to put in its place. Must differ from old_string.
    pub new_string: String,
    /// Replace every occurrence instead of requiring old_string to be unique.
    #[serde(default)]
    pub replace_all: bool,
}

/// Parameters of `bash`, the same as Claude Code's `Bash`.
#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct BashArgs {
    /// The command to run, with bash.
    pub command: String,
    /// Timeout in milliseconds (default 120000, max 600000). Ignored for a background run.
    pub timeout: Option<u64>,
    /// What the command does, in a few words. Shown to the user; not used by the server.
    pub description: Option<String>,
    /// Run detached and return at once with a job id; the output goes to a file. Poll with bash_jobs.
    #[serde(default)]
    pub run_in_background: bool,
}

/// Parameters of `bash_jobs`.
#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct BashJobsArgs {
    /// A job id from a background bash run. Without it, every job is listed.
    pub id: Option<String>,
    /// Stop the job (with its whole process group).
    #[serde(default)]
    pub stop: bool,
    /// How many lines from the end of the job's output to return (default 100).
    pub tail: Option<usize>,
}

#[derive(Clone)]
pub struct Remote {
    /// Directories the file tools may touch. Empty means anywhere.
    roots: Arc<Vec<PathBuf>>,
    /// The shell's working directory, kept between `bash` calls like Claude Code's own.
    cwd: Arc<Mutex<PathBuf>>,
    /// Where the shell started, named in the instructions as the session's working directory.
    start_cwd: PathBuf,
    jobs: Arc<shell::Jobs>,
    host: String,
    tool_router: ToolRouter<Remote>,
}

impl Remote {
    /// The router the handler macro dispatches through.
    fn router(&self) -> &ToolRouter<Remote> {
        &self.tool_router
    }
}

#[tool_router]
impl Remote {
    fn new(roots: Vec<PathBuf>, cwd: PathBuf, jobs: shell::Jobs) -> Self {
        let host = std::process::Command::new("hostname")
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|h| !h.is_empty())
            .unwrap_or_else(|| "remote".to_string());
        Self {
            roots: Arc::new(roots),
            start_cwd: cwd.clone(),
            cwd: Arc::new(Mutex::new(cwd)),
            jobs: Arc::new(jobs),
            host,
            tool_router: Self::tool_router(),
        }
    }

    /// A path from the model, made absolute against the shell's cwd and checked against the roots.
    async fn resolve(&self, path: &str) -> Result<PathBuf, McpError> {
        let cwd = self.cwd.lock().await.clone();
        files::resolve(path, &cwd, &self.roots).map_err(invalid)
    }

    #[tool(
        description = "Read a file on the remote machine (the remote form of Read). Returns numbered lines, up to 2000 by default; use offset and limit for more of a large file.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn read(&self, Parameters(a): Parameters<ReadArgs>) -> Result<CallToolResult, McpError> {
        let path = self.resolve(&a.file_path).await?;
        let text = files::read(&path, a.offset, a.limit).map_err(invalid)?;
        Ok(text_result(text))
    }

    #[tool(
        description = "Write a whole file on the remote machine (the remote form of Write). Creates the file and its parent directories, or overwrites it."
    )]
    async fn write(&self, Parameters(a): Parameters<WriteArgs>) -> Result<CallToolResult, McpError> {
        let path = self.resolve(&a.file_path).await?;
        files::write(&path, &a.content).map_err(invalid)?;
        Ok(text_result(format!("Wrote {} bytes to {}", a.content.len(), path.display())))
    }

    #[tool(
        description = "Replace text in a file on the remote machine (the remote form of Edit). old_string must match exactly and be unique unless replace_all is set."
    )]
    async fn edit(&self, Parameters(a): Parameters<EditArgs>) -> Result<CallToolResult, McpError> {
        let path = self.resolve(&a.file_path).await?;
        let n = files::edit(&path, &a.old_string, &a.new_string, a.replace_all).map_err(invalid)?;
        Ok(text_result(format!(
            "Edited {}: replaced {} occurrence{}",
            path.display(),
            n,
            if n == 1 { "" } else { "s" }
        )))
    }

    #[tool(
        description = "Run a shell command on the remote machine (the remote form of Bash). The working directory persists between calls; other shell state does not. Output is stdout and stderr together, capped. run_in_background returns a job id at once."
    )]
    async fn bash(&self, Parameters(a): Parameters<BashArgs>) -> Result<CallToolResult, McpError> {
        if a.run_in_background {
            let cwd = self.cwd.lock().await.clone();
            let job = self.jobs.start(&a.command, &cwd, a.description.as_deref()).map_err(internal)?;
            return Ok(text_result(format!(
                "Job {} started in the background. Output is written to {}. Poll it with bash_jobs.",
                job.id,
                job.out.display()
            )));
        }
        let timeout = a.timeout.unwrap_or(120_000).min(600_000);
        let mut cwd = self.cwd.lock().await;
        let run = shell::run(&a.command, &cwd, timeout).await.map_err(internal)?;
        if let Some(dir) = run.cwd {
            *cwd = dir;
        }
        drop(cwd);
        let mut text = run.output;
        if run.timed_out {
            text.push_str(&format!("\n[timed out after {} ms]", timeout));
        } else if run.status != 0 {
            text.push_str(&format!("\n[exit code {}]", run.status));
        }
        if run.status != 0 || run.timed_out {
            return Ok(CallToolResult::error(vec![ContentBlock::text(text)]));
        }
        Ok(text_result(text))
    }

    #[tool(
        description = "Background jobs from bash: list them, read one's status and the tail of its output, or stop one."
    )]
    async fn bash_jobs(&self, Parameters(a): Parameters<BashJobsArgs>) -> Result<CallToolResult, McpError> {
        let text = match a.id {
            None => self.jobs.list(),
            Some(id) => {
                if a.stop {
                    self.jobs.stop(&id).map_err(invalid)?;
                }
                self.jobs.show(&id, a.tail.unwrap_or(100)).map_err(invalid)?
            }
        };
        Ok(text_result(text))
    }
}

#[tool_handler(router = self.router())]
impl ServerHandler for Remote {
    fn get_info(&self) -> ServerConfig {
        let roots = if self.roots.is_empty() {
            "anywhere".to_string()
        } else {
            self.roots.iter().map(|r| r.display().to_string()).collect::<Vec<_>>().join(", ")
        };
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(server_info())
            .with_instructions(format!(
                "You are working on the machine `{host}`, not on the one Claude Code runs on. \
                 The platform, working directory and shell in your environment describe the machine \
                 Claude Code runs on; ignore them when asked where you are or what you are working on. \
                 Your working directory is {cwd} on `{host}`. \
                 read, edit, write and bash are the remote forms of Read, Edit, Write and Bash: \
                 same parameters, same results. Use them for all file and shell work; the local \
                 file and shell tools are turned off. Every path is a path on `{host}`; the file \
                 tools work under {roots}.",
                host = self.host,
                cwd = self.start_cwd.display(),
                roots = roots,
            ))
    }
}

fn server_info() -> Implementation {
    let mut info = Implementation::from_build_env();
    info.name = "remote-claude".into();
    info.version = env!("CARGO_PKG_VERSION").into();
    info
}

fn text_result(text: impl Into<String>) -> CallToolResult {
    CallToolResult::success(vec![ContentBlock::text(text.into())])
}

fn invalid(e: anyhow::Error) -> McpError {
    McpError::invalid_params(e.to_string(), None)
}

fn internal(e: anyhow::Error) -> McpError {
    McpError::internal_error(e.to_string(), None)
}

fn usage() -> ! {
    eprintln!("usage: remote-claude serve [--root <dir>]... [--cwd <dir>]");
    eprintln!("  --root  a directory the file tools may touch (repeatable; default: the current directory)");
    eprintln!("  --cwd   the shell's starting directory (default: the current directory)");
    std::process::exit(2)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Linked in as `xclip`, `open` and `xdg-open` by `rclaude`: see link.rs.
    let mut argv = std::env::args_os();
    let invoked_as = argv.next().map(PathBuf::from);
    match invoked_as.as_deref().and_then(|p| p.file_name()).and_then(|n| n.to_str()) {
        Some("xclip") => std::process::exit(clip::main(argv.collect())),
        Some(name @ ("open" | "xdg-open")) => std::process::exit(open::main(name, argv.collect())),
        _ => {}
    }
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some("serve") {
        usage();
    }
    let start = std::env::current_dir()?;
    let mut roots = Vec::new();
    let mut cwd = start.clone();
    while let Some(flag) = args.next() {
        let value = args.next().unwrap_or_else(|| usage());
        match flag.as_str() {
            "--root" => roots.push(std::fs::canonicalize(&value)?),
            "--cwd" => cwd = std::fs::canonicalize(&value)?,
            _ => usage(),
        }
    }
    if roots.is_empty() {
        roots.push(std::fs::canonicalize(&start)?);
    }
    let jobs = shell::Jobs::open()?;
    let service = Remote::new(roots, cwd, jobs).serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
