#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
//! journal/1 from the outside: golden journals written by hand (tests/fixtures/contracts/) put where henki looks, then
//! resumed. They pin the file layout and the session id, replay (unknown kinds and fields skipped, `elide` applied),
//! the `close` rule, the torn tail, repair, call ids, and the refusals.
use crate::harness::*;
use crate::mock::*;
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// A closed session of every record kind, two unknown kinds and an unknown field among them.
const GOLDEN: &str = "20261107T120000Z-c0de";
/// A session that died while a call ran, its last line torn.
const DANGLING: &str = "20261107T120500Z-dead";
const REPAIR: &str = "interrupted; the command may have partly run";

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(format!("{}/tests/fixtures/contracts/{name}", env!("CARGO_MANIFEST_DIR")))
}

/// FNV-1a 64 as 16 lowercase hex digits: the scope of a working directory is this of its realpath.
fn fnv(b: &[u8]) -> String {
    format!("{:016x}", b.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, &c| (h ^ u64::from(c)).wrapping_mul(0x0100_0000_01b3)))
}

/// `sessions/SCOPE/` of the run's working directory (or of `scope` when given), made as henki makes it.
fn scope_dir(r: &Run, scope: Option<&str>) -> PathBuf {
    let s = scope.map_or_else(|| fnv(fs::canonicalize(&r.cwd).unwrap().as_os_str().as_encoded_bytes()), String::from);
    let dir = r.state().join("sessions").join(s);
    fs::create_dir_all(&dir).unwrap();
    for d in [r.state(), r.state().join("sessions"), dir.clone()] {
        fs::set_permissions(d, fs::Permissions::from_mode(0o700)).unwrap();
    }
    dir
}

/// `bytes` as session `id` in the scope, mode 0600.
fn put(r: &Run, scope: Option<&str>, id: &str, bytes: &[u8]) -> PathBuf {
    let p = scope_dir(r, scope).join(format!("{id}.jsonl"));
    fs::write(&p, bytes).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
    p
}

fn golden() -> Vec<u8> {
    fs::read(fixture_path(&format!("{GOLDEN}.jsonl"))).unwrap()
}

/// A run whose resume fails before a session: the result's error (kind, message), the journal left as it was.
fn refused(r: &Run, args: &[&str], path: &Path) -> (String, String) {
    let before = fs::read(path).unwrap();
    let d = r.cmd(args).go();
    d.finish();
    assert_eq!((d.code, d.journal.as_ref()), (Some(2), None), "{}", d.diag());
    assert_eq!(fs::read(path).unwrap(), before, "a refused resume changed the journal");
    let e = &d.result()["error"];
    (e["kind"].as_str().unwrap().into(), e["message"].as_str().unwrap().into())
}

/// The golden journal resumed: the request renders exactly its history (the fixture beside it), unknown kinds and
/// fields skipped and the elided result elided; the file grows by appending only; a clean close gives no notice; the
/// next call without an id is `h3`.
#[test]
fn contracts_journal_golden_replay() {
    let no_id = Step::Call {
        form: Form::Native { fragments: 1, index: Some(0), id: String::new() },
        name: "shell".into(),
        args: json!({"command": "echo h3"}).to_string(),
        finish: "tool_calls".into(),
    };
    let m = Mock::start(vec![no_id, say("resumed")]);
    let r = Run::new(&m.url());
    let gold = golden();
    let path = put(&r, None, GOLDEN, &gold);
    let d = r.cmd(&["--json", "--resume", "and now"]).go();
    d.finish();
    assert_eq!((d.code, d.journal.as_ref()), (Some(0), Some(&path)), "{}", d.diag());
    let want: Vec<Value> = serde_json::from_str(&fs::read_to_string(fixture_path(&format!("{GOLDEN}.request.json"))).unwrap()).unwrap();
    let sent = m.log()[0].messages();
    assert_eq!(sent[0]["role"], "system");
    assert_eq!(sent[1..], want[..], "the request is not the golden history:\n{}", serde_json::to_string_pretty(&sent).unwrap());
    assert!(d.journal_bytes.starts_with(&gold), "the journal was not appended to");
    let s = &d.of("session")[0];
    assert_eq!((&s["v"], &s["id"], &s["resumed"]), (&json!(1), &json!(GOLDEN), &json!({"from_bytes": gold.len()})));
    assert_eq!(d.result()["session"], GOLDEN, "the session id is the file stem");
    let notices = d.notices();
    assert!(!notices.iter().any(|n| n.contains("did not close cleanly")), "{notices:?}");
    assert!(notices.iter().any(|n| n.starts_with("cwd changed since the session started: ")), "{notices:?}");
    assert_eq!(d.of("assistant")[0]["msg"]["tool_calls"][0]["id"], "h3", "the next free hN id");
    // The second request replays the new segment the same way, after the golden history.
    assert_eq!(m.log()[1].messages()[1..want.len() + 1], want[..]);
}

/// Call ids: a model's own id is kept, an empty one renamed `hN`; N counts on from the history the session started
/// with, and an `hN` the model sends during the session does not move it.
#[test]
fn contracts_journal_call_ids() {
    let with_id = |id: &str| Step::Call {
        form: Form::Native { fragments: 1, index: Some(0), id: id.into() },
        name: "shell".into(),
        args: json!({"command": "echo x"}).to_string(),
        finish: "tool_calls".into(),
    };
    let m = Mock::start(vec![with_id("h9"), with_id(""), say("done")]);
    let d = Run::new(&m.url()).json("ids");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    let ids: Vec<Value> = d.of("assistant").iter().filter_map(|a| a["msg"]["tool_calls"].get(0)).map(|c| c["id"].clone()).collect();
    assert_eq!(ids, [json!("h9"), json!("h1")], "h1, not h10: the model's h9 does not move N");
}

/// `--resume=ID` finds a session in any scope; `--resume` alone looks only in this working directory's scope, where
/// an empty file is no session.
#[test]
fn contracts_journal_scope_and_id() {
    let m = Mock::start(vec![say("found"), say("unused")]);
    let r = Run::new(&m.url());
    let elsewhere = put(&r, Some("0000000000000000"), GOLDEN, &golden());
    let empty = put(&r, None, "20261107T130000Z-0000", b"");
    let (kind, message) = refused(&r, &["--json", "--resume", "x"], &empty);
    assert_eq!((kind.as_str(), message.starts_with("no session to resume in ")), ("journal", true), "{message}");
    let d = r.cmd(&["--json", &format!("--resume={GOLDEN}"), "x"]).go();
    d.finish();
    assert_eq!((d.code, d.journal.as_ref()), (Some(0), Some(&elsewhere)), "{}", d.diag());
}

/// A session another henki holds (its flock) is in use.
#[test]
fn contracts_journal_lock() {
    let r = Run::new("http://127.0.0.1:9/v1");
    let path = put(&r, None, GOLDEN, &golden());
    let held = fs::File::open(&path).unwrap();
    held.lock().unwrap();
    let (kind, message) = refused(&r, &["--json", "--resume", "x"], &path);
    assert_eq!((kind.as_str(), message.as_str()), ("journal", format!("session {GOLDEN} is in use").as_str()));
}

/// A torn last line is cut off; a run that did not close is resumed with a notice; the call that never got a result
/// gets the repair result, journaled (and on stdout) before the new `session` record.
#[test]
fn contracts_journal_torn_and_repair() {
    let m = Mock::start(vec![say("went on")]);
    let r = Run::new(&m.url());
    let torn = fs::read(fixture_path(&format!("{DANGLING}.jsonl"))).unwrap();
    let keep = torn.iter().rposition(|&b| b == b'\n').unwrap() + 1;
    let path = put(&r, None, DANGLING, &torn);
    let d = r.cmd(&["--json", "--resume", "go on"]).go();
    d.finish();
    assert_eq!((d.code, d.journal.as_ref()), (Some(0), Some(&path)), "{}", d.diag());
    assert!(d.journal_bytes.starts_with(&torn[..keep]) && !d.journal_bytes.starts_with(&torn), "the torn line was not cut off");
    let after = lines(&String::from_utf8_lossy(&d.journal_bytes[keep..]));
    let repair = json!({"t": "tool_result", "id": "call_z", "name": "shell", "content": REPAIR, "is_error": true, "ms": 0, "bytes": 0, "ts": after[0]["ts"]});
    assert_eq!((&after[0], &after[1]["t"]), (&repair, &json!("session")), "{}", d.diag());
    assert_eq!(after[1]["resumed"]["from_bytes"], keep, "from_bytes is the length after the cut");
    assert_eq!(d.records()[..2], after[..2], "stdout shows the repair before the session");
    let notices = d.notices();
    let dropped = format!("dropped an incomplete last line ({} bytes) from {}", torn.len() - keep, path.display());
    assert!(notices.contains(&dropped) && notices.contains(&"previous run did not close cleanly".to_string()), "{notices:?}");
    let tool = m.log()[0].messages().into_iter().find(|x| x["role"] == "tool").unwrap();
    assert_eq!((&tool["content"], &tool["tool_call_id"]), (&json!(REPAIR), &json!("call_z")));
}

/// What a resume refuses: a `close` that is not the last record or does not give its own offset, a line that is not
/// a JSON object, a file without `session`, another dialect.
#[test]
fn contracts_journal_refusals() {
    let r = Run::new("http://127.0.0.1:9/v1");
    let gold = String::from_utf8(golden()).unwrap();
    let close = gold.lines().last().unwrap();
    let bytes = (gold.len() - close.len() - 1).to_string();
    let cases: Vec<(&str, String, &str, String)> = vec![
        (
            "close offset",
            gold.replace(&format!("\"bytes\":{bytes},"), &format!("\"bytes\":{},", bytes.parse::<u64>().unwrap() - 1)),
            "tampered",
            "journal modified outside henki: ".into(),
        ),
        ("after close", format!("{gold}{{\"t\":\"notice\",\"text\":\"late\",\"ts\":\"2026-11-07T12:00:04Z\"}}\n"), "tampered", "journal modified outside henki: ".into()),
        ("corrupt line", gold.replacen(gold.lines().nth(2).unwrap(), "[\"not an object\"]", 1), "journal", "corrupt journal ".into()),
        ("no session", gold.lines().nth(1).unwrap().to_string() + "\n", "journal", "".into()),
    ];
    for (what, text, kind, starts) in cases {
        let path = put(&r, None, GOLDEN, text.as_bytes());
        let (k, m) = refused(&r, &["--json", "--resume", "x"], &path);
        assert_eq!(k, kind, "{what}: {m}");
        assert!(m.starts_with(&starts), "{what}: {m}");
        match what {
            "corrupt line" => assert!(m.ends_with(&format!("{}:3", path.display())), "{m}"),
            "no session" => assert!(m.ends_with("no session record"), "{m}"),
            _ => {}
        }
    }
    let mut r = r;
    r.set("dialect", "\"anthropic\"");
    let path = put(&r, None, GOLDEN, gold.as_bytes());
    let (k, m) = refused(&r, &["--json", "--resume", "x"], &path);
    assert_eq!((k.as_str(), m), ("journal", format!("{}: the session's dialect is openai, not anthropic", path.display())));
}
