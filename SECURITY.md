# Security

henki gives a model's commands to your shell and its edits to your files. This file says what henki does to keep
that in your hands, what it does not do, and how to report a problem. Each claim names the test that holds henki to
it: a Rust test (`cargo test NAME`) or a case of the module manager's tests (`manager/tests.sh`,
`manager/mods-tests.sh`, `manager/dev-tests.sh`).

## Reporting a vulnerability

Use GitHub's private vulnerability reporting on this repository (Security, then "Report a vulnerability"). Please do
not open a public issue for it. You get an answer within 7 days. Say which version (`henki --version`), what you did,
and what you saw; a session journal (`~/.local/state/henki/sessions/`) helps, after you have read it for secrets.

Supported versions: the latest release. Fixes are not backported.

## The safety model

- **You approve each call.** With `approve = "ask"` (the default) every `shell`, `write` and `edit` call, and a `read`
  outside the project, is shown whole and asked about first. An allow rule lets one exact command run without asking
  in one project; nothing else is skipped. `approve = "auto"` and `henki run --approve auto` ask nothing: use them
  only where everything the model can reach is expendable.
- **henki's own files are protected.** The model's `write` and `edit` are refused on the config file and dir (the
  allow file and `AGENTS.md` with them), the state and data dirs, every module path and port link, and their real
  paths. Under a policy binding, a call the binding allows is asked instead when it reads one of them or its command
  names one. A shell command is not confined to that: what stops a shell call from changing them is your approval
  and the tripwire.
- **The tripwire.** henki stamps every trust file at start (the config, the allow file, `AGENTS.md`, the bash it
  runs, every module file it loads, the port links and their programs) and checks them before and after every call.
  When one changed, the turn ends `error changed` and henki takes no more messages until you restart it, so a new
  program never runs unseen in this session.
- **Processes are henki's to stop.** Each call, observer, context source and binding runs in a process group and
  session of its own. A timeout, Ctrl-C or the end of the session kills them; on Linux every descendant is swept
  at the end, on macOS what stays in the groups (a process that starts a session of its own outlives henki there).
- **A call's environment is a short list.** Calls get `HOME`, `USER`, `LOGNAME`, `SHELL`, `TERM`, the locale, `TZ`,
  `TMPDIR`, `NO_COLOR`, `SSH_AUTH_SOCK`, `SSL_CERT_FILE`, `SSL_CERT_DIR`, the five `XDG_*` dirs, the proxy variables,
  what `pass_env` adds, and henki's own `HENKI_*` variables, never the API key. `SSH_AUTH_SOCK` is there so `git push`
  works: anything the model runs can sign with the keys your agent holds. Bind the `sandbox` exec binding (below), or
  keep keys you care about out of the agent.
- **The model server.** A `base_url` that is not a loopback address gets a notice at every start: prompts, files and
  command output go to that host, unencrypted (henki speaks plain HTTP).

## Ports and bindings

A module can replace one of henki's ports with a program of its own: `history` (where sessions are kept), `policy`
(who may run a call), `context` (what the model sees of the history) and `exec` (where shell calls run). henki keeps
a floor under every binding:

- A binding gets the tool environment without the modules' `bin/`, runs in its own process group, and is killed and
  swept at the session end. `henki --port PORT=std` runs a session without it; `henki mods swap PORT std` drops it.
- A `policy` binding never allows a write to a protected path, and fails closed: a binding that hangs, dies or
  answers garbage turns every later call into a question.
- A `history` binding that fails ends the turn; henki never falls back to its own files in the middle of a session.
- A `context` binding sees the whole history before each request and chooses what the model sees. henki takes its
  answer only when every call and result in it is the history's own, unchanged and in order, and it fits the window;
  otherwise henki's own elision shapes that request. The journal keeps everything either way.
- A `history`, `context` or `policy` binding sees the whole session: it is code you trust as much as henki.

## Sandboxing the model's calls

The official `sandbox` module is an exec binding (`henki mods swap exec sandbox`): every shell call runs inside it
with `SSH_AUTH_SOCK` dropped. What it hides depends on the platform:

- Linux (bubblewrap, or user namespaces): your whole home and henki's own dirs are hidden; the project and a private
  `/tmp` are writable (and a `TMPDIR` outside `/tmp`, shared with your machine); everything else is read-only.
- macOS (`sandbox-exec`): a list of secret paths (`~/.ssh`, `~/.config`, `~/.aws`, the Keychains and others) and
  henki's own dirs are hidden; the rest of your home stays readable (other repos, documents, shell history). The
  project, the temp dirs and `/private/tmp` are writable; everything else is read-only.

The module's `doc.md` names each hidden path. While an exec binding is bound the model gets the `shell` tool only, so
no tool of the model runs on the host outside it. What it does not cover:

- Your `!cmd`, module commands, observers, context sources and the other port bindings run on the host, unsandboxed.
- The network stays on, and so do sockets it cannot hide: loopback servers, abstract unix sockets (Linux). Its
  `doc.md` lists what it hides; the list is not a proof.
- Files the model writes inside the project (`AGENTS.md`, git hooks, scripts) are read or run later by code on the
  host with your rights: a context source, git, you.
- A PATH dir inside the project (an activated `.venv/bin`, direnv's `PATH_add bin`, a `node_modules/.bin`) lets the
  model put a program of its own in front of the tools that code on the host runs by name. The sandbox takes its
  backend (`bwrap`, `unshare`) from the system dirs or your PATH but never one whose real path is in the project or
  the temp dir, and runs its own lookups on the system dirs; the official `project`, `checkpoints`, `notify` and `compact`,
  which run unasked, take their programs from the system dirs only. The commands you type (`henki mods`, `/sessions`,
  the `prompts` templates, your own modules), your shell and the programs you run from the project still find them
  first: keep such a dir off the PATH you start henki with.
- An exec program that hands the command to a daemon (a container runtime, ssh) moves it out of henki's kill and
  sweep; such a binding must say so in its doc.

## Modules, the index and the manager

- `henki mods add` copies a module into a read-only store, pinned by commit and tree hash, and runs nothing from it:
  no checkout, no hooks, no build. Its screen lists every program the module brings and when each runs, and asks.
- A module's code runs with your rights once it is on. `caps_net` says where its programs connect; it is declared,
  not enforced.
- A module from a URL is marked UNREVIEWED in red and gets one question; one from a directory on this machine is
  shown by its path (the screen says it is a directory here) and gets the same question. A module from the signed index
  shows who reviewed which version. The index's `main` is a chain of SSH signatures checked from the key henki ships:
  a key changes only through a commit signed by a current key, the verified commit must stay on the first-parent line
  of every later head (no rollback, no branch grafted by a key since rotated out), a first sync must find the
  checkpoint henki ships, and a key in `refused.keys` never verifies. Every index screen names the key's owner and
  fingerprint, and the anchor's path when it is not the one beside the manager.
- A yank reaches you only when you run `henki mods` (`sync`, or any command that reads the index). A module yanked as
  malicious is turned off; a sandbox or history binding of it stops henki until you choose another (`henki mods swap
  PORT std`), rather than falling back unseen.
- Inside a tool call the manager refuses every change to what henki runs (`add`, `on`, `swap`, `update`, `rollback`,
  `unpack`, `init`, `index add`). The model can get past that by changing its environment: the guard is henki asking
  before each shell call. Under `approve = "auto"`, or an allow rule that covers `henki`, the model can install code.
- `henki mods eval` runs the model with `approve = "auto"`, so every run of it is inside the `sandbox` program with a
  scratch home and scratch XDG dirs; without a working sandbox it runs only with `--unsandboxed`, and then only those
  dirs are scratch.

## Frontends and context

- `henki serve` hands the session to a frontend program, which is the approver: it answers the questions and can save
  `always` rules for the project root the question names. It must show you what the REPL shows before it does. Anything
  that exposes `henki serve` beyond your own processes (a socket, a network) must authenticate the other side itself.
- Context sources (`ctx/` in a module) run at every start, before any question, and their text goes into every
  request; each start shows their bytes. The `project` module reads the project's `AGENTS.md`: a cloned repository
  can carry instructions there. It reads only regular files, never a link, so a planted link cannot point it at a
  secret.
- The `compact` context binding's summary is written from the session, tool output included, and reaches the model
  as a `user` record labelled as the assistant's notes. Instructions planted in a page or a file can reach the model
  through it.

## Accepted risks

- A remote `base_url` only warns.
- An unreviewed install gets a red label and one question, not a refusal.
- A yank reaches only people who run `henki mods`.
- Module code runs with your rights; `caps_net` is not enforced.
- Under `approve = "auto"` or an allow rule that covers `henki`, the model can run `henki mods`.
- A frontend that approves everything answers for you, as `--approve auto` would; protected paths and the tripwire
  still hold.

## Claims and their tests

| Claim | Test |
|---|---|
| each call is asked, and an allow rule is one exact command in one project | `e2e_allow_rule_runs`, `e2e_allow_exact_only`, `e2e_allow_other_root_asks` |
| the model cannot write henki's files | `write_protected_config`, `write_protected_state`, `write_protected_realpath`, `e2e_allow_protected_write`, `e2e_policy_floor` |
| a changed trust file ends the turn | `e2e_tripwire_run_exit2`, `e2e_tripwire_bash_replaced`, `e2e_tripwire_allow_in_auto` |
| calls are killed and swept | `e2e_timeout_kills_call_tree`, `e2e_cancel_setsid_grandchild`, `e2e_run_exit_kills_background` |
| a call's environment is the allowlist | `e2e_env_allowlist` |
| a remote server is named at start | `config_remote_base_url_notice`, `pty_remote_base_url_notice` |
| a policy binding fails closed; history never falls back | `e2e_policy_failures_ask`, `e2e_policy_auto_refused`, `e2e_port_dead_mid_turn`, `e2e_port_refused_starts` |
| a bound exec runs every shell call and leaves only `shell`; a dangling one stops the start | `e2e_exec_runs_every_call`, `e2e_exec_shell_only`, `e2e_exec_needs_shell`, `e2e_exec_dangling_refused` |
| an exec program is killed and swept as the call; a module's `bash` never runs the call | `e2e_exec_timeout_kills_program`, `e2e_exec_background_swept`, `e2e_exec_module_bash_not_used` |
| `sandbox` hides the home (Linux) or a list of secret paths (macOS) and the agent socket, and keeps writes to the project and the temp dirs | `e2e_exec_official_sandbox`, `t_sandbox` |
| `sandbox` never runs a backend or tool from the project or the temp dir; `project` and `notify` never run one from the tool PATH | `e2e_exec_sandbox_planted_backend`, `t_sandbox`, `t_project`, `t_notify` |
| context sources do not run through exec, are skipped not cut, killed when slow, swept, trusted | `ctx_not_through_exec`, `ctx_skip_never_cut`, `ctx_slow_source_killed`, `ctx_leftover_swept`, `ctx_entry_is_trusted` |
| `project` never follows a link | `t_project` |
| a context binding's answer is checked; a failing one leaves henki's elision | `e2e_context_refused_answers`, `e2e_context_dead_or_failing`, `e2e_context_start_failure` |
| `compact`'s summary is labelled as the assistant's notes | `t_compact` |
| a serve frontend's answers name the call; `always` only where offered | `serve_questions_yes_always`, `serve_questions_wrong_id_and_always_not_offered`, `serve_lines`, `serve_tripwire` |
| the index chain: signatures, rotation, first-parent line, checkpoint, refused keys | `t_index_sync` |
| yanks, at sync and at every install | `t_index_yank` |
| tiers and the red UNREVIEWED label | `t_index_add` |
| the manager refuses changes inside a tool call | `t_index_guard` |
| an add runs nothing from a module and copies only committed bytes | `t_raw_blobs`, `t_transport`, `t_gitattributes` |
| a pack cannot roll an index back past a yank | `t_pack` |
| eval runs sandboxed in scratch dirs; what a run plants does not reach the check | `t_eval_live`, `t_eval_plant` |
