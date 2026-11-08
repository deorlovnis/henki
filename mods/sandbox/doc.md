# sandbox runs the model's shell calls in a sandbox: your home hidden, the project writable

Turn it on with `henki mods swap exec sandbox`; `henki --port exec=std` runs one session without it, and
`henki mods swap exec std` turns it off. While it is bound, henki offers the model the `shell` tool only: `read`,
`write` and `edit` are off, so every tool call of the model runs inside the sandbox. What it hides and what it
leaves reachable are below: a call can still talk to what the network or a visible socket lets it reach.

Inside a call:
- Your home is a writable scratch dir with none of your files, gone after the call; so are henki's config, state and
  data dirs, your runtime dir (`XDG_RUNTIME_DIR`, else `/run/user/UID`: the session bus, systemd, gpg-agent and editor
  sockets), tmux's and screen's socket dirs, and your ssh-agent socket (its dir too, unless that is the project or the
  temp dir), wherever they are (on Linux a hidden dir inside another shows as an empty dir). `SSH_AUTH_SOCK` is unset. The docker and system bus sockets refuse every connection.
- The project (the session's working directory) is writable. On Linux, `/tmp` and `/var/tmp` are empty and private to
  the call; a `TMPDIR` elsewhere is shared with your machine and writable. Everything else is read-only.
- `/dev` holds only `null`, `zero`, `full`, `random`, `urandom`, `tty`, a terminal set of its own (your terminals are not
  there), and a private `/dev/shm`; System V IPC is the call's own.
- The network is on: local model servers listen on loopback. So any server on loopback, any abstract unix socket
  (Linux; an X display or a session bus listening there among them) and any unix socket in a dir that is not hidden
  stays reachable from inside, read-only or not. If one of them runs commands for its clients (an X server that lets
  your uid in, a debug server, a remote-control port), the model can use it.
- Tools installed under your home (`~/.cargo/bin`, `~/.local/bin`, nvm, pyenv) are hidden with it on Linux.
- It refuses to run (exit 126, with a fix) in your home itself, a dir that holds it or a dir it hides, and with a
  `TMPDIR` that is `/`, a hidden dir, or neither yours nor a shared temp dir.

Backends, picked per call:
- Linux with `bwrap` (bubblewrap): a new mount, pid and IPC namespace.
- Linux without it, where user namespaces work: `unshare` and the setup script in `.henki/ns-setup`: a new mount, pid
  and IPC namespace; the command runs as your uid with no capabilities, in a nested namespace where the mounts are
  locked. The project and the temp dir are bound from the host; `/proc` is the host's (other processes' command lines
  are visible, their files and environment are not).
- macOS: `sandbox-exec` with `.henki/macos.sb`. It hides a list (`~/.ssh`, `~/.config`, `~/.aws`, `~/.gnupg`,
  `~/.docker`, `~/.kube`, `~/.netrc`, `~/.git-credentials`, the Keychains and cookies, henki's config, state and data)
  rather than the whole home, and allows writes to the project, the temp dirs and `/private/tmp`. It refuses unix
  sockets outside the project and the temp dirs, and denies `open`, `osascript`, `launchctl`, `shortcuts` and
  `automator`, LaunchServices, Apple Events and new launchd jobs, so a command cannot hand work to an app or a job
  outside the sandbox. Processes are not hidden, and an app that already runs and takes commands over loopback is
  reachable. Apple marks `sandbox-exec` deprecated; it still ships.
- None of these: every call exits 126 with "sandbox: no bwrap, no user namespaces; fix: install bubblewrap, or henki
  mods swap exec std".

On Linux a call is the whole life of its processes and of its `/tmp`: a `sleep 1000 &` or a server started in the
background ends with the call that started it, and files in `/tmp` are gone after it. Start it and use it in one
call; keep files that later calls need in the project.

What it does not do: the hidden paths are a list, not a proof. It confines the model's calls, not what later reads
their results: files the model writes in the project (AGENTS.md, git hooks, scripts) are read or run later by code on
your machine with your rights (context sources, git, you). Your `!cmd`, module commands, observers, context sources
and port bindings run outside it. A `--config` file outside `~/.config/henki` is not hidden.
