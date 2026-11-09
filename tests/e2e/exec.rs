#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
//! exec/1: a program linked at `CONFIG/port/exec` runs every shell call as `PROGRAM BASH --noprofile --norc -c CMD`,
//! and henki treats it as the call: its timeout, the kill, and the sweep at the session end. The toys in
//! tests/fixtures/exec/ are the programs: one that runs its arguments as they are (and logs them), and on Linux real
//! sandboxes (bubblewrap when it runs here, else user and pid namespaces).
use crate::harness::*;
use crate::mock::*;
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const TOYS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/exec");

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

/// `CONFIG/port/exec` linked to a wrapper script `RUN/bindings/exec` (in a 0700 dir) whose body is `body`.
fn bind(r: &Run, body: &str) -> PathBuf {
    let w = r.dir.join("bindings/exec");
    fs::create_dir_all(w.parent().unwrap()).unwrap();
    fs::set_permissions(w.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(&w, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&w, fs::Permissions::from_mode(0o700)).unwrap();
    let port = r.config_dir().join("port");
    fs::create_dir_all(&port).unwrap();
    fs::set_permissions(r.config_dir(), fs::Permissions::from_mode(0o700)).unwrap();
    let _ = fs::remove_file(port.join("exec"));
    symlink(&w, port.join("exec")).unwrap();
    w
}

/// toy-exec as the exec binding, logging to `RUN/toy.log`; returns the log's path.
fn toy(r: &Run) -> PathBuf {
    let log = r.dir.join("toy.log");
    bind(r, &format!("HENKI_TOY_LOG={} exec sh {TOYS}/toy-exec \"$@\"", log.display()));
    log
}

/// The argv of each call toy-exec ran, in order.
fn logged(log: &Path) -> Vec<Vec<String>> {
    let text = fs::read_to_string(log).unwrap_or_default();
    text.lines().map(|l| l.split('\x1f').map(String::from).collect::<Vec<_>>()).map(|mut a| (a.pop(), a).1).collect()
}

/// The bash henki runs: the first on the run's user PATH.
fn host_bash() -> String {
    ["/usr/local/bin/bash", "/usr/bin/bash", "/bin/bash"].into_iter().find(|b| Path::new(b).is_file()).unwrap().to_string()
}

/// The tools a request offers, by name.
fn offered(body: &Value) -> Vec<String> {
    body["tools"].as_array().into_iter().flatten().map(|t| t["function"]["name"].as_str().unwrap().to_string()).collect()
}

/// Each shell call runs as `PROGRAM BASH --noprofile --norc -c CMD` in the session's cwd with the tool
/// environment, and its output and status are the call's; the start notice and `session.ports` name the binding.
#[test]
fn e2e_exec_runs_every_call() {
    let m = Mock::start(vec![shell("echo one; pwd"), shell("printf '%s' \"$HENKI_SESSION\"; exit 3"), say("done")]);
    let r = Run::new(&m.url());
    let (log, w) = (toy(&r), r.dir.join("bindings/exec").display().to_string());
    let d = r.json("run two");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    let bash = host_bash();
    let want = |c: &str| vec![bash.clone(), "--noprofile".into(), "--norc".into(), "-c".into(), c.into()];
    assert_eq!(logged(&log), [want("echo one; pwd"), want("printf '%s' \"$HENKI_SESSION\"; exit 3")]);
    assert!(content(&d, 0).starts_with(&format!("one\n{}\n[exit 0", fs::canonicalize(&r.cwd).unwrap().display())), "{}", content(&d, 0));
    assert!(content(&d, 1).starts_with(&format!("{}\n[exit 3", d.id())) && d.results()[1]["is_error"] == true, "{}", d.results()[1]);
    let notice = format!("exec: {w} (exec/1) runs every shell call (read, write and edit are off); henki --port exec=std runs a session without it");
    d.check(d.notices().contains(&notice), "no exec notice");
    d.check(d.of("session")[0]["ports"] == json!({"exec": w}), "no session.ports.exec");
}

/// While a program is bound only `shell` is offered: the request's tools, the `session` record's and the system prompt
/// say so (part 1 without its line on reading before editing), and a `read` call is an unknown tool that never runs.
#[test]
fn e2e_exec_shell_only() {
    let m = Mock::start(vec![call("read", json!({"path": "notes.txt"})), say("done"), say("ok")]);
    let mut r = Run::new(&m.url());
    r.set("tools", "[\"shell\", \"read\", \"edit\"]");
    fs::write(r.path("notes.txt"), "secret\n").unwrap();
    let log = toy(&r);
    let d = r.json("read it");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    let body: Value = serde_json::from_str(m.log()[0].text()).unwrap();
    assert_eq!(offered(&body), ["shell"]);
    assert_eq!(d.of("session")[0]["tools"], json!(["shell"]));
    let system = m.log()[0].system();
    assert!(system.starts_with("You are henki") && !system.contains("Read a file") && !system.contains("edit"), "{system}");
    assert_eq!((content(&d, 0).as_str(), d.results()[0]["is_error"].as_bool()), ("unknown tool read; available: shell", Some(true)), "{}", d.diag());
    assert!(logged(&log).is_empty());
    // Unbound, the same config offers its three tools and part 1 is whole.
    let d = r.cmd(&["--json", "--port", "exec=std", "x"]).go();
    d.finish();
    assert_eq!(offered(&serde_json::from_str(m.log()[2].text()).unwrap()), ["shell", "read", "edit"]);
    assert!(m.log()[2].system().contains("- Read a file before you edit it."));
}

/// A config whose `tools` has no `shell` cannot start with a program bound; the fix names both ways out.
#[test]
fn e2e_exec_needs_shell() {
    let m = Mock::start(vec![say("never")]);
    let mut r = Run::new(&m.url());
    r.set("tools", "[\"read\"]");
    let w = bind(&r, "exec \"$@\"");
    let d = r.json("x");
    d.finish();
    let e = &d.result()["error"];
    assert_eq!((d.code, e["kind"].as_str()), (Some(2), Some("config")), "{}", d.diag());
    let want = format!("exec: {} runs shell calls, and tools has no shell; fix: add \"shell\" to tools, or start with --port exec=std", w.display());
    assert_eq!(e["message"], want.as_str());
    assert!(m.log().is_empty());
}

/// A call past its timeout is killed with the program, and nothing of it is left.
#[test]
fn e2e_exec_timeout_kills_program() {
    let (r, m) = scripted(|r| {
        let tree = format!("(sleep {m} & exec sleep {m}) & sleep {m} & exec sleep {m}", m = r.marker);
        vec![call("shell", json!({"command": tree, "timeout": 1})), Step::Stall { before_first_byte: true, after_words: 0, for_ms: 1500 }]
    });
    let log = toy(&r);
    let mut l = r.cmd(&["--json", "build a tree"]).spawn();
    l.until("the next request", 6 * SEC, |_| m.log().len() >= 2);
    let left = marked(&r.marker);
    assert!(left.is_empty(), "the timeout left processes of the call: {left:?}");
    let d = l.wait();
    d.finish();
    assert!(content(&d, 0).ends_with("[timed out after 1 s; process group killed]"), "{}", content(&d, 0));
    assert_eq!(logged(&log).len(), 1);
}

/// A call's background process outlives the call and is stopped at the session end.
#[test]
fn e2e_exec_background_swept() {
    let (r, _m) = scripted(|r| vec![shell(&format!("sleep {} & echo started", r.marker)), say("done")]);
    toy(&r);
    let d = r.json("leave something running");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    assert!(content(&d, 0).starts_with("started\n"), "{}", content(&d, 0));
    d.check(d.notices().iter().any(|n| n.starts_with("stopped ") && n.ends_with(" background processes at exit")), "no at-exit stop");
}

/// A link that names nothing runnable stops the start with its fix; nothing is asked of the model.
#[test]
fn e2e_exec_dangling_refused() {
    let m = Mock::start(vec![say("never")]);
    let r = Run::new(&m.url());
    let w = bind(&r, "exec \"$@\"");
    for broken in [true, false] {
        if broken {
            fs::remove_file(&w).unwrap();
        } else {
            fs::write(&w, "#!/bin/sh\n").unwrap();
            fs::set_permissions(&w, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let d = r.json("x");
        d.finish();
        let e = &d.result()["error"];
        assert_eq!((d.code, e["kind"].as_str()), (Some(2), Some("config")), "{}", d.diag());
        assert!(e["message"].as_str().unwrap().ends_with("not a link to a program; fix: henki mods swap exec std, or start with --port exec=std"), "{e}");
    }
    assert!(m.log().is_empty());
}

/// `--port exec=std` runs bash itself, without the program, and the session names no binding.
#[test]
fn e2e_exec_port_std() {
    let m = Mock::start(vec![shell("echo direct"), say("done")]);
    let r = Run::new(&m.url());
    let log = toy(&r);
    let d = r.cmd(&["--json", "--port", "exec=std", "x"]).go();
    d.finish();
    assert!(d.code == Some(0) && content(&d, 0).starts_with("direct\n[exit 0"), "{}", d.diag());
    assert!(logged(&log).is_empty() && d.of("session")[0].get("ports").is_none(), "{}", d.diag());
    assert_eq!(offered(&serde_json::from_str(m.log()[0].text()).unwrap()), ["shell", "read", "write", "edit"]);
}

/// A module's `bin/bash` comes first on the tool PATH, yet the program gets the stamped bash as argument 1, so the
/// shadow never takes a call.
#[test]
fn e2e_exec_module_bash_not_used() {
    let m = Mock::start(vec![shell("echo real"), say("done")]);
    let mut r = Run::new(&m.url());
    let x = r.config_dir().join("ext/wrap");
    fs::create_dir_all(x.join("bin")).unwrap();
    fs::write(x.join("doc.md"), "wraps bash\n").unwrap();
    fs::write(x.join("bin/bash"), "#!/bin/sh\necho shadow\n").unwrap();
    fs::set_permissions(x.join("bin/bash"), fs::Permissions::from_mode(0o755)).unwrap();
    r.set("shadow", "[\"wrap/bash\"]");
    let log = toy(&r);
    let d = r.json("x");
    d.finish();
    assert!(d.code == Some(0) && content(&d, 0).starts_with("real\n[exit 0"), "{}", d.diag());
    assert_eq!(logged(&log)[0][0], host_bash());
}

/// Linux: a real sandbox as the program, bubblewrap when it runs here, else user and pid namespaces; when neither
/// runs, the test fails. A call runs inside it, and the timeout's kill and the session end's sweep leave nothing behind.
#[test]
#[cfg(target_os = "linux")]
fn e2e_exec_sandbox() {
    let works =
        |body: &str| Command::new("sh").args(["-c", body, "_", "true"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
    // HENKI_TEST_BWRAP names a bubblewrap to use, else the first on PATH; the binding gets it by its absolute path.
    let path = std::env::var_os("PATH").unwrap_or_default();
    let found = std::env::split_paths(&path).map(|d| d.join("bwrap")).find(|b| b.is_file());
    let bwrap = std::env::var_os("HENKI_TEST_BWRAP").filter(|b| !b.is_empty()).map(PathBuf::from).or(found);
    let candidates = bwrap.iter().map(|b| format!("HENKI_TOY_BWRAP={} exec sh {TOYS}/toy-exec-bwrap \"$@\"", b.display())).chain([format!("exec sh {TOYS}/toy-exec-ns \"$@\"")]);
    let candidates: Vec<String> = candidates.collect();
    let body = candidates.iter().find(|b| works(b)).unwrap_or_else(|| panic!("neither bubblewrap nor unprivileged user namespaces run here: no sandbox to hold exec/1 to")).clone();
    let (r, m) = scripted(|r| {
        let tree = format!("(sleep {m} & exec sleep {m}) & sleep {m} & exec sleep {m}", m = r.marker);
        let bg = format!("sleep {} & echo $$", r.marker);
        vec![shell(&bg), call("shell", json!({"command": tree, "timeout": 1})), Step::Stall { before_first_byte: true, after_words: 0, for_ms: 1500 }]
    });
    bind(&r, &body);
    let mut l = r.cmd(&["--json", "sandboxed"]).spawn();
    l.until("the third request", 8 * SEC, |_| m.log().len() >= 3);
    let d = l.wait();
    d.finish();
    // pid 1 or 2 in a pid namespace of its own.
    assert!(["1\n", "2\n"].iter().any(|p| content(&d, 0).starts_with(p)), "{body}: {}", content(&d, 0));
    assert!(content(&d, 1).ends_with("[timed out after 1 s; process group killed]"), "{}", content(&d, 1));
}

/// Linux: the official `sandbox` module's program as the binding, unchanged from mods/. A call cannot read the home
/// (inside, a scratch dir with no file) or the agent socket, its writes reach the project and not the home, and its
/// background process ends with the call. When no backend runs here the test fails, as above.
#[test]
#[cfg(target_os = "linux")]
fn e2e_exec_official_sandbox() {
    let prog = concat!(env!("CARGO_MANIFEST_DIR"), "/mods/sandbox/port/exec");
    let (mut r, _m) = scripted(|r| {
        vec![
            shell("cat ~/.ssh/k; printf '[%s] %s\\n' \"${SSH_AUTH_SOCK-unset}\" \"$(find ~ -mindepth 1 ! -type d | wc -l)\""),
            shell(&format!("echo x >made-here && echo x >\"$HOME/out\"; sleep {} & echo started", r.marker)),
            say("done"),
        ]
    });
    let ok = Command::new(prog).args(["/bin/sh", "-c", "true"]).current_dir(&r.cwd).stdin(Stdio::null()).stderr(Stdio::piped()).output().unwrap();
    assert!(ok.status.success(), "the sandbox runs nothing here: {}", String::from_utf8_lossy(&ok.stderr));
    fs::create_dir_all(r.home.join(".ssh")).unwrap();
    fs::write(r.home.join(".ssh/k"), "secret-key\n").unwrap();
    r.env("SSH_AUTH_SOCK", &r.dir.join("agent.sock").display().to_string());
    let port = r.config_dir().join("port");
    fs::create_dir_all(&port).unwrap();
    fs::set_permissions(r.config_dir(), fs::Permissions::from_mode(0o700)).unwrap();
    symlink(prog, port.join("exec")).unwrap();
    let d = r.json("look around");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    assert_eq!((&d.of("session")[0]["ports"]["exec"], &d.of("session")[0]["tools"]), (&json!(prog), &json!(["shell"])), "{}", d.diag());
    let (seen, wrote) = (content(&d, 0), content(&d, 1));
    assert!(!seen.contains("secret-key") && seen.contains("No such file") && seen.contains("[unset] 0\n"), "{seen}");
    assert!(wrote.starts_with("started\n") && r.cwd.join("made-here").is_file() && !r.home.join("out").exists(), "{wrote}");
}

/// Linux: a PATH dir inside the project (an activated `.venv/bin`, say) never lends the sandbox its backend. Call 1
/// plants `.venv/bin/bwrap` (the project is writable inside); call 2 still runs sandboxed, and the planted program
/// never runs on the host.
#[test]
#[cfg(target_os = "linux")]
fn e2e_exec_sandbox_planted_backend() {
    let prog = concat!(env!("CARGO_MANIFEST_DIR"), "/mods/sandbox/port/exec");
    let (mut r, _m) = scripted(|r| {
        let plant = format!("printf '#!/bin/sh\\necho ran >%s/escaped\\nexit 0\\n' {} >.venv/bin/bwrap", r.dir.display());
        vec![shell(&format!("mkdir -p .venv/bin && {plant} && chmod +x .venv/bin/bwrap && echo planted")), shell("echo second; find ~ -mindepth 1 ! -type d | wc -l"), say("done")]
    });
    r.env("PATH", &format!("{}/.venv/bin:/usr/local/bin:/usr/bin:/bin", r.cwd.display()));
    let port = r.config_dir().join("port");
    fs::create_dir_all(&port).unwrap();
    fs::set_permissions(r.config_dir(), fs::Permissions::from_mode(0o700)).unwrap();
    symlink(prog, port.join("exec")).unwrap();
    let d = r.json("plant");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    assert!(content(&d, 0).starts_with("planted\n") && r.cwd.join(".venv/bin/bwrap").is_file(), "{}", content(&d, 0));
    assert!(!r.dir.join("escaped").exists(), "the planted bwrap ran on the host");
    assert!(content(&d, 1).starts_with("second\n0\n"), "call 2 did not run sandboxed: {}", content(&d, 1));
}
