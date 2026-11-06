#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
//! Tool processes: background work, timeouts, kills and sweeps, the child's environment and signals, the spill, and
//! the release gates on exit time and memory.
use crate::harness::*;
use crate::mock::*;
use serde_json::json;
use std::time::{Duration, Instant};

fn content(d: &Done, i: usize) -> String {
    d.results().get(i).and_then(|r| r["content"].as_str()).unwrap_or_else(|| panic!("no tool result {i}\n{}", d.diag())).to_string()
}

/// A run whose script needs the run's marker: `steps(&run)` builds it.
fn scripted(steps: impl FnOnce(&Run) -> Vec<Step>) -> (Run, Mock) {
    let mut r = Run::new("");
    let m = Mock::start(steps(&r));
    r.at(&m.url());
    (r, m)
}

#[test]
fn e2e_shell_bg_returns() {
    let (r, _m) = scripted(|r| vec![shell(&format!("sleep {} & echo started", r.marker)), say("left it running")]);
    let d = r.json("start a background job");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    assert!(content(&d, 0).starts_with("started\n"), "{}", content(&d, 0));
    assert!(d.results()[0]["ms"].as_u64().unwrap() <= 1000, "the call waited for its background job: {}", d.results()[0]);
}

#[test]
fn e2e_timeout_kills_call_tree() {
    let (r, m) = scripted(|r| {
        let tree = format!("(sleep {m} & exec sleep {m}) & sleep {m} & exec sleep {m}", m = r.marker);
        vec![call("shell", json!({"command": tree, "timeout": 1})), Step::Stall { before_first_byte: true, after_words: 0, for_ms: 1500 }]
    });
    let mut l = r.cmd(&["--json", "build a tree"]).spawn();
    l.until("the next request", 6 * SEC, |_| m.log().len() >= 2);
    let left = marked(&r.marker);
    assert!(left.is_empty(), "the timeout left processes of the call: {left:?}");
    let d = l.wait();
    d.finish();
    let c = content(&d, 0);
    assert!(["[timed out after 1 s; process group killed]", "(no output)\n[timed out after 1 s; process group killed]"].contains(&c.as_str()), "{c}");
    let ms = d.results()[0]["ms"].as_u64().unwrap();
    assert!((1000..2500).contains(&ms), "{ms} ms");
}

#[test]
fn e2e_timeout_late_tooldone() {
    let (r, _m) = scripted(|r| vec![call("shell", json!({"command": format!("echo before; trap '' TERM; {}", r.sleep()), "timeout": 1})), say("timed out")]);
    let d = r.json("ignore TERM");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    let c = content(&d, 0);
    assert!(c.starts_with("before\n") && c.ends_with("[timed out after 1 s; process group killed]"), "{c}");
    assert_eq!(d.results().len(), 1, "one result per call");
    let ms = d.results()[0]["ms"].as_u64().unwrap();
    assert!((1200..2800).contains(&ms), "TERM, the grace, then KILL: {ms} ms");
}

#[test]
fn e2e_cancel_setsid_grandchild() {
    let (r, _m) = scripted(|r| vec![shell(&format!("setsid sleep {m} & (setsid sleep {m} &); exec sleep {m}", m = r.marker)), say("never")]);
    let mut l = r.cmd(&["--json", "escape the group"]).spawn();
    l.ready();
    l.until("three marked processes", 5 * SEC, |_| marked(&r.marker).len() >= 3);
    l.signal(libc::SIGINT);
    let d = l.wait();
    d.finish();
    assert_eq!(d.code, Some(130), "{}", d.diag());
}

#[test]
fn e2e_run_exit_kills_background() {
    let (r, _m) = scripted(|r| vec![shell(&format!("sleep {m} & setsid sleep {m} & echo started", m = r.marker)), say("done")]);
    let d = r.json("leave something running");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    let n = d.notices();
    assert!(n.iter().any(|x| x.starts_with("stopped ") && x.ends_with(" background processes at exit")), "{n:?}");
}

#[test]
fn e2e_reap_notice() {
    let (r, _m) = scripted(|r| {
        let m = &r.marker;
        vec![shell(&format!("sleep {m} & echo one")), shell("echo quiet"), shell(&format!("sleep {m} & echo two")), say("done")]
    });
    let d = r.json("start two");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    let bg: Vec<String> = d.notices().into_iter().filter(|x| x.ends_with(" background processes from earlier calls still run")).collect();
    assert_eq!(bg, ["1 background processes from earlier calls still run", "2 background processes from earlier calls still run"], "once per change");
    let kinds = d.kinds();
    let first = kinds.iter().position(|k| k == "notice").unwrap();
    assert_eq!(kinds[first - 1], "tool_result", "the notice follows the call that changed the count");
}

#[test]
fn e2e_kill9_bash_dies() {
    let (r, _m) = scripted(|r| vec![shell(&format!("sleep {}; echo after", r.marker)), say("never")]);
    let mut l = r.cmd(&["--json", "wait"]).spawn();
    l.until("bash and its sleep", 5 * SEC, |_| marked(&r.marker).len() >= 2);
    let pid = l.pid;
    let bash = marked(&r.marker).into_iter().map(|p| p.0).find(|&p| ppid(p) == Some(pid)).expect("bash is henki's child");
    l.kill9();
    let gone = poll(SEC, || state(bash).is_none_or(|s| s == 'Z'));
    assert!(gone, "bash outlived henki by 1 s");
    let d = l.wait();
    kill_marked(&r.marker);
    d.finish();
}

#[test]
fn e2e_child_signals_default() {
    let (r, _m) = scripted(|r| {
        let term = format!("sleep {} & pid=$!; kill -TERM $pid; wait $pid; echo status $?", r.marker);
        vec![shell("grep -E '^Sig(Blk|Ign)' /proc/self/status"), call("shell", json!({"command": term, "timeout": 5})), say("checked")]
    });
    let d = r.json("check signals");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    assert!(content(&d, 0).starts_with("SigBlk:\t0000000000000000\nSigIgn:\t0000000000000000\n"), "{}", content(&d, 0));
    assert!(content(&d, 1).starts_with("status 143\n"), "TERM stops a tool child: {}", content(&d, 1));
}

#[test]
fn e2e_shell_tty_enxio() {
    let m = Mock::start(vec![shell("exec 3</dev/tty && echo opened"), say("no tty")]);
    let r = Run::new(&m.url());
    let d = r.json("open the terminal");
    d.finish();
    let c = content(&d, 0);
    assert!(c.contains("No such device or address") && !c.contains("opened"), "{c}");
    assert!(d.results()[0]["ms"].as_u64().unwrap() < 1000 && d.results()[0]["is_error"] == true);
}

#[test]
fn e2e_bash_env_ignored() {
    let m = Mock::start(vec![shell("echo plain; echo \"[$BASH_ENV][$ENV]\""), say("ok")]);
    let mut r = Run::new(&m.url());
    let script = r.dir.join("startup.sh");
    std::fs::write(&script, format!("touch {}/sourced\n", r.dir.display())).unwrap();
    r.env("BASH_ENV", script.to_str().unwrap()).env("ENV", script.to_str().unwrap());
    let d = r.json("run something");
    d.finish();
    assert!(content(&d, 0).starts_with("plain\n[][]\n"), "{}", content(&d, 0));
    assert!(!r.dir.join("sourced").exists(), "a startup file was sourced");
}

#[test]
fn e2e_env_allowlist() {
    let m = Mock::start(vec![shell("tr '\\0' '\\n' < /proc/$$/environ | sort"), say("seen")]);
    let mut r = Run::new(&m.url());
    r.set("api_key", "\"sk-local-secret-77\"").set("pass_env", "[\"FOO_SETTING\"]");
    r.env("PATH", "/usr/local/bin:relative/bin:/usr/bin:/bin").env("LANG", "C.UTF-8").env("TZ", "UTC").env("NO_COLOR", "1");
    r.env("http_proxy", "http://user:pw@proxy.invalid:3128").env("HTTPS_PROXY", "http://proxy.invalid:3128");
    r.env("FOO_SETTING", "bar").env("SECRET_THING", "x").env("EDITOR", "vi").env("BASH_ENV", "/nonexistent");
    let d = r.json("show the environment");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    let c = content(&d, 0);
    let vars: Vec<&str> = c.lines().take_while(|l| !l.starts_with('[')).collect();
    let user_path = "/usr/local/bin:/usr/bin:/bin";
    let home = format!("HOME={}", r.home.display());
    let session = format!("HENKI_SESSION={}", d.id());
    let mut want = vec!["FOO_SETTING=bar", "HENKI_DEPTH=1", home.as_str(), "HTTPS_PROXY=http://proxy.invalid:3128", "LANG=C.UTF-8", "NO_COLOR=1", "TZ=UTC"];
    let (p, up, v) = (format!("PATH={user_path}"), format!("HENKI_USER_PATH={user_path}"), concat!("HENKI_VERSION=", env!("CARGO_PKG_VERSION")).to_string());
    let xdg: Vec<String> = ["CACHE", "CONFIG", "DATA", "STATE"].iter().map(|k| format!("XDG_{k}_HOME={}", r.dir.join("xdg").join(k.to_lowercase()).display())).collect();
    want.extend([p.as_str(), up.as_str(), v.as_str(), session.as_str()]);
    want.extend(xdg.iter().map(String::as_str));
    want.sort();
    assert_eq!(vars, want, "the child environment is exactly the allowlist");
    assert!(!c.contains("sk-local-secret-77"), "the key reached a child");
}

#[test]
fn e2e_dumpable_environ_eacces() {
    let m = Mock::start(vec![shell("cat /proc/$PPID/environ; echo \"rc=$?\""), say("ok")]);
    let r = Run::new(&m.url());
    let d = r.json("read henki's environment");
    d.finish();
    let c = content(&d, 0);
    assert!(c.contains("Permission denied") && c.contains("rc=1"), "{c}");
}

#[test]
fn e2e_nested_refused() {
    let m = Mock::start(vec![say("nested ok")]);
    let mut r = Run::new(&m.url());
    r.env("HENKI_DEPTH", "1");
    let d = r.json("hello");
    d.finish();
    assert_eq!((d.code, d.result()["error"]["kind"].as_str()), (Some(2), Some("nested")), "{}", d.diag());
    assert_eq!(d.result()["error"]["message"], "refusing to start inside henki (HENKI_DEPTH=1); pass --nested if this is intended");
    assert!(d.result()["session"].is_null() && m.log().is_empty());
    let d = r.cmd(&["hello"]).go();
    d.finish();
    assert_eq!(d.code, Some(2));
    assert_eq!(d.err(), "henki: refusing to start inside henki (HENKI_DEPTH=1); pass --nested if this is intended\n");
    let d = r.cmd(&["--json", "--nested", "hello"]).go();
    d.finish();
    assert_eq!((d.code, m.log().len()), (Some(0), 1), "{}", d.diag());
}

#[test]
fn spill_le_8mib() {
    let m = Mock::start(vec![call("shell", json!({"command": "yes", "timeout": 2})), say("enough")]);
    let r = Run::new(&m.url());
    let d = r.json("say yes forever");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    let c = content(&d, 0);
    assert!(c.ends_with("[timed out after 2 s; process group killed]"), "{}", &c[c.len().saturating_sub(200)..]);
    let spill = std::fs::read_dir(r.state().join("spill")).unwrap().flatten().map(|e| e.path()).next().expect("a spill file");
    let size = std::fs::metadata(&spill).unwrap().len();
    assert!(size <= (8 << 20) + 100, "spill {size} bytes");
    assert_eq!(std::fs::metadata(&spill).map(|m| std::os::unix::fs::PermissionsExt::mode(&m.permissions()) & 0o777).unwrap(), 0o600);
}

#[test]
#[ignore = "release gate"]
fn e2e_exit_fast_when_idle() {
    let mut extra = Vec::new();
    for _ in 0..5 {
        let m = Mock::start(vec![shell("echo done"), say("finished")]);
        let r = Run::new(&m.url());
        let d = r.json("one call, then answer");
        d.finish();
        assert!(poll(SEC, || m.log()[1].done.is_some()));
        extra.push(d.exited.saturating_duration_since(m.log()[1].done.unwrap()));
    }
    extra.sort();
    println!("from the last byte to exit: {extra:?}");
    assert!(extra[2] <= Duration::from_millis(20), "median {:?}", extra[2]);
}

#[test]
#[ignore = "release gate"]
fn rss_headless_le_8mb() {
    let steps = vec![
        shell("ls -la; echo done"),
        call("write", json!({"path": "notes.txt", "content": "alpha\nbeta\n"})),
        call("read", json!({"path": "notes.txt"})),
        call("edit", json!({"path": "notes.txt", "old_text": "beta", "new_text": "gamma"})),
        shell("head -c 200000 /dev/zero | tr '\\0' r"),
        say("all done"),
    ];
    let m = Mock::start(steps);
    let r = Run::new(&m.url());
    let d = r.json("use every tool");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    println!("peak RSS {} KiB", d.maxrss_kb);
    assert!(d.maxrss_kb <= 8 * 1024, "peak RSS {} KiB", d.maxrss_kb);
}

#[test]
#[ignore = "release gate"]
fn rss_read_1gb_le_16mb() {
    let m = Mock::start(vec![call("read", json!({"path": "big.txt", "limit": 5})), say("read it")]);
    let r = Run::new(&m.url());
    let f = std::fs::File::create(r.path("big.txt")).unwrap();
    std::io::Write::write_all(&mut &f, "a line of text\n".repeat(1000).as_bytes()).unwrap();
    f.set_len(1 << 30).unwrap();
    let t = Instant::now();
    let d = r.json("read the big file");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    let c = content(&d, 0);
    assert!(c.starts_with("a line of text\n") && c.ends_with("[lines 1-5 shown; file is 1,073,741,824 bytes; continue with offset=6]"), "{c}");
    println!("peak RSS {} KiB in {:?}", d.maxrss_kb, t.elapsed());
    assert!(d.maxrss_kb <= 16 * 1024, "peak RSS {} KiB", d.maxrss_kb);
}
