# sessions: /sessions lists this folder's sessions; /resume [N|ID], /new and /reload start henki on one

Commands for you, not for the model (they cost no prompt bytes):

- `/sessions`: the 20 newest sessions of this working directory, newest first: number, id, start time (UTC), size
  and the first message (60 characters). `*` marks this session.
- `/resume N` or `/resume ID`: switch to that session (henki asks first); `/resume` alone: the newest other one.
- `/new`: close this session and start a new one.
- `/reload`: restart henki on this session (a henki you upgraded, say). It cannot load a config, AGENTS.md or module
  that changed before it ran: henki refuses a one-key restart then, and you restart henki yourself
  (`henki --resume` continues this session). `/mods` asks to restart on its own changes.

henki closes this session as `/exit` does, then starts again with the same arguments. If a trust file changed while
the command ran, henki names it and asks before it restarts; if one changed before, restart henki yourself.

These read henki's own session files (`$XDG_STATE_HOME/henki/sessions/`). When the history port is bound to another
store, there are none for this session: use that store's commands.
