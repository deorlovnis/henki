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
| `ext/1` | henki | the extension directory layout henki loaded first; a module without ext/2's entries keeps its rules |
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
| `eval/1` | henki-mods | a module's eval tasks, the two arms they run in, and the line that records them |
| `evalrun/1` | henki-mods | an eval run's dir and rows, its statistics, and the badge an index entry carries |
| `hist/1` | henki-mods | the lock lines a change replaced or removed, `mods.history` |
| `index/1` | henki-mods | a signed module index: its entries, its signature chain, and what henki-mods reads from it |
| `pack/1` | henki-mods | a pack of modules for a machine with no network: its members and its `PACK` |

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
| `close` | `t` `ts` `bytes?` |

- `session` starts a segment (a run of henki on the file). `v` is the journal major, `1`. `henki` is henki's version,
  `id` the session id, `cwd` the working directory, `model` and `dialect` (`openai` or `anthropic`) the model in use,
  `base_url` the server as `http://HOST:PORT/PATH`, `base_url_from` `config` or `HENKI_BASE_URL`. `approve` is `ask`
  or `auto`, `approve_from` `default`, `--approve` or `FILE:LINE`. `config` is the config file's path or `null`.
  `tools` lists the built-in tools offered (in the order `shell`, `read`, `write`, `edit`), `exts` the extension names,
  `pass_env` the variable names passed to tools, `shell` the bash that runs calls, `ctx` the context window in tokens.
  The `*_hash` fields are FNV-1a 64 hashes of the bytes of the config file, the allow file and the global `AGENTS.md`
  as read, or `null` when absent or not read; `project_agents_hash` is always `null` in this version. `resumed` is
  `null` for a new session, else `{"from_bytes": N}`: the file's length when this segment began (`{}` in a session a
  history binding keeps: history/1).
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
- `close`: the session ended cleanly; `bytes` is the file's length before this line (absent in a session a history
  binding keeps: history/1).

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

ext/2 is ext/1 with user commands, observers and bindings, and stricter text and link rules. A module with none of
`cmd/`, `on/`, `port/` and `eval/` is an ext/1 module, and henki holds it to ext/1's rules for at least one release
more, as the Stability section asks: only line 1 of its doc.md is read, and its links may leave it. Every other module
is held to the rules below.

```
NAME/doc.md              required: line 1 the summary; the whole file a text (below)
NAME/bin/CMD             tools: commands first on the calls' PATH (ext/1)
NAME/cmd/CMD             a user command /CMD (handoff/1); CMD as for bin/, not exit, help, mods or allow
NAME/cmd/CMD.md          data for a command: a text (below)
NAME/on/EVENT            an observer of EVENT (observe/1); EVENT.wait for turn_start and call_start
NAME/port/PORT           a binding program (port/1); run only through a link in CONFIG_DIR/port/
NAME/eval/...            tasks for the manager (eval/1); henki ignores them
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
- **Links.** Inside an ext/2 module every link must be relative and lead to a path inside it (`a link that leaves the
  module; fix: copy the file in`); the module directory itself may be a link (the manager's store copy).
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
| `restart` | as `new` with `--resume=THIS`, or as `new` while the session has no message, or after the command changed `CONFIG_DIR/port/history` (the session stays in the history it was kept in, which a notice names) |
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
  its own, with the tool environment (ext/1: every module's `bin/` first on `PATH`) plus `HENKI_EVENT` (the event),
  `HENKI_MODULE` (the module dir) and `HENKI_STATE` (the state dir); never `HENKI_NEXT`. stdin is one line
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
os = ["linux", "macos"]          # optional; refused on another OS
needs = ["git"]                  # optional; looked for on PATH, never installed (a warning, not a refusal)
shadow = ["halti/pbpaste"]       # optional; the NAME/COMMAND entries the user must add to the config's shadow
caps_net = "none"                # optional: none | loopback | any (declared, NOT enforced)
maintainer = "..."               # optional
ports = ["history"]              # optional; exactly the programs in port/ (port/1), each a port henki can bind
asset_aarch64_apple_darwin = ["bin/x", "https://...", "sha256:..."]   # optional; a prebuilt program per target
```

`asset_TARGET` names a prebuilt program for one machine: TARGET is `aarch64_apple_darwin`, `x86_64_apple_darwin`,
`aarch64_unknown_linux_musl`, `x86_64_unknown_linux_musl`, `aarch64_unknown_linux_gnu` or `x86_64_unknown_linux_gnu`
(on Linux the musl one is taken first), and the array holds the program's place (`bin/NAME`, NAME not `.` or `..`),
an https URL and the SHA-256 of its bytes (64 lowercase hex digits). At an install henki-mods downloads this machine's
one (curl, https only, at most 64 MiB), refuses other bytes, and puts it at its place in the store copy, as a program;
the module's own files must not hold that place. A module with assets but none for this machine is refused. The asset
is left out of the hd1 (which stays the hash of the module's own files, the one an index lists) and is checked by its
SHA-256 instead, whenever the store copy is (`verify`, `on`, a swap).

A module without a manifest is unmanaged (allowed for path and URL installs, and listed as such). `port/PORT` is a
program its owner can run (a file, or a link inside the module), PORT one that henki binds (`history` or `policy`).
`cmd/` and `eval/` are ext/2's (`eval/` holds eval/1 tasks), `on/` observe/1's; `.henki/template` may hold a program
that `cmd/` links run. Any other file under `.henki/` is data the module's own programs read (a library, an Agent
Skill's files): a regular file of mode 644, never a link or a program, which nothing puts on a PATH. The directory
`ctx/` is reserved for a later layout: henki-mods refuses it.

A git source (`URL[@REF][#SUBDIR]`) is cloned only over `https`, `http`, `ssh`, `git` or `file` URLs, or scp's
`USER@HOST:PATH`; a remote helper (`ext::`, `fd::`, any other `SCHEME::` or unknown `SCHEME://`) is refused before
git runs. The module is copied from the commit's raw blobs, and refused when a `.gitattributes` in it or in a
directory above it sets `export-subst`, `export-ignore`, `ident`, `filter`, `text`, `eol`, `crlf` or
`working-tree-encoding` (a checkout would hold other bytes), or when two of its paths differ only in ASCII case (one
name on a Mac). `add SRC --expect hd1:HASH` installs only a copy of that hd1 (the line `henki-mods share NAME`
prints, with the full commit).

**Agent Skills.** `henki-mods add --skill SRC [--as NAME]` makes a module from an Agent Skill (agentskills.io: a
directory whose `SKILL.md` starts with YAML frontmatter, then markdown instructions), SRC being a path or a git source
as above; the source is copied the same way, and nothing of it runs. The frontmatter is read for top-level scalars
(plain, quoted on one line, `|` and `>` blocks) and `metadata`'s block map, which is skipped; anything else is refused.
`name` follows the format (1 to 64 of `a-z`, `0-9` and inner single hyphens, the directory's name) and must also be a
henki name (else `--as`), not one henki keeps, an official module's, or one an added module or its command has; the
body is at most 16 KiB of UTF-8 without a code point henki escapes, the skill at most 200 files and 4 MiB, with no
links. The module is made the same way each time (so its hd1 is too): `doc.md` (line 1 the `description` on one line,
cut to 160 bytes, then where it came from), `cmd/NAME.md` (the body), `cmd/NAME -> ../.henki/template` (the `prompts`
module's template, which hands the text back as `send`, with a line naming `.henki/skill` when the skill has other
files), `.henki/skill/...` (the skill's other files, mode 644; `LICENSE*` at the top as well) and a manifest
(`version = "skill-"` and 12 hex digits of the commit, or of the source's hd1; `license` the frontmatter's when it is
one the index accepts, else `NOASSERTION`; `requires = ["henki >= 0.2"]`). `allowed-tools` is shown and ignored.
## lock/1

Owned by henki-mods. `CONFIG_DIR/mods.lock` holds one line per module, fields separated by tabs, lines sorted by name,
written atomically (a temp file, then a rename):

```
lock/1 NAME STATE VERSION SOURCE_KIND SOURCE COMMIT HD1 ADDED HOW FLAGS
```

- STATE `on` or `off`; SOURCE_KIND `official`, `path`, `url`, `skill` or `index:INDEX`; SOURCE the module's name
  (official), the path, `URL` or `URL#SUBDIR` (for `skill` too: the skill's directory or git source), or for
  `index:INDEX` the package's name in index INDEX (index/1);
  COMMIT 40 hex digits or `-`; HD1 the module's `hd1:` hash; ADDED the UTC time, `YYYY-MM-DDTHH:MMZ`; HOW `tty`
  (answered on the terminal) or `yes` (`--yes`).
- FLAGS is empty or a comma-separated list: `pin` (an update leaves the module as it is), `ref=REF` (the ref a URL
  install follows: the branch or tag it named, `HEAD` for the default branch; none when it named a commit),
  `yanked=malicious` or `yanked=buggy` (an index yanked this version; a malicious one stays off), `skill=hd1:HASH`
  (the hd1 of the Agent Skill's directory the module was made from; a skill from a URL has `ref=` too), `mask=cmd/NAME`, one per masked command (`henki-mods off NAME/CMD`: the
  link names a copy of the store copy without `cmd/CMD` and `cmd/CMD.md`, beside it as `ID.mHASH`, where HASH is 8 hex
  digits of the SHA-256 of the masked names, sorted, one per line; HD1 stays the module's own).
- A line whose first field is not `lock/1` (or `bind/1`, after them), or that holds a kind or flag henki-mods does not
  know, is refused: `this lock is newer than this henki-mods; upgrade henki`. So no rewrite of the lock drops what a
  later henki-mods wrote.
- On means the link `CONFIG_DIR/ext/NAME` points at the module's store copy (or its masked copy); off means no link.
- A profile is a file of lock/1 lines that `henki-mods init PROFILE` adds: lines of kind `official` (VERSION, COMMIT,
  HD1, ADDED and HOW `-`: the copy this henki ships) and of kind `index:INDEX` (a package pinned by VERSION and HD1).
  henki ships some in `share/henki/mod-profiles/NAME.lock`, an index may hold more in `profiles/NAME.lock`, and
  `henki-mods profile save FILE` writes the modules on as one. A module the lock has already is left as it is: an off
  line is never turned on by a profile. Each module is staged and checked as `add` does: a package yanked or removed
  as malicious refuses.

## bind/1

Owned by henki-mods. After the lock/1 lines, `mods.lock` holds one line per port bound to a module, sorted by port:

```
bind/1 PORT NAME ID ADDED HOW
```

- NAME is the module, ID the first 12 hex digits of its store copy's name (its commit, else its hd1), ADDED and HOW
  as in lock/1. The binding is the link `CONFIG_DIR/port/PORT` -> `DATA/henki/mods/NAME/ID/port/PORT`, which
  `henki-mods swap PORT NAME` writes (after the conformance suite passes and the user says yes) and `swap PORT std`
  removes. henki reads the link only, never this line.
- HOW `yanked`: an index yanked NAME as malicious while it held a port that has no safe fallback (any but `policy` and
  `context`, which go back to std). The link is then `CONFIG_DIR/port/PORT -> yanked`, which names nothing, so henki
  refuses to start until the user runs `henki-mods swap PORT std` (or another package), or starts with
  `--port PORT=std`: a sandbox or a history store never silently falls back.
- `henki-mods rm NAME` is refused while a link in `CONFIG_DIR/port/` leads into NAME's store copies. A henki-mods that
  does not know `bind/1` refuses the lock, so it never drops a binding it cannot see; one that knows it refuses a line
  for a port it does not bind the same way (`this lock is newer than this henki-mods`).

## hist/1

Owned by henki-mods. `CONFIG_DIR/mods.history` keeps every lock/1 line a change replaced or removed, one per line,
appended:

```
hist/1 TIME WHY LOCK_LINE
```

- TIME is the UTC time (`YYYY-MM-DDTHH:MMZ`), WHY the verb (`rm`, `update`, `rollback`, `unpack`, `init`) and
  LOCK_LINE the whole lock/1 line as it was, its tabs included. A rollback takes the pin it goes back to from here,
  never from the module's source.
- The store (`DATA/henki/mods/`) and the index clones are shared by every config dir henki-mods runs on, each listed
  once in `DATA/henki/mods.users`. `henki-mods gc` keeps whatever the lock, the port links, the history and
  `mods.index` of any of them name.

## index/1

Owned by henki-mods. A module index is a git repository whose `main` lists modules someone has read, each pinned to a
full commit and its hd1; it never holds module code. henki-mods reads it to search, show and install modules, and to
apply yanks.

- **Files.** `mods/NAME.toml` (a package), `mods/NAME/VERSION.toml` (one per indexed version), `profiles/NAME.lock`
  (a profile, lock/1), `removed.toml`
  (`NAME = "malicious|buggy|withdrawn: REASON (YYYY-MM-DD)"`, a name taken out for good), `.signers` (who may sign
  `main`, git's allowed_signers format), and the index's own gate and docs. Entries are in module/1's grammar; an
  unknown key is refused, and `index = 2` is a later format (`this index is newer than this henki-mods`).
- **A package**: `index = 1`, `name` (the file's), `kind` (`git`, or `official` for a record about a module henki
  ships, never installed from the index), for `git` a `repo` (an https URL; henki-mods also reads a `file:///` one,
  which the index's own gate refuses, for an index on a local disk: hd1 binds the files either way) and a `path` (the
  module's directory in it, `""` for the root), `summary` (at most 160 bytes), `maintainer`, `license`, `latest` (an
  indexed version, not yanked) and `added` (`YYYY-MM-DD`).
- **A version**: `index = 1`, `version` (the file's), `commit` (40 hex digits), `hd1` (of the module's directory at
  that commit, from its raw blobs), the keys the module itself decides (`requires`, `contracts`, `os`, `ports`,
  `commands`, `bins`, `observers`, `ctx`, `prompt_bytes`, `card_bytes`, `caps_net`; `henki-mods-dev entry derive`
  prints them), `conform` with `ports`, `asset_TARGET` and `asset_check_TARGET` for a prebuilt binary, `reviewed`
  and `reviewer` (who read this version, and when), `yanked` (`malicious|buggy|withdrawn: REASON (YYYY-MM-DD)`) and
  the `eval_*` keys of a badge, all or none.
- **The signature chain.** Every commit on `main`'s first-parent line is SSH-signed (namespace `git`) by a key listed
  in the `.signers` of its first parent (at most 64 KiB), the root commit by a key of the anchor; a line whose key
  blob is in the refused keys never counts, and neither does an OpenPGP or X.509 signature, whatever the person's own
  gpg says of it. A key changes only through a commit, signed by a current key, that edits `.signers`. At most 5,000
  commits are on the line.
- **Anchors.** The default index's are beside henki-mods (`share/henki/index/`, or `HENKI_SHARE/index/`, which every
  index screen then names): `NAME.url`, `NAME.signers` (its root's keys; with no key line the index is not live yet),
  `NAME.checkpoint` (a commit on its first-parent line) and `refused.keys` (key blobs that never verify, one per
  line). More indexes are `index/1<TAB>NAME<TAB>URL` lines in `CONFIG_DIR/mods.index`, each with its pinned
  `CONFIG_DIR/index/NAME.signers` and, optionally, `NAME.checkpoint` (`henki-mods index add NAME URL --signers FILE
  [--checkpoint COMMIT]` writes them after showing the keys' fingerprints, `index rm NAME` takes them out). Index URLs are https, ssh or file. A first sync
  of the default index with no checkpoint beside henki-mods is refused; elsewhere (`HENKI_SHARE`, `mods.index`) it is
  a warning.
- **A sync** fetches `main` into the bare clone `DATA/henki/index/NAME.git` (`refs/henki/fetched`). The commit verified
  before (`refs/henki/verified`), or at a first sync the checkpoint, must be on the fetched first-parent line: no
  rollback, and no branch grafted by a key since rotated out. Then the chain is checked for the commits after it, and
  `refs/henki/verified` moves to the fetched commit; on any refusal, or offline, the verified commit stays in use.
  `DATA/henki/index/NAME.synced` holds the time of the last good sync. `index add NAME` starts a new clone (one left
  by `index rm` is removed first), unless another config dir uses the same index (URL, anchor and checkpoint); a
  different one by that name is refused.
- **Reading.** Entries are read with git from the verified commit only, never from a checkout. Once per run before
  the first read, that commit's own signature is checked again against the `.signers` of its first parent: a ref
  edited to point at an unsigned commit is refused, but a history crafted by someone who can write `DATA` is not
  (only a sync checks the chain from the anchor). An entry henki-mods cannot read is skipped with a warning; a version file it parses but skips (an unknown key or value, a later `index`) still yanks, and an install
  that finds no version names the skipped ones. A verified commit over 30 days old is a warning on every index screen.
- **Tiers.** A version with `reviewed` and `reviewer` shows `reviewed by REVIEWER (signed index PRINCIPAL, DATE)`,
  PRINCIPAL being the anchor's; `source only` is added when the asset for this machine has `asset_check = "none"`;
  without a review it is red, `UNREVIEWED: listed in PRINCIPAL, but no reviewer has read this version`. A URL or path
  install whose hd1 an index lists shows that version's review.
- **Names.** `add NAME` takes an official module of that name first, then the one index that lists the package
  (`NAME@INDEX` when more than one does). An index from `mods.index` never provides a name henki keeps (`std`,
  `henki`, `mods`, `allow`, `help`, `exit`, `index`, the ports' names, `x-*`) or an official module's.
- **Yanks** act only on what their index provided: on an `index:INDEX` lock line, index INDEX's yank of that version,
  its removal of the package, or its yank of a version with the same hd1; on a `url`, `path` or `skill` line, a yank
  of a version with the same hd1 in the default index; on an `official` line, nothing. They are applied at every
  sync, and checked at every install from the verified commit, with no network. A malicious one refuses the install;
  at a sync it turns the module off (a pin too) with the flag `yanked=malicious`, puts `policy` and `context`
  bindings of it back to std, and makes any other binding of it the `yanked` marker (bind/1). A buggy or withdrawn
  one is said and flagged; nothing else changes.

## pack/1

Owned by henki-mods. `henki-mods pack [NAME...] [--with-index] -o FILE` writes modules into a gzip ustar archive that
`henki-mods unpack FILE` installs on another machine without the network. The same store copies give the same bytes:
members sorted by name, owner 0, every time 1970-01-01, gzip without a name or time. Members:

- `PACK`: line 1 `pack/1`; then one lock/1 line per module (FLAGS without a yank); then one line per prebuilt program,
  `asset/1<TAB>NAME<TAB>DEST<TAB>SHA256` (module/1's asset of the machine it was packed on).
- `store/NAME/ID/...`: each module's store copy as it is (asset in place), ID as for its lock line.
- `index/INDEX.bundle` (with `--with-index`): a git bundle of `refs/henki/verified` of each index in use.

`unpack` lists the members before it extracts anything and refuses an absolute path, a `..`, a name twice, a member
under a link, a link outside a store copy, and anything but files, directories and links. Each copy must hold its
line's hd1 and its asset's SHA-256, and fit this machine (module/1's `os`, `requires`, assets). A bundle counts only
for an index in use here, checked from the anchor and checkpoint here (never a key or anchor in the pack), and only when
its head has the commit verified here on its first-parent line (or nothing is verified here); it never moves
`refs/henki/verified`, and the yanks and removals of the commit verified here apply after its own. A line's fields are
checked, not believed: no control character, COMMIT a full commit or `-`, HD1 an hd1, a `path` SOURCE an absolute
path, each `mask=cmd/CMD` a command the copy has; KIND `official` only when this henki's module of that name has that
hd1, `index:INDEX` only for an index in use here whose commit is verified here (or whose bundle is used) and lists that
hd1 as the line's package and version (the default index's yanks of the same hd1 apply to it too). A malicious yank
refuses the install. A module the lock holds with the same hd1 is left as it is; another one is replaced, its line
going to the history (hist/1, `unpack`).

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
  port this henki binds, and a link to a regular file its owner can run (or that file itself, which the manager never
  writes), that file and its directory owned by the user (or the file by root) and not group- or world-writable.
  Anything else stops the start (`error config`) with the fix `henki mods swap PORT std, or start with --port
  PORT=std`. The directory, the link and the program are trust files, and the program is protected from the model's
  write and edit tools. The binding's name in notices and records is `NAME ID` for a program at
  `.../mods/NAME/ID/port/PORT`, else its path.
- **The process.** Once per session, before the first record, henki runs the program with no arguments in the session's
  working directory, in a session and process group of its own, without a terminal. Its environment is the tool
  environment without the extensions' `bin/` (the allowlist and `pass_env`, `HENKI_DEPTH` one deeper, `HENKI_SESSION`
  empty, no API key). Its stderr goes to henki's kernel log, `STATE/spill/.kernel-PID.log` (0600, removed with the
  spill files after 24 hours). It must exit when its stdin closes, and leave no process behind: at the session's end
  henki sends `bye`, waits 300 ms for the answer, then kills the process group and, on Linux, every process of the
  binding's session. The working directory and the state dir go to it as JSON strings: when either path is not
  UTF-8, which JSON cannot hold, the start stops (`error config`, naming the binding and the path).
- **Lines.** UTF-8 JSON, one object per line, `\n` after it, at most 16 MiB each way. henki writes canonical lines
  (compact, keys sorted, invisible code points escaped). A request is `{"id":N,"method":M,"params":P}`, ids from 0, one
  at a time. An answer is `{"id":N,"result":R}` or `{"error":{"code":C,"message":S},"id":N}`. A line with another id
  (or a second answer to one) is dropped as henki reads it, while it waits and between calls; a line without `id` (or
  with `"id":null`) is a notification, which this version reads and skips. More than 256 MiB of such lines between two
  answers is a broken line (the dead rule).
- **hello**, the first request: params `henki` (its version), `port`, `protocol` (`[1]`), `versions` (the port's
  method set majors henki offers, `[1]`), `scope` (the session's working directory) and `state`
  (`STATE/port/PORT/`, 0700, the binding's own dir). The answer: `{"caps":[...],"impl":V,"name":N,"protocol":1,
  "version":1}`, `name` and `impl` 1 to 64 bytes without spaces or invisible characters, at most 16 `caps`; or error
  `version`. `NAME IMPL` is shown in parentheses in the binding notice.
- **Codes** a binding may send: `invalid`, `unsupported` (a method it does not know), `io`, `not_found`, `busy`,
  `exists`, `tampered`, `corrupt`, `version`; anything else arrives as `other`. henki adds `timeout`, `crashed`,
  `protocol`, `interrupted` (a signal) and `budget` (the session's time ran out).
- **Timeouts.** `hello` 5 s, `create` 2 s, `open` 5 s, `read` 10 s per page, `list` 5 s, `append` 2 s, `discard` 2 s,
  `decide` 5 s, `record` 2 s, `bye` 300 ms. A signal or the session deadline ends a wait for `hello`, `create`, `open`,
  `read`, `list` or `decide` within 50 ms, never one for a method that changes state.
- **The dead rule.** A timeout, a closed stdout, a line over the cap or cut at EOF, a non-object line, or an answer
  with both or neither of `result` and `error`, or 256 MiB of lines no request asked for, kills the binding for the
  rest of the session: every later call fails at once. An error answer is one failed call; the binding lives on.
- **Visibility.** A bound port is a notice at start (`history: NAME ID (port/1, N V); henki --port history=std runs a
  session without it`), and the journal's `session` record names it in `ports` (`{"history":"NAME ID"}`).
- **`henki port PORT [--offer V]... -- PROGRAM`** (not in the usage text): no config read, the same environment, the
  state dir `STATE/port/PORT/`, the working directory as scope. It prints the hello result as `{"result":{"caps":[...],
  "name":"N V"}}` (or the error, then exit 2), then for each request line on stdin, `{"method":M,"params":P}`, one
  canonical answer line, `{"result":R}` or `{"error":{"code":C,"message":S}}`. `{"kill":true}` kills the binding
  without `bye` and ends it. The conformance suites run through it.

## history/1

The history port over port/1: where sessions are kept. Records are journal/1 objects; a binding keeps them as given
(it may add fields named `x-*`). henki sends them as the std binding gets them, before std fills the fields of its own
file: `close` without `bytes`, and a resumed `session` with `resumed` `{}` (journal/1 marks both). A store exported
to std goes through std's `append`, which fills them; a binding's file copied into std's sessions dir is no journal/1
file and is refused. Session ids follow journal/1's rule for any id henki makes, and any id is 1 to 64 of
`A-Z a-z 0-9 . _ -`, not starting with `.` or `-`. A session is held by one binding process at a time, from `create`
or `open` until another `create` or `open`, `bye`, or the process's end (a kill included).

| Method | Params | Result | Errors |
|---|---|---|---|
| `create` | `scope`, `hint` (an id or null) | `{"session":ID}`, held; a `hint` that is not null is the id | `invalid`, `exists` (the hint is taken), `io` |
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
| `decide` | `call` (`args`, `id`, `name`), `cwd`, `root` (the project root), `session`, `target` (a file tool's: `outside`, `path` as resolved, `protected`, `shown`; null for a shell call; a call whose resolved path is not UTF-8 is asked without `decide`) | `{"decision":"allow","reason"?:S}`, `{"decision":"ask","always"?:B}` or `{"decision":"deny","reason":S}` |
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
- **Failure asks.** A binding that does not start or answer `hello` stops the start. In a session a timeout, an exit,
  a broken line or a decision that is none of the three asks, with one notice naming the binding and ending `every
  call is asked`; the binding is then dead for the session and every later call is asked at once. An error answer asks
  that call only, with a notice ending `this call is asked`; the binding lives on and decides the next call. A signal
  or the session deadline ends a `decide` within 50 ms and the turn with it.
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
  `bye`; the next `a` step starts a fresh one; its REQUEST and EXPECT are `-`), or `+V` (a fresh instance offering
  version V).
- REQUEST is `hello` (the instance's first line) or `{"method":M,"params":P}`. `@NAME@` is a saved value; `@SCOPE@` and
  `@SCOPE2@` are two empty directories, `@BIG@` 1 MiB of text.
- EXPECT is the canonical answer line, where `*` is any text and `@(A|B)` is A or B; every other character stands for
  itself. SAVE is `NAME=KEY`: the first string value of `"KEY"` in the answer.
- A step named with a trailing lowercase letter prepares a check and counts only when it fails. After the steps, each
  `henki port` must end within 1 s of its stdin closing, and the program run alone must exit within 1 s of EOF on its
  stdin. henki's own tests run the history file against the std binding through its trait: std speaks no protocol,
  so the hello, version and kill steps are skipped, and the test itself answers `list`, `read`'s `from` (std reads
  whole sessions) and an unknown method (`unsupported`).


## eval/1

Owned by henki-mods; henki never reads it. A module's `eval/` holds tasks that show what it does for a model, and the
general suite (`share/henki/eval/general/` in a release, `eval/general/` in the repository) holds everyday shell tasks,
run to show that a module costs a model nothing on tasks it is not for. Each task is a directory, named as a module is:

```
TASK/prompt.txt          required: the task, 1 to 4 KiB of UTF-8 without a code point henki escapes
TASK/setup               required: a program that makes the task's files in an empty work dir
TASK/check               required: a program; exit 0 is a pass
TASK/fake-bin/CMD        optional: programs first on PATH in the run (a tool the task's machine lacks, or must not run)
TASK/task.toml           optional, module/1's grammar: max_steps (1 to 10000), max_seconds (1 to 9999; default 120)
lib/                     optional: data and programs the tasks share (never a task); a fake-bin entry may be a
                         relative link into it
```

Dot files are ignored anywhere, and so are `README*` and `LICENSE*` next to the tasks; anything else is refused.
`henki-mods eval --check DIR` checks this layout and runs nothing; `add` and `verify --dir` check a module's `eval/`
(its links, as every link in a module, stay inside the module).

- **A run.** Each run of a task has a fresh root dir of its own holding `home/` (the run's `HOME`),
  `xdg/{config,data,state,cache}` (the four `XDG_*_HOME`) and `work/` (the cwd); nothing of the person's own home,
  config, state or data is read or written in it. `setup` runs in `work/` with stdin empty, on a copy of the task and
  of the suite's `lib/` in the root; then `henki run --json --approve auto [--model M] --max-seconds S [--max-steps N]
  -` with prompt.txt on stdin and plain copies of `fake-bin/`'s programs first on PATH, while the root holds no other
  file of the suite; then `check` in `work/`, on a fresh copy of the task and `lib/` (the root as henki left it, but
  no dir the run's commands made), without `fake-bin/` on PATH, with, on stdin, the `text` of henki's `result`
  record (json/1), byte for byte, with no newline added. Its exit status is the result. Setup and check get
  `EVAL_DIR` (the task's copy), `EVAL_LIB` (the copy of `lib/`), `EVAL_HOME` (= `HOME`) and `EVAL_ROOT` (the root);
  henki does not. henki's records of the run (its `--json` stdout) go to the run dir, out of the root, and are copied
  to `EVAL_ROOT/henki.jsonl` for `check`. A setup or a check is stopped after 60 seconds. A setup that fails stops
  the eval, and its run is VOID (evalrun/1).
- **The sandbox.** Each of the three runs inside the official `sandbox` module's program (`port/exec`, run directly,
  not as an exec binding, so henki keeps all four tools), started in the root with the person's own `HOME`, which it
  hides (on macOS only its listed secret paths), as it hides henki's own dirs (the run dir among them). It hides
  nothing else: a suite kept elsewhere on the machine, such as an official module's `eval/` or the general suite in
  henki's share dir, is in the model's reach.
  Where it does not run, `eval` refuses unless `--unsandboxed` is given; then only `HOME` and the XDG dirs are
  scratch dirs.
- **The arms.** The module on and the module off: in each run's home the person's `config.toml` without its `ext` and
  `shadow` keys and their `AGENTS.md` without the mods cards block; in the on arm the module is then added there as a
  person adds one (`henki-mods add DIR --yes`: its store copy, its link, its card, and the `shadow` line it names). No
  allow file and no port link. Before any task each arm answers `Reply with the word ready.`; a status other than
  `ok` stops the eval. In each repeat every task runs once per arm, the arms taking turns going first. 3 repeats; a
  suite other than `general` whose effect after them is positive with 0 inside its 95% CI (evalrun/1) gets 10 more;
  `--repeats N` runs exactly N. `--suite` names one suite or several, comma-separated (`own` is the module's own tasks,
  the default), all in one run.
- **The record.** Per suite and arm one line, printed and appended to `$XDG_STATE_HOME/henki/eval.tsv` (else
  `~/.local/state/henki/eval.tsv`), fields separated by tabs:

```
eval/1 PKG VERSION HD1 MODEL SUITE ARM PASSES RUNS SECONDS
```

  PKG is the module's name, VERSION its manifest's (`-` without one), HD1 its tree hash, MODEL `--model`, else
  `HENKI_MODEL`, else the config's `model` (`-` with none), SUITE the module's name (its own tasks), `general`, another
  module's name or a suite's path, ARM `on` or `off`, PASSES and RUNS counts, SECONDS the sum of the runs'
  `duration_s`. Each run is also an evalrun/1 row.
- **The general suite** uses POSIX tools and git only, no network, and every check is deterministic.
- eval runs a module's `setup` and `check`, and henki with `approve = "auto"`, so it shows what it will run and asks
  first (`--yes` once the screen was read); it refuses to run inside henki (`HENKI_DEPTH` set). `--server URL` (a
  loopback `http://HOST:PORT/PATH`, passed as `HENKI_BASE_URL`) names the model server; `--model-file F` the model's
  file, whose SHA-256 the run records.

## evalrun/1

Owned by henki-mods; henki never reads it. One eval run's record, and the badge a module's index entry carries.

- **The run dir** `$XDG_STATE_HOME/henki/eval/RUN/` (RUN is `YYYYMMDDTHHMMSSZ-XXXX`, UTC and 4 hex digits) holds
  `run.toml` (module/1's grammar: `run`, `package`, `version`, `hd1`, `model`, `server`, `server_model` and
  `server_build` from the server's `GET /props` when it answers one (llama.cpp's), else `-`, `henki` (its `--version`),
  `model_file` and `model_sha256` with `--model-file`, `sandbox` (the program, or `none`), `repeats`, `started`,
  `ended`), `runs.tsv` (one row per run, in run order), `VOID` when a run was VOID, and per run a dir
  `NNNN-ARM-REPEAT-TASK/` with henki's records (`henki.jsonl`), its journal, and what setup and check printed.
- **A row**, fields separated by tabs, none empty, none holding `"`, `\` or a control character:

```
evalrun/1 RUN PKG VERSION HD1 MODEL SUITE ARM REPEAT TASK PASS STATUS STEPS SECONDS PROMPT_BYTES
```

  RUN, PKG, VERSION, HD1 and MODEL as eval/1's (the same in every row of a run); SUITE as eval/1's (one suite besides
  `general` per run); ARM `on` or `off`; REPEAT from 1; TASK the task's name; PASS `1` or `0`; STATUS henki's result
  (`ok`, `gave_up:REASON`, `needs_approval`, `budget`, `interrupted`, `output_closed`, `error:KIND`; json/1), or
  `none` (no result) or `setup` (the setup failed); STEPS and SECONDS the result's `steps` and `duration_s`;
  PROMPT_BYTES what the arm adds to the system prompt (the module's doc line with the extensions header, and its
  card), 0 in the off arm. A run whose STATUS is `none`, `setup` or `error:` `config`, `nested`, `journal`,
  `internal` or `tampered` is VOID: it measured henki or its setup, not the model.
- **The statistics** (`henki-mods eval report RUN`), per suite, with tasks as the unit: d(t) = passes on minus
  passes off over the R repeats of task t; the effect is the sum of d(t) per R×T runs. Its 95% CI is a bootstrap
  over the tasks: 2,000 resamples with replacement, each task drawn as `s % T` (tasks in byte order) with the
  Park-Miller generator `s = s * 16807 % 2147483647` from seed 1 for each suite, the 50th and the 1,951st of the
  sorted sums. The sign test is the exact two-sided binomial over the tasks better and worse.
- **The badge rule** (`henki-mods eval badge RUN`: PASS or FAIL with the reason): no VOID run; rows of the module's
  own suite and of `general`, at least 3 repeats of each; the own suite's CI lower bound above 0 and its sign p at
  most 0.05; the general suite's CI upper bound at least 0. Below 13 repeats, an own suite with a positive effect and
  0 in its CI fails with "rerun with --repeats 13".
- **In an index** (index/1) a version's badge is the `eval_*` keys `henki-mods-dev publish --eval RUN` prints:
  `eval_suite`, `eval_model`, `eval_hd1` (= the version's `hd1`), `eval_repeats`, `eval_on`, `eval_off`,
  `eval_effect` and `eval_general` as the statistics give them, `eval_model_sha256` and `eval_server` from the run,
  and `eval_by` (who ran it, and when); the rows behind it are `evals/NAME/VERSION.tsv`, byte for byte the run's
  `runs.tsv`. `henki-mods-dev eval badge --check ROWS FILE` passes when the rows give a PASS whose keys equal FILE's.
  henki-mods recomputes the badge from those rows the same way; a reviewed version whose badge recomputes is
  `measured` when `eval_by`'s login is its `reviewer` (the reviewer reran it), and otherwise shows it as the author's.

## The module manager's programs and files

Not a contract of its own: where the formats above that henki-mods owns live, which program reads and writes each, and
what any program can rely on when it runs the manager.

- **Two programs**, shipped side by side in `bin/`: `henki-mods`, the person's verbs (and `test --port PORT
  NAME|DIR|PROGRAM`, which `swap` runs, and `eval`), and `henki-mods-dev`, the author's verbs (`new`, `test [DIR]`,
  `try`, `publish`, `share`, `entry`, `eval report|badge`). `henki-mods VERB` and `henki mods VERB` reach both:
  henki-mods runs henki-mods-dev for the author's verbs. henki-mods-dev reads a module with henki-mods's own code (it
  sources it), so an author's `test` checks what an install checks.
- **Messages.** A refusal is one line on stderr, `henki-mods: PROBLEM; fix: WHAT TO DO`, and exit 1; a warning is
  `henki-mods: warning: ...`; a usage error is exit 2. `doctor` exits 1 when it found a problem, and `eval badge`
  exits 0 only on PASS. Screens and texts may change.
- **Questions.** A change shows a screen, then asks on `HENKI_MODS_TTY` (default `/dev/tty`); `--yes` answers yes for a
  screen already read; without a terminal and without `--yes` it refuses. Inside a tool call (`HENKI_SESSION` set and
  no `HENKI_NEXT`, which henki gives only to a command the person typed) `add`, `on`, `update`, `rollback`, `unpack`,
  `init`, `index add` and `swap` refuse whatever `--yes` says: `run it at the prompt with /mods`. A model can change
  its environment, so this only slows it down; the guard is henki asking before each shell call.
- **A change made under henki** writes `restart` into `HENKI_NEXT` (handoff/1); run alone it prints how to restart.
- **The share dir**, SHARE below, is looked for in `HENKI_SHARE` when it is set, then in `share/henki/` beside the
  program's own dir (`bin/../share/henki`, as a release and Homebrew lay it out), then in the repository's layout
  (`mods/`, `manager/`), per kind of file. Every index screen names the anchor's path when it is not the one beside
  the program.
- **`contracts`** in a manifest may name the contracts of this file's index; the manager's list of them is tested
  against the index.

| Path | Holds | Written by | Format |
|---|---|---|---|
| `CONFIG_DIR/mods.lock` | the modules and the bound ports | henki-mods | lock/1, bind/1 |
| `CONFIG_DIR/mods.history` | the lock lines a change replaced or removed | henki-mods | hist/1 |
| `CONFIG_DIR/mods.index`, `CONFIG_DIR/index/NAME.signers`, `NAME.checkpoint` | the indexes added with `index add`, their pinned anchors | henki-mods | index/1 |
| `CONFIG_DIR/ext/NAME` | a link to the module's store copy: the module is on | henki-mods | lock/1 |
| `CONFIG_DIR/port/PORT` | a link to a store copy's `port/PORT`, or to the missing `yanked` | henki-mods (`swap`, a yank) | bind/1, port/1 |
| `CONFIG_DIR/AGENTS.md` | only the block between `# >>> mods cards` and `# <<< mods cards` | henki-mods | module/1 (`.henki/card.md`) |
| `DATA/henki/mods/NAME/ID/` | read-only store copies (a masked copy is `ID.mHASH`) | henki-mods | hd1, lock/1 |
| `DATA/henki/mods.users` | the config dirs that use the store | henki-mods | hist/1 |
| `DATA/henki/index/NAME.git`, `NAME.synced` | an index's bare clone (`refs/henki/verified`) and its last good sync | henki-mods | index/1 |
| `STATE/henki/eval.tsv`, `STATE/henki/eval/RUN/` | eval's lines and runs | henki-mods | eval/1, evalrun/1 |
| `SHARE/official/NAME/` | the official modules | release | module/1, ext/2 |
| `SHARE/index/` | the default index's `NAME.url`, `NAME.signers`, `NAME.checkpoint` and `refused.keys` | release | index/1 |
| `SHARE/mod-profiles/NAME.lock` | the shipped module profiles | release | lock/1 |
| `SHARE/conform/PORT.tsv`, `SHARE/conform/fixtures/` | the conformance suites | release | conform/1 |
| `SHARE/eval/general/` | the general eval suite | release | eval/1 |

CONFIG_DIR is `~/.config/henki` (or `HENKI_CONFIG`, which henki sets for `/mods`), DATA `$XDG_DATA_HOME` (else
`~/.local/share`), STATE `$XDG_STATE_HOME` (else `~/.local/state`). henki itself reads only the links in `ext/` and
`port/`, and `AGENTS.md`; every other file here is the manager's.
