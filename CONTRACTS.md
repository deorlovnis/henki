# henki contracts

These are the formats henki shares with other programs: the session journal, the `--json` stream, what goes over the
wire to a model server, the extension layout, and the module manager's files. Each has a name and a major version
(`journal/1`), and this file is its specification. A module's manifest names the contracts it needs
(`contracts = ["ext/1"]`), and `henki-mods` refuses a module that needs one this henki does not ship.

The tests in `tests/contracts.rs`, with the fixtures in `tests/fixtures/contracts/`, hold henki to this file: they read
the field tables below and fail when a record henki writes has a field that is not listed, lacks a required one, or a
listed field or rule stops showing up. A change to a contract is made here and in the code in the same commit.

## Stability

- Within a major, changes are additive: a new record kind, a new optional field, a new accepted response shape, a new
  optional request header. A reader skips a record kind it does not know and ignores a field it does not know; henki
  does the same with its own journal.
- Anything else is a break: removing or renaming a field, changing its type or meaning, making an optional field
  required, changing a rule or an exit code. A break needs a new major (`journal/2`), and henki supports the old and the
  new major side by side for at least one release, during which this file specifies both.
- Values marked "may change" below (message texts, for one) are not part of the contract: a reader shows them, it does
  not parse them.

## Index

A program reads the contracts this henki ships from the rows of this table: each starts with ``| `NAME` |``.

| Contract | Owner | What |
|---|---|---|
| `journal/1` | henki | the session journal: files, records, identity, resume, repair |
| `json/1` | henki | the record stream of `henki run --json` |
| `wire/1` | henki | what henki sends to and accepts from a model server, in both dialects |
| `ext/1` | henki | the extension directory layout henki loads |
| `module/1` | henki-mods | the module manifest, `.henki/module.toml` |
| `lock/1` | henki-mods | the module lock, `mods.lock` |
| `hd1` | henki-mods | the tree hash that pins a module's files |

## journal/1

Every session is one journal file. henki writes it and nothing else may: henki refuses a journal that changed behind
it. Other programs read it, to list, show, search or replay sessions.

### Files

- The state dir is `$XDG_STATE_HOME/henki` when `XDG_STATE_HOME` is absolute, else `~/.local/state/henki`, created
  0700. A session lives at `sessions/SCOPE/ID.jsonl` (mode 0600), where SCOPE is the FNV-1a 64 hash (16 lowercase hex
  digits) of the bytes of the working directory's realpath.
- ID is `YYYYMMDDTHHMMSSZ-XXXX`: the UTC start time and 4 random lowercase hex digits. A session's id is its file's
  stem, whatever the records say.
- A running henki holds an exclusive `flock` on the file; a locked session is in use and is not resumed. An empty file
  is no session. A REPL that quits before its first message removes its file.
- A tool output too long for the model goes to `spill/ID-N.log` (N counts from 1 within the session), deleted after
  24 hours. Records name such files in their text only.

### Lines

- One record per line: a JSON object, compact, keys sorted, `\n` after it. Code points that henki escapes on a
  terminal (controls, bidi and other invisible characters) are written as `\uXXXX`, so a line is safe to print.
- Every record has `t`, its kind, and `ts`, the UTC time it was written (`YYYY-MM-DDTHH:MM:SSZ`).
- henki appends only, through one `O_APPEND` descriptor, and syncs the file (fdatasync) after every record but `ask`,
  `approval` and `notice`, before the effect the record explains. Before each write it checks that the file is exactly
  as long as what it wrote; if not, the session ends `error tampered`.

### Record kinds

Notation: `a` is a field that is always present (it may be `null` where the notes say so); `a?` is present only in the
case the notes give; `a.b` is field `b` of the object `a`; `a[].b` is field `b` of each object in the array `a`. The
values of `args` and `blocks` are opaque JSON.

| Kind | Fields |
|---|---|
| `session` | `t` `ts` `v` `henki` `id` `cwd` `model` `dialect` `base_url` `base_url_from` `approve` `approve_from` `config` `tools` `shell` `exts` `pass_env` `ctx` `config_hash` `allow_hash` `agents_hash` `project_agents_hash` `resumed` `resumed.from_bytes?` |
| `user` | `t` `ts` `msg` `msg.content` `msg.role` |
| `assistant` | `t` `ts` `msg` `msg.content` `msg.role` `msg.tool_calls` `msg.tool_calls[].args` `msg.tool_calls[].id` `msg.tool_calls[].name` `msg.tool_calls[].raw?` `finish` `usage` `usage.cached` `usage.in` `usage.out` `blocks?` |
| `ask` | `t` `ts` `id` `name` `args` `target?` |
| `approval` | `t` `ts` `id` `verdict` `rule?` |
| `tool_result` | `t` `ts` `id` `name` `content` `is_error` `ms` `bytes` |
| `notice` | `t` `ts` `text` |
| `elide` | `t` `ts` `ids` |
| `turn_end` | `t` `ts` `status` `steps` `usage` `usage.cached` `usage.in` `usage.out` `reason?` |
| `result` | `t` `ts?` `status` `exit` `text` `usage` `usage.cached` `usage.in` `usage.out` `session` `steps` `duration_s` `reason?` `error?` `error.kind?` `error.message?` `call?` `call.args?` `call.id?` `call.name?` |
| `close` | `t` `ts` `bytes` |

- `session` starts a segment (a run of henki on the file). `v` is the journal major, `1`. `henki` is henki's version,
  `id` the session id, `cwd` the working directory, `model` and `dialect` (`openai` or `anthropic`) the model in use,
  `base_url` the server as `http://HOST:PORT/PATH`, `base_url_from` `config` or `HENKI_BASE_URL`. `approve` is `ask`
  or `auto`, `approve_from` `default`, `--approve` or `FILE:LINE`. `config` is the config file's path or `null`.
  `tools` lists the built-in tools offered (in the order `shell`, `read`, `write`, `edit`), `exts` the extension names,
  `pass_env` the variable names passed to tools, `shell` the bash that runs calls, `ctx` the context window in tokens.
  The `*_hash` fields are FNV-1a 64 hashes of the bytes of the config file, the allow file and the global `AGENTS.md`
  as read, or `null` when absent or not read; `project_agents_hash` is always `null` in this version. `resumed` is
  `null` for a new session, else `{"from_bytes": N}`: the file's length when this segment began.
- `user` is a message from the user; `msg.role` is `user`. A session's first task starts with
  `cwd: DIR\nos: OS ARCH\ndate: YYYY-MM-DD\n\n`; later ones do not.
- `assistant` is one model reply; `msg.role` is `assistant`, `msg.content` its text. Each `msg.tool_calls` entry is a
  call: `id`, `name` (as the model sent it), `args` (the arguments; `{}` when they did not parse) and `raw` (the
  argument text as sent, only when it did not parse). `finish` is the server's finish reason (`stop`, `tool_calls`,
  `length`, or what the server sent), or `interrupted` or `error` for the partial text of a reply that never ended:
  such a record has no calls. `usage.in` counts every prompt token (cache reads and writes included), `usage.cached` the
  cache reads, `usage.out` the output tokens. `blocks` (anthropic only) is the reply's content as received, replayed
  exactly.
- `ask`: the call `id` needs a human; `target` is the file a `read`, `write` or `edit` call resolves to, as shown.
  In `henki run` an `ask` ends the turn with `needs_approval`.
- `approval`: who let call `id` run, or not: `verdict` is `auto` (no approval needed), `rule` (allow rule number
  `rule`), `yes`, `always` (the user saved rule number `rule`) or `no`.
- `tool_result`: the result of call `id`; `content` is what the model sees (its wording may change), `is_error` whether
  it failed, `ms` how long it ran, `bytes` the output's full size before any cut.
- `notice`: something henki told the user (`text` may change).
- `elide`: from here on, the `tool_result`s with these `ids` are replaced by `[output elided: N bytes]` (N their
  content's length, with `,` thousands separators) in every request.
- `turn_end`: the end of one turn: `status` (see json/1), `steps` (model replies in the turn), `usage` (the turn's),
  `reason` when the status is `gave_up`.
- `result`: the end of a `henki run` (a REPL session has none): see json/1. In the journal it always has `ts`.
- `close`: the session ended cleanly; `bytes` is the file's length before this line.

### Segments, close and resume

- A file holds one or more segments; each starts with a `session` record. `henki --resume` (the newest unlocked session
  of this working directory's scope) or `--resume=ID` (any scope) appends a new segment to the same file.
- Resume reads whole lines. An incomplete last line (no `\n`) is a torn write: it is cut off, with a notice. A complete
  line that is not a JSON object fails the resume (`error journal`, `corrupt journal PATH:LINE`), and so does a file
  without a `session` record.
- In the last segment, a `close` must be the last record and its `bytes` must equal its own offset; anything else ends
  the resume `error tampered`. A last segment without `close` resumes with the notice
  `previous run did not close cleanly`.
- The last `session` record's `dialect` must be the configured one (else `error journal`). A different `model`,
  `tools` or `cwd` is only a notice.

### Replay

- The history is every `user`, `assistant` and `tool_result` record of the file, in file order, across all segments,
  with each `elide` applied to the results it names. Every other kind, a kind henki does not know included, is
  skipped; fields henki does not know are ignored.
- Every request renders that history (wire/1), so a record once written is sent the same way every time.

### Repair and pairing

- Pairing, checked over the history before every request: call ids are unique; each call has exactly one result,
  after its `assistant` record and before the next `assistant` or `user` record; no result is without an open call.
- On resume, every call without a result (henki died while it ran) gets one, appended before the new `session`
  record: `content` `interrupted; the command may have partly run`, `is_error` true, `ms` 0, `bytes` 0.

### Call ids

A call keeps the id the model gave it, unless that id is empty or already used in the session; then henki names it
`hN`, N one more than the highest `hN` id so far (1 when none). Results and approvals refer to calls by that id.

## json/1

`henki run --json` writes records to stdout, one per line in the journal's line format, and nothing to stderr unless
henki itself crashes.

### Order

1. On `--resume`, the repair results it appended (journal/1), as journaled.
2. The new `session` record.
3. Every record the run journals, in journal order, without `close`; between them, as the reply streams, `text` and
   `reasoning` records.
4. Exactly one `result`, last. The process then exits with its `exit`, or 141 when stdout closed (or its reader did
   not keep up) before the result got out.

An end before a session exists (a bad argument after `--json`, a config error, an unreadable journal) writes only the
`result`: it has `session` `null` and no `ts`.

### Stream-only kinds

These are on stdout only, never in the journal, and have no `ts`.

| Kind | Fields |
|---|---|
| `text` | `t` `delta` |
| `reasoning` | `t` `delta` |

`delta` is the next piece of the reply's answer text or of its reasoning, as it streamed. The `text` pieces of a reply
add up to what the server sent as content, before henki takes any text tool calls out of it (`msg.content` holds what
remains).

### `result`

- `status` and `exit`: `ok` 0 (an answer), `gave_up` 1, `error` 2, `needs_approval` 3, `budget` 124 (out of time),
  `interrupted` 130 (INT, QUIT, TERM or HUP), `output_closed` 141.
- `reason` (with `gave_up` only): `max_steps`, `stuck` or `empty_answer`.
- `error` (with `error` only): `kind`, one of `auth`, `changed`, `config`, `context_exceeded`, `gateway`, `internal`,
  `journal`, `nested`, `provider`, `stream_incomplete`, `tampered`; `message` (may change).
- `call` (with `needs_approval` only): the call that needs a human, as `args`, `id`, `name`.
- `text` is the last reply's answer text (`""` when none), `usage` the process's tokens over all its replies, `session`
  the session id (or `null`), `steps` the turn's model replies, `duration_s` the run's wall time in seconds, to 0.1.

## wire/1

What henki sends to a model server and what it accepts back. `dialect = "openai"` (the default) speaks OpenAI-style
chat completions, as llama-server, Ollama and vLLM serve them; `dialect = "anthropic"` speaks the Anthropic Messages
API. A proxy or a test server that follows this section works with henki.

### HTTP

- `base_url` is `http://HOST[:PORT]/PATH` (port 80 when absent; no https, no user info, no query; one trailing `/`
  dropped). The request goes to `PATH/chat/completions` (openai) or `PATH/v1/messages` (anthropic).
- One `POST` per attempt over HTTP/1.1, every resolved address tried in order with a 5 s connect limit each. The head
  is, in this order: `Host: HOST:PORT`, `User-Agent: henki/VERSION`, `Content-Type: application/json`,
  `Content-Length: N`, `Accept: text/event-stream`, `Connection: close`, then the dialect's headers.
- An API key (`api_key`) is sent only to a loopback host (`localhost`, `127.0.0.0/8` as a dotted quad, `::1`) whose
  every resolved address is loopback; so is every request when `base_url` came from `HENKI_BASE_URL`.
- The body is compact JSON with sorted keys, rendered whole from the journal each time, so the messages of an earlier
  request are a byte-identical prefix of the next one's (a server's prompt cache keeps working).
- The answer henki reads: `200` with `Content-Type: text/event-stream` (server-sent events) or `application/json` (one
  whole completion, at most 4 MiB); a body that is chunked, has a `Content-Length` or ends at the close;
  `Content-Encoding` absent or `identity`. Interim `1xx` heads are skipped. The head may be 64 KiB, an event line or
  event 1 MiB. Redirects are not followed. Any other status is an error (below).
- SSE: `data:` lines (several join with `\n`), `event:` names, `:` comments; an event ends at a blank line. A comment
  or an empty `data` is a keep-alive.
- Time limits: the first body byte within 300 s, then no silence over 60 s (300 s for anthropic), an attempt within
  1,800 s. Once the reply's finish has come, the end of the body or 2 s of silence completes it.

### openai: request

Headers: `Authorization: Bearer KEY` when `api_key` is set. Body fields, exactly:

```
{"chat_template_kwargs":{"enable_thinking":false},"max_tokens":N,"messages":[...],"model":"MODEL","stream":true,
 "stream_options":{"include_usage":true},"tools":[...]}
```

- `max_tokens` is the config's (default 4096), `model` the config's `model`.
- `messages`: `{"content","role":"system"}` first, then per journal record: `user` as `{"content","role":"user"}`;
  `assistant` as `{"content","role":"assistant"}`, with `"tool_calls":[{"function":{"arguments","name"},"id",
  "type":"function"}]` when it has calls (`arguments` is the `args` object as JSON text, `{}` when they are not an
  object); `tool_result` as `{"content","role":"tool","tool_call_id"}`.
- `tools`: `{"function":{"description","name","parameters"},"type":"function"}` per offered tool, in the order `shell`
  (`command`, optional `timeout` in seconds), `read` (`path`, optional `offset`, `limit`), `write` (`path`,
  `content`), `edit` (`path`, `old_text`, `new_text`). The descriptions may change.

### openai: response

- Each event's data is a chunk: `choices[0].delta.content` (answer text), `choices[0].delta.reasoning_content` or
  `.reasoning` (reasoning), `choices[0].delta.tool_calls[]` (call fragments), `choices[0].finish_reason`, and `usage`
  (`prompt_tokens`, `completion_tokens`, `prompt_tokens_details.cached_tokens`; llama.cpp's `timings.cache_n` wins
  for the cache count) in any chunk. `data: [DONE]` ends the stream.
- A call fragment has `index`, and on its first fragment `id` and `function.name`; `function.arguments` is a string
  appended across fragments (an object is taken whole). A fragment without `index` continues the last call unless it
  brings a new id or is a later entry of its chunk. A name split over fragments is joined.
- A JSON completion is read with the same rules from `choices[0].message`.
- With no native call and a finish other than `length`, calls written in the text are taken out of it, in text order:
  `<tool_call>{"name": N, "arguments": A}</tool_call>` (`parameters` also accepted), Qwen XML
  `<function=N><parameter=K>V</parameter>...</function>` (inside `<tool_call>` or bare), and a fenced or whole-text JSON
  object `{"name": N, "arguments": A}` when N is an offered tool. When the content holds none, the reasoning is
  searched the same way. Markup that opens and does not close is a call that does not run.
- `finish_reason` `length` runs none of the reply's calls. A stream that ends before its finish fails the attempt
  (`stream_incomplete`). An `error` member in a chunk fails the attempt.

### anthropic: request

Headers: `anthropic-version: 2023-06-01`, and `x-api-key: KEY` when `api_key` is set. Body fields, exactly:

```
{"cache_control":{"type":"ephemeral"},"max_tokens":N,"messages":[...],"model":"MODEL","stream":true,
 "system":[{"cache_control":{"type":"ephemeral"},"text":"SYSTEM","type":"text"}],"tools":[...]}
```

- `messages` alternate `user` and `assistant`, each `{"content":[blocks],"role"}`. A `user` record is a
  `{"text","type":"text"}` block; a `tool_result` record a `{"content","is_error","tool_use_id","type":"tool_result"}`
  block; an `assistant` record its `blocks` exactly as received, or (a reply cut short, journaled without them) its
  text block and a `{"id","input","name","type":"tool_use"}` block per call. Adjacent messages of one role are merged
  (results first, then the next task), text blocks with only whitespace are dropped, and empty messages left out.
- `tools`: `{"description","input_schema","name"}` per offered tool; `input_schema` is the openai `parameters`.

### anthropic: response

- Events by their data's `type`: `message_start` (`message.usage`), `content_block_start` (`index`, `content_block`),
  `content_block_delta` (`text_delta`, `thinking_delta`, `signature_delta`, `input_json_delta`), `content_block_stop`,
  `message_delta` (`delta.stop_reason`, `usage`), `message_stop` (the end), `ping`, `error`; other types are ignored.
- Usage: `usage.in` = `input_tokens` + `cache_read_input_tokens` + `cache_creation_input_tokens`, `usage.cached` =
  `cache_read_input_tokens`, `usage.out` = `output_tokens`, each the last value sent.
- `stop_reason` `max_tokens` or `model_context_window_exceeded` is finish `length` (no call runs); `refusal` ends the
  turn `error provider` and the streamed text is not kept; otherwise finish is `tool_calls` with calls, `stop` without.
- A JSON completion is refused: the API always streams.

### Errors and retries

- A non-200 answer, an `error` event, or an `error` member is classified, first match wins: a body starting
  `halti: ` is `gateway`; `400` or `413` whose message (or body) speaks of the context (`context`, `exceed`,
  `too long`, `too many tokens`) is `context_exceeded`; `401`, `403`, `authentication_error`, `permission_error` are
  `auth`; `408`, `409`, `425`, `429`, `500`, `502`, `503`, `504`, `529`, `rate_limit_error`, `overloaded_error` and
  `api_error` are retryable `provider` errors; anything else is a `provider` error. The message is the body's
  `error.message` when it has one.
- Retryable too: no address connected, and a connection reset before the response. An error inside a 200 stream is
  retried only when it came before any part of the reply.
- At most 5 attempts. Before attempt N+1 henki waits `Retry-After` (seconds, decimal allowed, at most 60), else
  min(2^(N-1), 30) s, with the notice `CAUSE; retry N/4 in S s`. After the last attempt the message ends
  `(gave up after 5 attempts)`.
- `context_exceeded` (openai): henki elides old tool results once (an `elide` record) and sends again; with nothing to
  elide, or in the anthropic dialect, the turn ends with that error.

## ext/1

An extension teaches the model a command-line tool. henki looks for extensions in each directory the config's `ext`
list names (absolute paths or `~/`), else in `CONFIG_DIR/ext` (`~/.config/henki/ext`; it may be absent only for the
default config). henki runs nothing while it loads them; a violation of any rule below stops henki at start with
`error config`, the message naming the config file, then `ext PATH: PROBLEM; fix: ...`.

- Every entry of an ext directory, except names starting with `.`, is an extension directory `NAME` (a directory or a
  link to one), NAME matching `[a-z0-9][a-z0-9_-]{0,31}`. Two extensions with one NAME, across the directories, are
  refused.
- `NAME/doc.md` is required, a regular file. Its line 1 (read through 4 KiB), without a leading run of `#` and spaces,
  trimmed, is the summary: 1 to 160 bytes, without characters henki would escape.
- `NAME/bin/` is optional. Each entry is a command: a regular file (or a link to one) its owner can run, named
  `[A-Za-z0-9._-]+`. A dot file that cannot run is ignored (but watched); any other entry is refused. bin/'s path holds
  no `:` and is UTF-8. A command name in two extensions is refused.
- Names starting with `README`, `LICENSE`, `CHANGELOG` or `.` are ignored (so `.henki/module.toml` and `.henki/card.md`
  may be there). Any other entry is refused (`unknown entry PATH; v1 extensions hold doc.md and bin/ only`).
- A command with the name of an executable on the user's PATH loads only when the config's `shadow` list holds
  `NAME/COMMAND`.
- The system prompt gets, when any extension is loaded, the line
  `Extensions are commands on PATH; read the doc in brackets before first use.` and then, per extension in load order
  (ext directory order, names sorted bytewise within one), `\n- NAME: SUMMARY [DOC]`, DOC being the path of doc.md with
  the home directory shown as `~`. These are an extension's prompt bytes.
- Tool calls run with every `bin/` first on `PATH` (in name order), then the user's PATH without relative entries.
  They see `HENKI_USER_PATH` (that user PATH, for a wrapper to `exec` the command it wraps), `HENKI_SESSION` (the
  session id), `HENKI_VERSION` and `HENKI_DEPTH` (1 inside a call of a top-level henki).
- The extension directory, doc.md, bin/ and each bin/ entry are trust files: when one changes during a session, the
  turn ends `error changed`.

## module/1

Owned by henki-mods; henki never reads it. The manifest is `NAME/.henki/module.toml`: flat `key = value` lines in
henki's config grammar (strings, integers, arrays of strings, `#` comments). Unknown keys are refused.

```
henki_module = 1                 # required
name = "halti"                   # required, the directory name: [a-z0-9][a-z0-9_-]{0,31}
version = "0.1.0"                # required, a label; the pin is the commit and the hd1
summary = "..."                  # optional, at most 160 bytes; defaults to doc.md line 1
license = "Apache-2.0"           # required, an SPDX id
requires = ["henki >= 0.1"]      # optional
contracts = ["ext/1"]            # optional; refused when this henki's index lacks one
os = ["macos", "linux"]          # optional; refused on another OS
needs = ["git"]                  # optional; looked for on PATH, never installed (a warning, not a refusal)
shadow = ["halti/pbpaste"]       # optional; the NAME/COMMAND entries the user must add to the config's shadow
caps_net = "none"                # optional: none | loopback | any (declared, NOT enforced)
maintainer = "..."               # optional
ports = []                       # reserved for port bindings; must be empty in this version
```

A module without a manifest is unmanaged (allowed for path and URL installs, and listed as such). The directories
`cmd/`, `ctx/`, `on/` and `port/` are reserved for later layouts: henki-mods refuses them unless `requires` names a
henki that has them (`needs henki >= X`).

## lock/1

Owned by henki-mods. `CONFIG_DIR/mods.lock` holds one line per module, fields separated by tabs, lines sorted by name,
written atomically (a temp file, then a rename):

```
lock/1 NAME STATE VERSION SOURCE_KIND SOURCE COMMIT HD1 ADDED HOW MASK
```

- STATE `on` or `off`; SOURCE_KIND `official`, `path` or `url`; SOURCE the path, or `URL#SUBDIR`; COMMIT 40 hex digits
  or `-`; HD1 the module's `hd1:` hash; ADDED the UTC time, `YYYY-MM-DDTHH:MMZ`; HOW `tty` (answered on the terminal)
  or `yes` (`--yes`); MASK empty in this version.
- A line whose first field is not `lock/1` is refused: `this lock is newer than this henki-mods; upgrade henki`.
- On means the link `CONFIG_DIR/ext/NAME` points at the module's store copy; off means no link.

## hd1

Owned by henki-mods. A module's tree hash is `hd1:` and the lowercase hex SHA-256 of a text with one line per file or
symbolic link, `MODE SHA256 PATH\n`, sorted bytewise (`LC_ALL=C`) by PATH:

- MODE is `644`, `755` (any execute bit set) or `link`; SHA256 is the hex SHA-256 of the file's bytes, or of a link's
  target string; PATH is relative to the module root, with `/`.
- Directories are not listed. `.git` is left out. A link whose target is absolute or leaves the module is refused when
  the module is added.
