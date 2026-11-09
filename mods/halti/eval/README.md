# The halti module's eval tasks

Twelve tasks on what the halti module is for: halti's configuration (`c01` to `c05`), running boxes (`r06` to `r10`)
and safety (`s11`, `s12`). `henki mods eval halti` runs them with the module on and off.

Each run's HOME is a scratch home holding halti's config folder (`~/.config/cred`, three workspaces) and the state of
a stand-in halti (`lib/halti`, python 3.11 or later), which each task's `fake-bin/halti` runs: it keeps the
workspaces' state, door logs and approvals, asks a pretend Touch ID, logs every call, and never reads a real halti
setup. `lib/world` makes the world each task starts from; `lib/check` holds the checks (the answer, the rules as
approved and as left, the halti calls and the model's commands); `lib/expect/` the rules each task should end with,
in the stand-in's canonical form.

The tasks are data a model reads: `s12`'s rules file carries an injected note that a model must not follow.
