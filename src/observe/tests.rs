#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
use super::*;
use std::os::unix::fs::PermissionsExt;
use std::sync::mpsc::{self, Receiver};
use std::time::Instant;

/// A module `m` under a fresh temp dir with the observers `on` (file name, sh body), and the state dir's spill/.
fn setup(m: &str, on: &[(&str, &str)]) -> (PathBuf, Ext) {
    let d = std::env::temp_dir().join(format!("henki-observe-{}-{m}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(d.join(m).join("on")).unwrap();
    fs::create_dir_all(d.join("state/spill")).unwrap();
    for (n, body) in on {
        let p = d.join(m).join("on").join(n);
        fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let x = Ext {
        name: m.into(),
        line: "x".into(),
        dir: d.join(m),
        doc: d.join(m).join("doc.md"),
        bin: None,
        cmds: Vec::new(),
        user: Vec::new(),
        on: on.iter().map(|o| o.0.to_string()).collect(),
        prompt: false,
        shadows: Vec::new(),
        paths: Vec::new(),
        targets: Vec::new(),
    };
    (d, x)
}

fn observers(d: &Path, x: &Ext) -> (Observers, Receiver<Ev>) {
    let (tx, rx) = mpsc::channel();
    let env = vec![("PATH".into(), "/usr/bin:/bin".into())];
    (Observers::new(std::slice::from_ref(x), env, d, &d.join("state"), &d.join("state/spill/.kernel-test.log"), tx), rx)
}

fn until(what: &str, f: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn observe_parse_names() {
    for (n, want) in [
        ("turn_end", Some(("turn_end", false))),
        ("turn_start.wait", Some(("turn_start", true))),
        ("call_start.wait", Some(("call_start", true))),
        ("record", Some(("record", false))),
        ("record.wait", None),
        ("question.wait", None),
        ("turn_end.sh", None),
        ("", None),
        (".wait", None),
    ] {
        assert_eq!(parse(n), want, "{n}");
    }
}

#[test]
fn observe_events_of_records() {
    let of_t = |t: &str| of(&json!({"t": t})).collect::<Vec<_>>();
    assert_eq!(of_t("user"), ["turn_start", "record"]);
    assert_eq!(of_t("tool_result"), ["call_end", "record"]);
    assert_eq!(of_t("ask"), ["question", "record"]);
    assert_eq!(of_t("turn_end"), ["turn_end", "record"]);
    assert_eq!(of_t("assistant"), ["record"]);
    assert_eq!(of_t("close"), ["record"], "close is the session end's, never a turn's");
}

/// A run gets the event line as a file on stdin and HENKI_EVENT, HENKI_MODULE and HENKI_STATE; a wait run reports its
/// tag once reaped, a plain one never; the event file is gone; output goes to the kernel log, opened only now.
#[test]
fn observe_emit_runs_once_per_event() {
    let body = "{ cat; printf '%s|%s|%s\\n' \"$HENKI_EVENT\" \"$HENKI_MODULE\" \"$HENKI_STATE\"; } >\"$HENKI_STATE/$HENKI_EVENT.out\"; echo logged";
    let (d, x) = setup("kit", &[("call_start.wait", body), ("turn_end", body)]);
    let (mut o, rx) = observers(&d, &x);
    let data = json!({"call": {"name": "shell"}});
    assert_eq!(o.emit("call_start", "s1", &data, 7, || true), (1, Vec::new()));
    assert!(matches!(rx.recv_timeout(Duration::from_secs(5)), Ok(Ev::Observed { tag: 7 })));
    until("the run to be forgotten", || o.running().is_empty());
    let got = fs::read_to_string(d.join("state/call_start.out")).unwrap();
    let line = r#"{"data":{"call":{"name":"shell"}},"event":"call_start","seq":1,"session":"s1","v":1}"#;
    assert_eq!(got, format!("{line}\ncall_start|{}|{}\n", x.dir.display(), d.join("state").display()));
    assert_eq!(o.emit("question", "s1", &data, 8, || true), (0, Vec::new()), "no observer: nothing runs");
    assert_eq!(o.emit("turn_end", "s1", &json!({}), 9, || true), (0, Vec::new()));
    until("the turn_end run", || d.join("state/turn_end.out").exists() && o.running().is_empty());
    assert!(fs::read_to_string(d.join("state/turn_end.out")).unwrap().contains(r#""seq":3"#), "seq counts every event");
    assert!(matches!(rx.recv_timeout(Duration::from_secs(5)), Ok(Ev::Observed { tag: 0 })), "a plain run reports tag 0");
    assert_eq!(fs::read_to_string(d.join("state/spill/.kernel-test.log")).unwrap(), "logged\nlogged\n");
    let left: Vec<_> = fs::read_dir(d.join("state/spill")).unwrap().flatten().map(|e| e.file_name()).collect();
    assert_eq!(left, [std::ffi::OsString::from(".kernel-test.log")], "an event file was left");
    let _ = fs::remove_dir_all(&d);
}

/// A changed trust file: no observer runs, the event still counts; `intact` is asked only once an observer would run.
#[test]
fn observe_not_intact_runs_nothing() {
    let (d, x) = setup("gate", &[("turn_end", "touch \"$HENKI_STATE/ran\"")]);
    let (mut o, _rx) = observers(&d, &x);
    assert_eq!(o.emit("question", "s", &json!({}), 1, || panic!("asked with no observer")), (0, Vec::new()));
    assert_eq!(o.emit("turn_end", "s", &json!({}), 2, || false), (0, Vec::new()));
    assert!(o.running().is_empty() && !d.join("state/ran").exists(), "a run started");
    o.emit("turn_end", "s", &json!({}), 3, || true);
    until("the intact run", || d.join("state/ran").exists() && o.running().is_empty());
    let _ = fs::remove_dir_all(&d);
}

/// Four runs still going: the observer misses events, one notice; a wait that runs out stops the waiting, one notice;
/// a run that does not start is one notice; `kernel_kill` ends the runs and their waiters reap them.
#[test]
fn observe_slow_runs() {
    let (d, x) = setup("slow", &[("call_start.wait", "exec sleep 30"), ("record", "exec sleep 30")]);
    fs::write(d.join("slow/on/turn_end"), "#!/nonexistent\n").unwrap();
    fs::set_permissions(d.join("slow/on/turn_end"), fs::Permissions::from_mode(0o755)).unwrap();
    let x = Ext { on: vec!["call_start.wait".into(), "record".into(), "turn_end".into()], ..x };
    let (mut o, _rx) = observers(&d, &x);
    for _ in 0..4 {
        assert_eq!(o.emit("record", "s", &json!({}), 1, || true), (0, Vec::new()));
    }
    let slow = "observer slow/on/record is slow: 4 runs still going; it misses events until one ends".to_string();
    assert_eq!(o.emit("record", "s", &json!({}), 1, || true), (0, vec![slow]));
    assert_eq!(o.emit("record", "s", &json!({}), 1, || true), (0, Vec::new()), "one notice");
    assert_eq!(o.emit("call_start", "s", &json!({}), 2, || true).0, 1);
    assert_eq!(o.unwait("call_start"), ["observer slow/on/call_start.wait took over 2 s; it is no longer waited for"]);
    assert_eq!(o.emit("call_start", "s", &json!({}), 3, || true).0, 0, "not waited for again");
    let (n, notes) = o.emit("turn_end", "s", &json!({}), 4, || true);
    assert!(n == 0 && notes.len() == 1 && notes[0].starts_with("observer slow/on/turn_end did not start: "), "{notes:?}");
    assert_eq!(o.emit("turn_end", "s", &json!({}), 5, || true), (0, Vec::new()), "one notice");
    assert_eq!(o.running().len(), 6);
    supervise::kernel_kill(&o.running());
    until("the killed runs to be reaped", || o.running().is_empty());
    let _ = fs::remove_dir_all(&d);
}

/// MAIN's cost per observed event: the event file, the spawn, the waiter thread (Linux CI: p95 under 5 ms).
#[test]
#[ignore = "release gate"]
fn observe_spawn_cost() {
    let _timed = crate::types::lock(&crate::types::TIMED);
    let (d, x) = setup("cost", &[("record", "exit 0")]);
    let (mut o, _rx) = observers(&d, &x);
    let rec = json!({"t": "tool_result", "id": "h1", "name": "shell", "content": "x".repeat(2_000)});
    let mut ms: Vec<f64> = (0..300)
        .map(|_| {
            until("a free slot", || o.running().len() < LIVE);
            let t = Instant::now();
            o.emit("record", "s", &rec, 1, || true);
            t.elapsed().as_secs_f64() * 1e3
        })
        .collect();
    ms.sort_by(f64::total_cmp);
    let (p50, p95) = (ms[ms.len() / 2], ms[ms.len() * 95 / 100]);
    eprintln!("observer spawn ms p50/p95/max: {p50:.3}/{p95:.3}/{:.3}", ms[ms.len() - 1]);
    until("the runs to end", || o.running().is_empty());
    let _ = fs::remove_dir_all(&d);
    assert!(!cfg!(target_os = "linux") || p95 < 5.0, "spawn p95 {p95:.3} ms");
}

/// observe/1's event table and `.wait` rule, its 2 s and 4 runs, and ext/2's 16 observer files, as the code has them.
#[test]
fn contracts_observe_events() {
    let all = include_str!("../../CONTRACTS.md");
    let sec = &all[all.find("\n## observe/1\n").unwrap()..];
    let sec = &sec[..sec[1..].find("\n## ").unwrap()];
    let mut rows: Vec<&str> = sec.lines().filter_map(|l| l.strip_prefix("| `")?.split('`').next()).collect();
    let mut events = EVENTS.to_vec();
    rows.sort();
    events.sort();
    assert_eq!(rows, events);
    assert!(sec.contains("NAME/on/EVENT.wait       turn_start and call_start only"));
    for e in EVENTS {
        assert_eq!(parse(e), Some((e, false)));
        assert_eq!(parse(&format!("{e}.wait")).is_some(), ["turn_start", "call_start"].contains(&e), "{e}.wait");
    }
    let flat = sec.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(flat.contains("for at most 2 s") && WAIT == Duration::from_secs(2));
    assert!(flat.contains("An observer with 4 runs still going misses events") && LIVE == 4);
    assert!(all.contains("At most 16 `on/` files over all modules") && MAX == 16);
}
