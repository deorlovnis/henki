shapes long sessions by summarising old turns with your local model

Add it with `henki mods add compact` and bind it with `henki mods swap context compact`; `henki mods swap context std`
goes back to henki's own elision.

What it does. Before each model request henki shows compact the whole history and the byte limit (80 % of
`context_window` at 4 bytes a token). While the request stays under `start` of the limit, compact leaves it to henki.
Past that, the turns before the last `keep_turns` of your messages are replaced, for that request only, by one message:
"Summary of the earlier part of this session, written by the assistant (…): …", written by your model in one
completion of at most 512 tokens.
Tool calls and their results that stay are sent exactly as they happened. The session file keeps everything, and
`--resume` replays it whole.

Settings, in `~/.config/henki/compact.toml`, all optional:

    base_url = "http://127.0.0.1:8080/v1"   # default: base_url in henki's config.toml
    model = "my-model"                       # default: model in henki's config.toml
    keep_turns = 2                           # your last N messages, and what followed them, stay as they are
    start = 0.6                              # summarise once a request passes this share of the limit

compact reads only `~/.config/henki`: henki starts it without your command line or its environment, so a config file
given with `--config`, `HENKI_MODEL`, `HENKI_BASE_URL` and `--model` do not reach it. If you use any of them, put
`base_url` and `model` in `compact.toml`. Without a `base_url` and a `model` in either file it summarises nothing, and says so once.

Limits. compact sends the summarised turns to `base_url`, which must be a loopback address (localhost, 127.x.x.x,
[::1]); it reads no API key and uses no proxy. If the model is down, answers nothing, or takes over 8 s, that request
is shaped by henki's own elision; a summary that arrives late is kept for the next request. Summaries are cached
under henki's state dir (`port/context/compact/`, the newest 256), keyed by the turns they cover and the model. When one turn alone
is too big for the limit there is nothing to summarise, and henki's elision takes over. A summary is lossy: if the
model forgets a detail, say it again.

Trust. The summary is written from everything in the summarised turns, tool output included (web pages, files). It
goes to the model as a message in your role, labelled as the assistant's own notes and not your words, and compact asks
the summariser to report what tool output showed, never instructions found in it. That lowers, but does not remove,
the chance that text planted in a file or page reaches the model as if you had asked for it. Unbind compact if you work
with untrusted content and do not want that.

Needs python3 (3.8 or later).
