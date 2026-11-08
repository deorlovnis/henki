# henki contracts

These are the formats henki shares with other programs: the session journal, the `--json` stream, what goes over the
wire to a model server, the extension layout, and the module manager's files. Each has a name and a major version
(`journal/1`), and this file is its specification. A module's manifest names the contracts it needs
(`contracts = ["ext/1"]`), and `henki-mods` refuses a module that needs one this henki does not ship.

The tests in `tests/contracts.rs`, with the fixtures in `tests/fixtures/contracts/`, hold henki to this file. They read
the index, the field tables and the exit table from it, and fail when a record henki writes has a field that is not
listed or lacks a required one, or when a listed field or exit code stops showing up. The rules in prose are pinned by
tests written from them, not read from here: a change to one fails a test only when henki's behaviour changes too.
A change to a contract is made here, in the code and in its test in the same commit.

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
| `ext/1` | henki | the extension directory layout henki loaded first; valid under ext/2 |
| `ext/2` | henki | the module directory layout henki loads: doc, tools, user commands, bindings |
| `handoff/1` | henki | a user command's environment, and what it hands back to henki in `HENKI_NEXT` |
| `observe/1` | henki | a module's observers: the session's events, how henki runs them, and what it waits for |
| `port/1` | henki | how henki talks to a port binding: a program it starts, JSON lines both ways |
| `history/1` | henki | the history port's methods over port/1: where sessions are kept |
| `policy/1` | henki | the policy port's methods over port/1: who may run a call |
| `conform/1` | henki | the conformance suite files a binding is held to |
| `module/1` | henki-mods | the module manifest, `.henki/module.toml` |
| `lock/1` | henki-mods | the module lock, `mods.lock` |
| `bind/1` | henki-mods | the lock lines of bound ports, in `mods.lock` |
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
  as long as what it wrote and that its path still names that file (an edit that puts a new file in its place, as
  `sed -i` does, keeps the size); if not, the session ends `error tampered`.

### Record kinds

Notation: `a` is a field that is always present (it may be `null` where the notes say so); `a?` is present only in the
case the notes give; `a.b` is field `b` of the object `a`; `a[].b` is field `b` of each object in the array `a`. The
values of `args` and `blocks` are opaque JSON.

| Kind | Fields |
|---|---|
| `session` | `t` `ts` `v` `henki` `id` `cwd` `model` `dialect` `base_url` `base_url_from` `approve` `approve_from` `config` `tools` `shell` `exts` `pass_env` `ctx` `config_hash` `allow_hash` `agents_hash` `project_agents_hash` `resumed` `resumed.from_bytes?` `ports?` `ports.history?` `ports.policy?` |
| `user` | `t` `ts` `msg` `msg.content` `msg.role` |
| `assistant` | `t` `ts` `msg` `msg.content` `msg.role` `msg.tool_calls` `msg.tool_calls[].args` `msg.tool_calls[].id` `msg.tool_calls[].name` `msg.tool_calls[].raw?` `finish` `usage` `usage.cached` `usage.in` `usage.out` `blocks?` |
| `ask` | `t` `ts` `id` `name` `args` `target?` |
| `approval` | `t` `ts` `id` `verdict` `rule?` `by?` `reason?` |
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
  cache reads, `usage.out` the output tokens. `blocks` (anthropic only) is the reply's content blocks as henki assembled
  them from the stream: a `tool_use` block's `input` the parsed arguments (`{}` when they are not a JSON object), its
  `id` the call's id as journaled, text blocks with only whitespace left out. They are replayed exactly as journaled.
- `ask`: the call `id` needs a human; `target` is the file a `read`, `write` or `edit` call resolves to, as shown.
  In `henki run` an `ask` ends the turn with `needs_approval`.
- `approval`: who let call `id` run, or not: `verdict` is `auto` (no approval needed), `rule` (allow rule number
  `rule`), `policy` (a policy binding allowed it; `by` names it as the binding notice does), `yes`, `always` (the user
  saved rule number `rule`, or a policy binding kept it), `no`, or `deny` (a policy binding refused it; `reason` is the
  call's result text).
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
`hN`. N starts at one more than the highest `hN` id in the history the session (or the resumed segment) started with,
1 when none, and goes up by one per name given, skipping ids already used; an `hN` id the model sends meanwhile does
not move it. Results and approvals refer to calls by that id.

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
  block; an `assistant` record its `blocks` exactly as journaled, or (a reply cut short, journaled without them) its
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
  retried only when nothing came before it that counts as the reply's start: in anthropic a text, reasoning or call
  part (a `message_start` or `ping` does not count); in openai any data event (a chunk with only a `role` or only
  `usage` counts, so after the usual role chunk an error is not retried).
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
  may be there). Any other entry is refused (`unknown entry PATH; ...`), unless ext/2 names it.
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

## ext/2

ext/2 is ext/1 with user commands, observers and bindings, and stricter text and link rules. henki loads every module
under it; an ext/1 module stays valid unless it holds a link out of itself or a doc.md that fails the text rules below.

```
NAME/doc.md              required: line 1 the summary; the whole file a text (below)
NAME/bin/CMD             tools: commands first on the calls' PATH (ext/1)
NAME/cmd/CMD             a user command /CMD (handoff/1); CMD as for bin/, not exit, help, mods or allow
NAME/cmd/CMD.md          data for a command: a text (below)
NAME/on/EVENT            an observer of EVENT (observe/1); EVENT.wait for turn_start and call_start
NAME/port/PORT           a binding program (port/1); run only through a link in CONFIG_DIR/port/
NAME/eval/...            the manager's; henki ignores it
NAME/README* LICENSE* CHANGELOG* .*   ignored
```

- **Texts.** doc.md and each `cmd/*.md` are at most 16 KiB of UTF-8 without a code point henki escapes (on any line),
  regular files (or links to them) owned by the user or root and not group- or world-writable, as AGENTS.md is. A
  failure names the file and the line (`PATH:LINE: holds U+202E, which a terminal does not show; fix: remove it`).
- **Entries.** `bin/`, `cmd/`, `on/` and `port/` hold files their owner can run, named `[A-Za-z0-9._-]+`; in `on/`
  the name is an event (observe/1), in `port/` a port (`model`, `history`, `context`, `exec`, `policy`, `frontend`). A
  dot file that cannot run is ignored but watched; in `cmd/` a `NAME.md` that cannot run is data. Anything else there
  is refused. `ctx/` and any other name are refused (`unknown entry PATH; ext/2 modules hold doc.md, bin/, cmd/, on/,
  port/ and eval/`). At most 16 `on/` files over all modules. `on/` and its files run unasked, so, as a port binding,
  `on/`, the real file of each `on/` entry and every directory from it up to the module's real directory must be owned
  by the user or root and not group- or world-writable (`PATH is group- or world-writable; fix: chmod go-w PATH`);
  henki does not check the owner or mode of the module's other files and directories, nor of the ext path.
- **Links.** Inside a module every link must be relative and lead to a path inside it (`a link that leaves the module;
  fix: copy the file in`); the module directory itself may be a link (the manager's store copy).
- **Names across modules.** A `bin/` or a `cmd/` name in two modules stops the start.
- **Trust.** The module directory, doc.md, `bin/`, `cmd/`, `on/`, `port/` and every entry of them are trust files;
  the realpaths of the links among them are protected.
- **The prompt.** A module enters the system prompt (ext/1's line) only with `bin/`, or with none of `cmd/`, `on/` and
  `port/`: a module of commands, observers or bindings costs no prompt bytes. The banner says `ext NAME: CMDS` (its
  `bin/` names), `doc only` or `no tools`; `/help` lists every user command as `/CMD  (NAME)`.

## handoff/1

A typed line `/CMD REST` runs the module command `cmd/CMD`, with REST (trimmed) as one argument and none when empty;
`/mods ARGS` and `/allow ARGS` (as `/mods allow ARGS`) run the shipped manager, `henki-mods` next to henki's realpath
or else on the user's PATH (never a module's), with ARGS split at spaces. A pasted line is never a command.

- **The run.** On the terminal, in henki's process group, in the session's working directory, as `!CMD` runs: INT and
  QUIT reach it (henki waits on), TERM, HUP and a closed stdout kill it and end henki. The model is not told.
- **The environment.** The tool environment (ext/1: every module's `bin/` first on `PATH`), plus `HENKI_CONFIG` (the
  config dir), `HENKI_STATE` (the state dir), `HENKI_BIN` (henki's path), `HENKI_MODULE` (the module dir; not for the
  manager) and `HENKI_NEXT`. A std session's journal is `$HENKI_STATE/sessions/*/$HENKI_SESSION.jsonl` (journal/1);
  none is found when history is bound to another store. Tools, bindings and observers never get `HENKI_NEXT`.
- **`HENKI_NEXT`** names an empty file henki made (0600, `STATE/spill/.next-PID-RAND`). Afterwards henki reads it only
  if it is still that regular file, at most 64 KiB of UTF-8; anything else is a notice and ignored. It is removed
  then. Line 1 is `VERB [ARG]`; the rest of the file is the text of `send`.

| Line 1 | henki |
|---|---|
| (empty, or no file) | nothing |
| `send`, then TEXT | shows `[from /CMD]` and the whole TEXT, asks `send? [Y/n]` (Enter or y sends it as the next message); refused when TEXT holds a code point henki escapes |
| `resume ID` | ID as history/1's id rule (`ID@N` is refused); asks `switch to session ID? [y/N]`, then closes the session as `/exit` does and starts `henki ... --resume=ID` in its place |
| `new` | closes the session and starts `henki ...` without `--resume`, no question |
| `restart` | as `new` with `--resume=THIS`, or as `new` while the session has no message |
| anything else | a notice naming the verb |

- **Trust files.** A trust file the command changed is listed (`changed while /CMD ran: PATH`). Then no text is sent,
  the REPL refuses messages until a restart, and `new`, `resume` and `restart` ask `restart on these changes? [y/N]`
  (`switch to session ID on these changes? [y/N]`; n refuses messages). After a change from before the command, no
  verb starts a new henki: `restart henki yourself`.
- **The new henki** is `henki` (its realpath) with this henki's arguments without `--resume` and `--resume=ID`, so
  `--config`, `--approve`, `--model`, `--nested` and every `--port` stay, then `--resume=ID`; it gets the environment
  henki started with. `henki run` has no hand-off.
- **The manager** writes `restart` into `HENKI_NEXT` after every change (add, on, off, rm, swap, allow rm), and without
  `HENKI_NEXT` prints `restart henki (or /exit and start again) to load the change`. `HENKI_CONFIG`, when set, is the
  config dir it changes.

## observe/1

A module's `on/EVENT` programs watch a session. henki runs each once per event, as git runs a hook, and nothing they
do changes the session.

```
NAME/on/EVENT            runs once per EVENT; never waited for
NAME/on/EVENT.wait       turn_start and call_start only: henki waits for it, up to 2 s
```

| Event | When | `data` |
|---|---|---|
| `session_start` | the first turn of the session in this henki; a REPL session that gets no message has none | the `session` record |
| `turn_start` | each `user` record; a `run --resume` without TASK | that record; `{"t":"turn_start"}` |
| `call_start` | after a call's approval, before henki's last trust check and the run | `{"call":{"args","id","name"},"rule","target","verdict"}` |
| `call_end` | each `tool_result` record | the record |
| `question` | each `ask` record | the record |
| `turn_end` | each `turn_end` record | the record |
| `session_end` | the `close` record | the record |
| `record` | every record a turn journals, after the record's own event; not observer notices | the record |

`call_start`'s `verdict` is the `approval` record's (`auto`, `rule`, `policy`, `yes`, `always`), `rule` the rule's number or
null, `target` the resolved path of a file tool's target, or null (a shell call, or a path that is not UTF-8, which
JSON cannot hold). No event comes before `session_start`.

- **The run.** The program with no arguments, in the session's working directory, in a process group and session of
  its own, with the tool environment (journal/1 "Files") plus `HENKI_EVENT` (the event), `HENKI_MODULE` (the module
  dir) and `HENKI_STATE` (the state dir); never `HENKI_NEXT`. stdin is one line
  `{"data":DATA,"event":"EVENT","seq":N,"session":"ID","v":1}` in journal/1's JSON, read from a file, so a run that
  never reads it holds up nothing. `seq` counts this henki's events, so runs that overlap can be put in order. stdout
  and stderr go to henki's log, `STATE/spill/.kernel-PID.log`; the exit status is ignored. An observer whose run does
  not start gets one notice, the first time, not one per event. No run starts once a trust file changed (the tripwire
  tells): a changed `on/` file, or a trust file it runs (`bin/`, `cmd/`), is used only after a restart. Other files of
  the module (a dot file) are not trust files and may change under a running henki, so an observer should run only
  trust files.
- **Waiting.** The `.wait` runs of one event are waited for together, for at most 2 s; a signal or the run's time
  budget ends that wait as it ends any other. An observer whose run misses the 2 s gets one notice and is not waited
  for again in this henki; the run goes on. No other run is waited for, and no run is killed for being slow.
- **Slow observers.** An observer with 4 runs still going misses events, with one notice.
- **No power.** What an observer writes is the user's doing: a trust file it changes ends the turn (`error changed`)
  as any other change does, `call_start`'s included, since henki checks after it. Observer notices are journaled as
  `notice` records and are never events.
- **The session end.** After `close`, the runs still going get 300 ms; then henki kills each one's process group and
  session. An observer must not leave processes behind: one it leaves counts among the background processes of
  earlier calls in notices, and on Linux goes in the next stop's sweep or the session end's.

## module/1

Owned by henki-mods; henki never reads it. The manifest is `NAME/.henki/module.toml`: flat `key = value` lines in
henki's config grammar (strings, integers, arrays of strings, `#` comments). Unknown keys are refused.

```
henki_module = 1                 # required
name = "halti"                   # required, the directory name: [a-z0-9][a-z0-9_-]{0,31}
version = "0.1.0"                # required, a label; the pin is the commit and the hd1
summary = "..."                  # optional, at most 160 bytes; defaults to doc.md line 1
license = "Apache-2.0"           # required, an SPDX id or expression
requires = ["henki >= 0.1"]      # optional
contracts = ["ext/1"]            # optional; refused when this henki's index lacks one
os = ["macos", "linux"]          # optional; refused on another OS
needs = ["git"]                  # optional; looked for on PATH, never installed (a warning, not a refusal)
shadow = ["halti/pbpaste"]       # optional; the NAME/COMMAND entries the user must add to the config's shadow
caps_net = "none"                # optional: none | loopback | any (declared, NOT enforced)
maintainer = "..."               # optional
ports = ["history"]              # optional; exactly the programs in port/ (port/1), each a port henki can bind
```

A module without a manifest is unmanaged (allowed for path and URL installs, and listed as such). `port/PORT` is a
program its owner can run (a file, or a link inside the module), PORT one that henki binds (`history`). `cmd/` and
`eval/` are ext/2's, `on/` observe/1's; `.henki/template` may hold a program that `cmd/` links run. The directory
`ctx/` is reserved for a later layout: henki-mods refuses it.

A git source (`URL[@REF][#SUBDIR]`) is cloned only over `https`, `http`, `ssh`, `git` or `file` URLs, or scp's
`USER@HOST:PATH`; a remote helper (`ext::`, `fd::`, any other `SCHEME::` or unknown `SCHEME://`) is refused before
git runs. The module is copied from the commit's raw blobs, and refused when a `.gitattributes` in it or in a
directory above it sets `export-subst`, `export-ignore`, `ident`, `filter`, `text`, `eol`, `crlf` or
`working-tree-encoding` (a checkout would hold other bytes), or when two of its paths differ only in ASCII case (one
name on a Mac).

## lock/1

Owned by henki-mods. `CONFIG_DIR/mods.lock` holds one line per module, fields separated by tabs, lines sorted by name,
written atomically (a temp file, then a rename):

```
lock/1 NAME STATE VERSION SOURCE_KIND SOURCE COMMIT HD1 ADDED HOW MASK
```

- STATE `on` or `off`; SOURCE_KIND `official`, `path` or `url`; SOURCE the module's name (official), the path, or
  `URL` or `URL#SUBDIR`; COMMIT 40 hex digits or `-`; HD1 the module's `hd1:` hash; ADDED the UTC time,
  `YYYY-MM-DDTHH:MMZ`; HOW `tty` (answered on the terminal) or `yes` (`--yes`); MASK empty in this version.
- A line whose first field is not `lock/1` (or `bind/1`, after them) is refused: `this lock is newer than this
  henki-mods; upgrade henki`.
- On means the link `CONFIG_DIR/ext/NAME` points at the module's store copy; off means no link.

## bind/1

Owned by henki-mods. After the lock/1 lines, `mods.lock` holds one line per port bound to a module, sorted by port:

```
bind/1 PORT NAME ID ADDED HOW
```

- NAME is the module, ID the first 12 hex digits of its store copy's name (its commit, else its hd1), ADDED and HOW
  as in lock/1. The binding is the link `CONFIG_DIR/port/PORT` -> `DATA/henki/mods/NAME/ID/port/PORT`, which
  `henki-mods swap PORT NAME` writes (after the conformance suite passes and the user says yes) and `swap PORT std`
  removes. henki reads the link only, never this line.
- `henki-mods rm NAME` is refused while a link in `CONFIG_DIR/port/` leads into NAME's store copies. A henki-mods that
  does not know `bind/1` refuses the lock, so it never drops a binding it cannot see.

## hd1

Owned by henki-mods. A module's tree hash is `hd1:` and the lowercase hex SHA-256 of a text with one line per file or
symbolic link, `MODE SHA256 PATH\n`, sorted bytewise (`LC_ALL=C`) by PATH:

- MODE is `644`, `755` (any execute bit set) or `link`; SHA256 is the hex SHA-256 of the file's bytes, or of a link's
  target string; PATH is relative to the module root, with `/`.
- Directories are not listed. `.git` is left out. A link whose target is absolute, leaves the module, or goes through
  another link (a link as any name of the target but the last) is refused when the module is added.

## port/1

A port is a part without which there is no agent: `model`, `history`, `context`, `exec`, `policy`, `frontend`. Each
has one binding: the std one built into henki, or a program that `CONFIG_DIR/port/PORT` links to. This henki binds
`history` and `policy` that way; `--port PORT=std` (repeatable, all six names) runs a session on std whatever the links say.

- **The link.** henki reads `CONFIG_DIR/port/` at start: names starting with `.` are skipped; any other name must be a
  port this henki binds, and a link to a regular file its owner can run, that file and its directory owned by the user
  (or the file by root) and not group- or world-writable. Anything else stops the start (`error config`) with the fix
  `henki mods swap PORT std, or start with --port PORT=std`. The directory, the link and the program are trust files,
  and the program is protected from the model's write and edit tools. The binding's name in notices and records is
  `NAME ID` for a program at `.../mods/NAME/ID/port/PORT`, else its path.
- **The process.** Once per session, before the first record, henki runs the program with no arguments in the session's
  working directory, in a session and process group of its own, without a terminal. Its environment is the tool
  environment without the extensions' `bin/` (the allowlist and `pass_env`, `HENKI_DEPTH` one deeper, `HENKI_SESSION`
  empty, no API key). Its stderr goes to henki's kernel log, `STATE/spill/.kernel-PID.log` (0600, removed with the
  spill files after 24 hours). It must exit when its stdin closes, and leave no process behind: at the session's end
  henki sends `bye`, waits 300 ms for the answer, then kills the process group and, on Linux, every process of the
  binding's session.
- **Lines.** UTF-8 JSON, one object per line, `\n` after it, at most 16 MiB each way. henki writes canonical lines
  (compact, keys sorted, invisible code points escaped). A request is `{"id":N,"method":M,"params":P}`, ids from 0, one
  at a time. An answer is `{"id":N,"result":R}` or `{"error":{"code":C,"message":S},"id":N}`. A line with another id
  is dropped; a line without `id` (or with `"id":null`) is a notification, which this version reads and skips.
- **hello**, the first request: params `henki` (its version), `port`, `protocol` (`[1]`), `versions` (the port's
  method set majors henki offers, `[1]`), `scope` (the session's working directory) and `state`
  (`STATE/port/PORT/`, 0700, the binding's own dir). The answer: `{"caps":[...],"impl":V,"name":N,"protocol":1,
  "version":1}`, `name` and `impl` 1 to 64 bytes without spaces or invisible characters, at most 16 `caps`; or error
  `version`. `NAME IMPL` is shown in parentheses in the binding notice.
- **Codes** a binding may send: `invalid`, `unsupported` (a method it does not know), `io`, `not_found`, `busy`,
  `exists`, `tampered`, `corrupt`, `version`; anything else arrives as `other`. henki adds `timeout`, `crashed`,
  `protocol` and `interrupted`.
- **Timeouts.** `hello` 5 s, `create` 2 s, `open` 5 s, `read` 10 s per page, `list` 5 s, `append` 2 s, `discard` 2 s,
  `decide` 5 s, `record` 2 s, `bye` 300 ms. A signal or the session deadline ends a wait for `hello`, `create`, `open`,
  `read`, `list` or `decide` within 50 ms, never one for a method that changes state.
- **The dead rule.** A timeout, a closed stdout, a line over the cap or cut at EOF, a non-object line, or an answer with
  both or neither of `result` and `error` kills the binding for the rest of the session: every later call fails at once.
  An error answer is one failed call; the binding lives on.
- **Visibility.** A bound port is a notice at start (`history: NAME ID (port/1, N V); henki --port history=std runs a
  session without it`), and the journal's `session` record names it in `ports` (`{"history":"NAME ID"}`).
- **`henki port PORT [--offer V]... -- PROGRAM`** (not in the usage text): no config read, the same environment, the
  state dir `STATE/port/PORT/`, the working directory as scope. It prints the hello result as `{"result":{"caps":[...],
  "name":"N V"}}` (or the error, then exit 2), then for each request line on stdin, `{"method":M,"params":P}`, one
  canonical answer line, `{"result":R}` or `{"error":{"code":C,"message":S}}`. `{"kill":true}` kills the binding
  without `bye` and ends it. The conformance suites run through it.

## history/1

The history port over port/1: where sessions are kept. Records are journal/1 objects; a binding keeps them as given
(it may add fields named `x-*`). Session ids follow journal/1's rule for any id henki makes, and any id is 1 to 64 of
`A-Z a-z 0-9 . _ -`, not starting with `.` or `-`. A session is held by one binding process at a time, from `create`
or `open` until another `create` or `open`, `bye`, or the process's end (a kill included).

| Method | Params | Result | Errors |
|---|---|---|---|
| `create` | `scope`, `hint` (an id or null) | `{"session":ID}`, held; `hint` is used when free | `invalid`, `exists`, `io` |
| `open` | `scope`, `session` (an id, looked up in every scope, or null: the newest session of `scope` nothing holds) | `{"session":ID}`, held; the id asked | `not_found`, `busy`, `corrupt`, `io` |
| `read` | `session`, `from` (a record index) | `{"records":[...],"next"?:N,"notices"?:[...]}`; `next` (the count so far) asks for another page | `not_found`, `tampered`, `corrupt`, `io` |
| `append` | `session` (held), `record` | `null`, once the record is durable (it survives a kill of the binding and of henki) | `not_found`, `invalid`, `tampered`, `io` |
| `discard` | `session` (held) | `{"removed":B}`: a session this process created that has no `user` record is removed | `not_found` |
| `list` | `scope` | `{"sessions":[{"session":ID,"updated":SECONDS},...]}`, newest first | `io` |
| `bye` | `{}` | `null`, then exit | |

henki checks every answer as it checks its own journal: the id rule, `read` records are objects with a string `t`,
`next` grows and equals the count so far, at most 256 MiB over all pages; then the replay, the repairs (appended) and
the pairing of calls and results at start. A history failure in a session ends the turn `error journal` (exit 2) with
the fix `henki --port history=std for this session, or henki mods swap history std`; henki never falls back to std in
a session. `list` is for tools (henki-mods, an import); henki itself does not call it. The cap `fork` is reserved.

## policy/1

The policy port over port/1: who may run a call. henki asks it about every call that passed henki's own checks (the
call's arguments, its target, the tripwire) and before the call runs.

| Method | Params | Result |
|---|---|---|
| `decide` | `call` (`args`, `id`, `name`), `cwd`, `root` (the project root), `session`, `target` (a file tool's: `outside`, `path` as resolved, `protected`, `shown`; null for a shell call) | `{"decision":"allow","reason"?:S}`, `{"decision":"ask","always"?:B}` or `{"decision":"deny","reason":S}` |
| `record` (cap `record`) | `call`, `verdict` (`always`) | `null` or `{"shown":S}`, once it is kept |
| `bye` | `{}` | `null`, then exit |

- **allow** runs the call and shows `HEAD  (allowed by policy NAME ID: REASON)`; the `approval` record has verdict
  `policy` and `by`. **deny** does not run it: its result is `denied by policy NAME ID: REASON`, journaled with verdict
  `deny` and that `reason`, and it counts for the stuck rule. **ask** asks the user in the REPL, and ends `henki run`'s
  turn `needs_approval`. `always` is offered only when the binding has cap `record` and asked with `always: true`, for a
  shell call while no earlier process still runs; the user's `always` then calls `record`. A `reason` is shown escaped
  and cut at 200 characters.
- **henki's floor**, which no binding changes. A write or edit under a protected path is refused before `decide`. An
  allow is asked instead, with the notice `asked: the call touches henki's own files`, when the call's target is
  protected (the config file and dir, AGENTS.md, the state and data dirs, the module paths, the port links and their
  programs), a read too, except the files henki points the model at (a module's `doc.md`, this session's spill files),
  or when a shell command's text names a protected path, absolute or with `~/`. That text match is a seatbelt only: a
  command can name a path in ways no match sees, as with `approve = "auto"`; the tripwire stays the floor for the
  trust files.
- **Failure asks.** A binding that does not start or answer `hello` stops the start. In a session a timeout, an exit, a
  broken line or a decision that is none of the three asks, with one notice naming the binding; the binding is then
  dead for the session and every later call is asked at once. A signal or the session deadline ends a `decide` within
  50 ms and the turn with it.
- **Std.** `approve = "auto"` is a mode of the std policy: henki refuses to start with it while a policy binding is
  linked (`error config`). While one is linked henki reads no allow rules; `--port policy=std` runs a session on std.

## conform/1

A conformance suite is a file of steps, `PORT.tsv`, shipped with henki-mods (`manager/conform/` in the repository),
and `henki-mods test --port PORT NAME|/PROGRAM` runs it through `henki port`. One step per line, TAB-separated,
`#` comments:

```
NAME INST REQUEST EXPECT [SAVE]
```

- INST is `a` or `b`, an instance (`henki port PORT -- PROGRAM`, started on first use), `a!` (kill instance a without
  `bye`; the next `a` step starts a fresh one), or `+V` (a fresh instance offering version V).
- REQUEST is `hello` (the instance's first line) or `{"method":M,"params":P}`. `@NAME@` is a saved value; `@SCOPE@` and
  `@SCOPE2@` are two empty directories, `@BIG@` 1 MiB of text.
- EXPECT is the canonical answer line, where `*` is any text and `@(A|B)` is A or B; every other character stands for
  itself. SAVE is `NAME=KEY`: the first string value of `"KEY"` in the answer.
- A step named with a trailing lowercase letter prepares a check and counts only when it fails. After the steps, each
  `henki port` must end within 1 s of its stdin closing, and the program run alone must exit within 1 s of EOF on its
  stdin. The std binding passes the same file in henki's own tests.

