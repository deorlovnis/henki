# sessions: /sessions lists this folder's sessions; /resume [N|ID], /fork [N|ID@N], /new and /reload start henki on one

Commands for you, not for the model (they cost no prompt bytes):

- `/sessions`: the 20 newest sessions of this working directory, newest first: number, id, start time (UTC) and the
  first message (60 characters). `*` marks this session.
- `/resume N` or `/resume ID`: switch to that session (henki asks first); `/resume` alone: the newest other one.
- `/fork N`: a new session that holds this one up to its message at line N, without that message: what you send next
  goes there instead, and this session stays as it is. `/fork` alone lists this session's messages by line and asks
  for one; `/fork ID@N` forks another session. henki asks first (`fork session ID at line N?`). A history binding
  that cannot fork is refused by henki, which names the fix. With no terminal to ask on (a frontend on `henki serve`),
  `/fork` alone lists the lines and refuses (exit 1): send `/fork N`.
- `/new`: close this session and start a new one.
- `/reload`: restart henki on this session (a henki you upgraded, say). It cannot load a config, AGENTS.md or module
  that changed before it ran: henki refuses a one-key restart then, and you restart henki yourself
  (`henki --resume` continues this session). `/mods` asks to restart on its own changes.

henki closes this session as `/exit` does, then starts again with the same arguments. If a trust file changed while
the command ran, henki names it and asks before it restarts; if one changed before, restart henki yourself.

They read the sessions where henki keeps them: its own session files, or the history binding `henki mods swap
history` bound (through `henki mods history list` and `show`, which you can run yourself too). A session started with
`henki --port history=std` sees henki's own files.
