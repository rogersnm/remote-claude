# remote-claude

Run Claude Code on your laptop while its files and shell live on another machine. Or run it on that machine, in tmux, and still paste screenshots from your laptop with ctrl+v.

```
Your laptop                                      The remote machine
─────────────────────────────                    ─────────────────────────
claude  (the CLI, model calls,                   remote-claude serve
         plan mode, skills, memory)              (a small Rust program)
   │                                                ▲
   │  Claude decides: "run `git status`"            │
   │  → calls tool mcp__myhost__bash                │
   │                                                │
   └── ssh myhost remote-claude serve ──────────────┘
        (stdin/stdout carry JSON: "run this command" → "here's the output")
```

The remote machine does no thinking: it reads, edits and runs commands, and sends back the results. Claude's model calls, your login, skills and memory stay on the laptop. Only tool calls cross the network; the session itself lives on the laptop.

Claude Code keeps running locally, with its plan mode, subagents, skills, memory, and login. Its `Read`, `Edit`, `Write` and `Bash` tools are switched off, and a small MCP server on the remote machine provides `read`, `edit`, `write` and `bash` in their place, with the same parameters and the same output. Claude Code starts that server over ssh, so the ssh session's stdin and stdout carry the MCP stream. The server is one static binary and knows nothing about ssh.

This is useful when the code, the build, or the hardware lives somewhere else: a big cloud box, a Linux machine for a Linux-only toolchain, or a home server you reach from a laptop on a bad connection. Nothing is synced or mounted. Each tool call is one round trip.

## Install

**On the remote machine** (Linux; Rust 1.85 or later):

```sh
cargo install --locked --git https://github.com/rogersnm/remote-claude
```

This puts `remote-claude` in `~/.cargo/bin`, which is where the launcher looks by default. Background jobs need `setsid`, which every Linux has (util-linux).

**On your machine** (macOS or Linux, with Claude Code installed, and `jq` for the Monitor tool), put the launcher on your `PATH`:

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

## Or run Claude Code on the remote, with image paste and `open`

```sh
rclaude myhost --on-host --cwd src/myproject
```

This is the other way round: Claude Code runs on the remote, in a tmux session named after the working tree (`--session` to choose), so it keeps working while your laptop sleeps, and running the command again reattaches. What you would lose by ssh-ing in and running `claude` yourself is pasting a screenshot with ctrl+v, because Claude Code reads the clipboard of the machine it runs on. `--on-host` keeps it:

```
Your Mac                                          The remote machine
───────────────────────────────                   ──────────────────────────────────
rclaude --on-host                                 tmux ─ claude
  a clipboard responder (perl)                      ctrl+v → xclip -t image/png -o
          ▲                                                    │  (remote-claude, as xclip)
          └──── ssh -R 127.0.0.1:<port> ◄──────────────────────┘
```

On Linux, Claude Code pastes an image by running `xclip -selection clipboard -t TARGETS -o` and `xclip -selection clipboard -t image/png -o`. `--on-host` links `xclip` to `remote-claude` in a directory it puts first on the session's `PATH`; run under that name, it answers those two calls through a port forwarded back to a small responder on your Mac, which reads the clipboard with `osascript` as Claude Code does locally. Any other `xclip` call, and every call while no `rclaude` connection is open, goes to the real `xclip` if there is one. A token written to `~/.rclaude-link` (mode 600) on every connect keeps the remote's other users off the link, and lets a Claude Code started in an earlier connection find the new one after you reattach.

**mosh.** When both ends have [mosh](https://mosh.org) (`brew install mosh`, `apt install mosh`), `--on-host` connects with it instead of ssh: mosh echoes your keystrokes locally instead of waiting a round trip for each one, and a session survives the laptop sleeping or changing networks without reattaching. mosh carries only the terminal, so the link rides a separate ssh that reconnects by itself after a sleep. `mosh-server` is started with `LC_ALL=C.UTF-8`, the one UTF-8 locale every Linux has. `--ssh` connects with ssh anyway.

`--command <cmd>` runs a shell command in a new tmux session instead of `claude`; a Claude Code that command starts still gets the link.

On the remote this needs tmux, Claude Code and `remote-claude`. The link needs macOS; from Linux, `--on-host` still gives you the tmux session, without it. A tmux session started some other way does not have the links on its `PATH`, so start it with `rclaude`.

### Opening files on your Mac

In both modes, `open <file>` and `xdg-open <file>` on the remote are `remote-claude` too: they copy each file over the same link, and your Mac saves it to `~/Downloads/rclaude/<host>/` and opens it, so asking Claude to show you a PDF or a screenshot it made just works. The Mac decides what it will open, not the remote: documents, images, audio and video by extension (pdf, png, jpg, gif, webp, heic, svg, html, md, txt, csv, tsv, json, log, mp4, mov, webm, mp3, wav), up to 100 MB, never programs or scripts, since opening a `.command` or an `.app` would run it. Each copy carries macOS's quarantine flag, like any download. URLs are refused; only files cross.

## How it works

`rclaude` generates four things and passes them to `claude` on the command line. It writes no config files.

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
    "deny": ["Read", "Edit", "Write", "NotebookEdit", "Grep", "Glob", "Bash", "DesignSync", "EnterWorktree", "ExitWorktree"],
    "allow": ["mcp__myhost__read", "mcp__myhost__edit", "mcp__myhost__write", "mcp__myhost__bash", "mcp__myhost__bash_jobs"]
  }
}
```

The deny list names every tool that reads local files or runs local commands, except Monitor (below), and all of them matter: with `Grep` still allowed, Claude once read a local README with it and reported it as the remote one. `DesignSync` can upload local files. Worktree isolation is local and is denied too; make worktrees by hand with `git worktree` through `bash`.

**A hook that moves Monitor to the remote.** Claude Code's `Monitor` tool streams a command's output lines to Claude as events, and runs that command in a local shell. A `PreToolUse` hook, which is `rclaude` itself in `--monitor-hook` mode, rewrites each command to `ssh <host> 'cd <cwd> || exit; <command>'` on the same held connection, so Claude writes ordinary monitors and they run on the remote. A hook that fails in any other way lets Claude Code run the tool with its original input, locally, so every failure is turned into exit 2, which refuses the call: a monitor with no command (the WebSocket form), a missing `jq`, even a launcher that has been moved away. Without `jq`, `rclaude` denies `Monitor` instead.

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

Your own machine is kept out of reach by the deny list and the Monitor hook, which name tools one by one. A future Claude Code release can add a tool that reads local files or runs local commands. After upgrading, ask a session to list its tools and try each unfamiliar one against a local file or `hostname`.

## Limitations

- The remote must be Linux, because background jobs use `setsid` and `--on-host` stands in for `xclip`.
- Clickable `file:line` references and the IDE diff view for edits assume local paths, so they do not work.
- Hooks, skills and anything else that runs a local command still run locally.

## License

MIT
