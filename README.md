# remote-claude

Run Claude Code on your laptop while its files and shell live on another machine.

Claude Code keeps running locally, with its plan mode, subagents, skills, memory, and login. Its `Read`, `Edit`, `Write` and `Bash` tools are switched off, and a small MCP server on the remote machine provides `read`, `edit`, `write` and `bash` in their place, with the same parameters and the same output. Claude Code starts that server over ssh, so the ssh session's stdin and stdout carry the MCP stream. The server is one static binary and knows nothing about ssh.

This is useful when the code, the build, or the hardware lives somewhere else: a big cloud box, a Linux machine for a Linux-only toolchain, or a home server you reach from a laptop on a bad connection. Nothing is synced or mounted. Each tool call is one round trip.

## Install

**On the remote machine** (Linux; Rust 1.85 or later):

```sh
cargo install --locked --git https://github.com/rogersnm/remote-claude
```

This puts `remote-claude` in `~/.cargo/bin`, which is where the launcher looks by default. Background jobs need `setsid`, which every Linux has (util-linux).

**On your machine** (macOS or Linux, with Claude Code installed), put the launcher on your `PATH`:

```sh
curl -fsSLo ~/.local/bin/rclaude https://raw.githubusercontent.com/rogersnm/remote-claude/main/bin/rclaude
chmod +x ~/.local/bin/rclaude
```

You need key-based ssh to the remote that works without a prompt. Check that this prints `ok` and nothing else:

```sh
ssh -T -o BatchMode=yes myhost echo ok
```

## Use

```sh
rclaude myhost --cwd src/myproject
```

`myhost` is anything `ssh` accepts: an alias from `~/.ssh/config`, `user@host`, or an address. `--cwd` is the working tree on the remote. A relative path is taken from the remote home directory, so write `src/myproject`, not `~/src/myproject`, which your local shell would expand to a local path.

Options:

| option | meaning |
|---|---|
| `--cwd <dir>` | the remote shell's starting directory, and the tree the session works on |
| `--root <dir>` | a remote directory the file tools may touch; repeatable. Default: `--cwd`, or the remote home directory |
| `--server <path>` | the `remote-claude` binary on the remote. Default: `$HOME/.cargo/bin/remote-claude` |
| `--name <name>` | the MCP server name, which becomes the tool prefix (`mcp__<name>__read`). Default: derived from the host |

Everything else is passed to `claude`, e.g. `rclaude myhost --cwd src/app --resume` or `--dangerously-skip-permissions`.

Each host and working tree gets its own empty local start directory under `~/.local/state/rclaude/`. Claude Code keys session history and project memory by start directory, so `--resume` lists only the sessions for that remote tree, and there is no local checkout for Claude to confuse with the remote one.

## How it works

`rclaude` generates three things and passes them to `claude` on the command line. It writes no config files.

**The MCP server.** A stdio server whose command is ssh:

```json
{
  "mcpServers": {
    "myhost": {
      "type": "stdio",
      "command": "ssh",
      "args": ["-T", "-o", "BatchMode=yes", "-o", "ControlMaster=auto",
               "-o", "ControlPath=~/.ssh/rclaude-%C", "-o", "ControlPersist=10m",
               "myhost", "$HOME/.cargo/bin/remote-claude serve --root 'src/myproject' --cwd 'src/myproject'"]
    }
  }
}
```

`ControlMaster` keeps one connection open per host, so starting the server opens a channel on it rather than doing a new handshake. `BatchMode` makes ssh fail instead of waiting for a password or host-key prompt that nobody can answer. The session must print nothing of its own on stdout: a shell startup file that echoes something will corrupt the stream.

**The permissions.** A bare tool name in `deny` takes that tool out of Claude's context entirely, so Claude sees only the remote set. The `allow` rules stop the remote tools from prompting on every call.

```json
{
  "permissions": {
    "deny": ["Read", "Edit", "Write", "NotebookEdit", "Grep", "Glob", "Bash", "EnterWorktree", "ExitWorktree"],
    "allow": ["mcp__myhost__read", "mcp__myhost__edit", "mcp__myhost__write", "mcp__myhost__bash", "mcp__myhost__bash_jobs"]
  }
}
```

The deny list names every local file and shell tool, and all of them matter: with `Grep` still allowed, Claude once read a local README with it and reported it as the remote one. Worktree isolation is local and is denied too; make worktrees by hand with `git worktree` through `bash`.

**An appended system prompt**, saying the session is working on the remote machine and has no local tools. Claude Code's own environment section (platform, working directory, shell) describes the local machine and ranks above an MCP server's instructions. Without the extra prompt, asked where it is working, Claude names your laptop.

To set it up without the launcher, put the first two blocks in files and run `claude --mcp-config mcp.json --strict-mcp-config --settings settings.json --append-system-prompt "…"` from a directory that is not a checkout.

## The tools

| tool | parameters | notes |
|---|---|---|
| `read` | `file_path`, `offset?`, `limit?` | numbered lines, 2000 by default; marked read-only, so plan mode allows it and refuses the other four |
| `write` | `file_path`, `content` | creates parent directories; writes beside the file and renames into place |
| `edit` | `file_path`, `old_string`, `new_string`, `replace_all?` | exact match; must be unique unless `replace_all`; `old_string` and `new_string` must differ |
| `bash` | `command`, `timeout?`, `description?`, `run_in_background?` | the working directory persists between calls, other shell state does not (as with the built-in); stdout and stderr together, cut in the middle past 30 kB; timeout default 120 s, max 600 s, kills the whole process group; a non-zero exit is reported as a tool error with the output |
| `bash_jobs` | `id?`, `stop?`, `tail?` | background jobs: list them, show one's status and the end of its output, or stop it |

Background jobs run under `setsid`, with their output and exit status in files under `$XDG_RUNTIME_DIR/remote-claude/jobs/`. They outlive the server process and a dropped ssh connection, and a later server still lists them.

Long foreground commands are also bounded by Claude Code's MCP tool timeout. Raise `MCP_TOOL_TIMEOUT` (milliseconds) in your environment if a build legitimately runs longer, or use `run_in_background`.

## Security

`--root` confines the file tools: a path outside every root is refused after symlinks are resolved, so a stray path cannot reach `~/.ssh`. It does not confine `bash`, which runs as your ssh user and can do anything that user can. Treat a session as having your shell on the remote machine, and use a dedicated user or machine if that is too much.

## Limitations

- The remote must be Linux, because background jobs use `setsid`.
- Clickable `file:line` references and the IDE diff view for edits assume local paths, so they do not work.
- Hooks, skills and anything else that runs a local command still run locally.

## License

MIT
