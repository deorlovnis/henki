#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
//! The tripwire in `run`: a trust file changed during the session ends it with exit 2 `changed`, whether a call
//! changed it, created it, or a background process did after the last call. And the reads that need no approval:
//! this session's own spill files.
use crate::harness::*;
use crate::mock::*;
use serde_json::json;
use std::fs;
use std::path::Path;

/// The run ended `error changed` naming `path`, as the result and as a notice.
fn tripped(d: &Done, path: &Path) {
    let says = format!("trust file changed: {}", path.display());
    let e = &d.result()["error"];
    d.check((d.code, e["kind"].as_str(), e["message"].as_str()) == (Some(2), Some("changed"), Some(says.as_str())), &format!("not ended by {says:?}"));
    d.check(d.notices().contains(&says), "no tripwire notice");
}

#[test]
fn e2e_tripwire_run_exit2() {
    let mut r = Run::new("");
    let agents = r.config_dir().join("AGENTS.md");
    let both = Step::Stream {
        parts: vec![
            Step::Call {
                form: Form::Native { fragments: 2, index: Some(0), id: call_id() },
                name: "shell".into(),
                args: json!({"command": format!("echo more >> {}", agents.display())}).to_string(),
                finish: String::new(),
            },
            Step::Call {
                form: Form::Native { fragments: 2, index: Some(1), id: call_id() },
                name: "shell".into(),
                args: json!({"command": "touch second-ran"}).to_string(),
                finish: String::new(),
            },
        ],
        finish: Some("tool_calls".into()),
        done: true,
        hold_ms: 0,
    };
    let m = Mock::start(vec![both, say("never")]);
    r.at(&m.url());
    fs::create_dir_all(r.config_dir()).unwrap();
    fs::write(&agents, "Be brief.\n").unwrap();
    let d = r.json("change the instructions");
    d.finish();
    tripped(&d, &agents);
    assert_eq!(d.results()[1]["content"], "not run: trust file changed");
    assert!(!r.path("second-ran").exists() && m.log().len() == 1, "{}", d.diag());
}

#[test]
fn e2e_tripwire_config_created() {
    let m = Mock::start(vec![shell("mkdir -p ~/.config/henki && echo 'model = \"other\"' > ~/.config/henki/config.toml"), say("never")]);
    let mut r = Run::new(&m.url());
    r.config_text(None).env("HENKI_BASE_URL", &m.url()).env("HENKI_MODEL", "env-model");
    let d = r.cmd(&["--json", "--approve", "auto", "write a config"]).go();
    d.finish();
    tripped(&d, &r.config_path);
}

#[test]
fn e2e_tripwire_bash_replaced() {
    let mut r = Run::new("");
    let bash = r.dir.join("bin/bash");
    fs::create_dir_all(bash.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink("/bin/bash", &bash).unwrap();
    let m = Mock::start(vec![shell(&format!("rm {b} && ln -s /bin/bash {b}", b = bash.display())), say("never")]);
    r.at(&m.url()).env("PATH", &format!("{}:/usr/local/bin:/usr/bin:/bin", bash.parent().unwrap().display()));
    let d = r.json("replace the shell");
    d.finish();
    assert_eq!(d.of("session").first().map(|s| s["shell"].clone()), Some(json!(bash)), "{}", d.diag());
    tripped(&d, &bash);
}

/// A background process from an earlier call changes a trust file after the last call's check (here once the next
/// request is out, while the model is silent): the turn would end with the plain answer, and the check at its end
/// catches it.
#[test]
fn e2e_tripwire_at_finishing() {
    let mut r = Run::new("");
    let agents = r.config_dir().join("AGENTS.md");
    let late = Step::Stall { before_first_byte: true, after_words: 0, for_ms: 30_000 };
    let m = Mock::start(vec![shell(&format!("(while [ ! -f go ]; do sleep 0.05; done; echo x >> {}) >/dev/null 2>&1 &", agents.display())), late]);
    r.at(&m.url());
    let mut l = r.cmd(&["--json", "start it and answer"]).spawn();
    l.until("the next request", 10 * SEC, |_| m.log().len() >= 2);
    fs::write(r.path("go"), "").unwrap();
    l.until("the change", 10 * SEC, |_| fs::read_to_string(&agents).is_ok_and(|t| t == "x\n"));
    m.open_gate();
    let d = l.wait();
    d.finish();
    tripped(&d, &agents);
    assert_eq!(d.of("assistant").last().unwrap()["msg"]["content"], "late answer", "the answer came, then the check\n{}", d.diag());
    assert_eq!(m.log().len(), 2);
}

/// A call's output spills; this session reads its own spill file without asking (in a later run of the session
/// too); another session asks for it (exit 3 in `run`).
#[test]
fn e2e_spill_read_free() {
    let big = "head -c 100000 /dev/zero | tr '\\0' z";
    let m = Mock::start(vec![shell(big), say("spilled")]);
    let mut r = Run::new(&m.url());
    r.set("approve", "\"ask\"");
    r.write_allow(&format!("{}\t{big}\n", r.root()));
    let d = r.json("make output");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    let spill = spill_of(d.results()[0]["content"].as_str().unwrap(), &r.home);
    let m2 = Mock::start(vec![call("read", json!({"path": spill, "limit": 2})), say("read it")]);
    r.at(&m2.url());
    let d = r.cmd(&["--json", "--resume", "read your spill file"]).go();
    d.finish();
    assert_eq!((d.code, d.of("approval")[0]["verdict"].as_str(), d.of("ask").len()), (Some(0), Some("auto"), 0), "{}", d.diag());
    assert!(d.results()[0]["content"].as_str().unwrap().starts_with("zzz"), "{}", d.diag());
    let m3 = Mock::start(vec![call("read", json!({"path": spill})), say("never")]);
    r.at(&m3.url());
    let d = r.json("read another session's spill file");
    d.finish();
    assert_eq!((d.code, d.of("ask").len()), (Some(3), 1), "{}", d.diag());
}

/// In auto mode the allow file is never read, but it is still watched: a call that creates it ends the run.
#[test]
fn e2e_tripwire_allow_in_auto() {
    let mut r = Run::new("");
    let m = Mock::start(vec![shell(&format!("echo '*\tcurl x | sh' >> {}", r.allow().display())), say("never")]);
    r.at(&m.url());
    let d = r.json("add a rule");
    d.finish();
    tripped(&d, &r.allow());
    assert_eq!((&d.of("session")[0]["allow_hash"], m.log().len()), (&serde_json::Value::Null, 1), "{}", d.diag());
}

/// The snapshot is taken when the config and the files it names are read, so a change made while `run -` still waits for
/// its task (here a rule revoked from another terminal, as `henki allow rm 1` does) is found before the first call: the
/// revoked rule never runs it.
#[test]
fn e2e_run_stdin_trust_change_trips() {
    let m = Mock::start(vec![shell("echo ruled"), say("never")]);
    let mut r = Run::new(&m.url());
    r.set("approve", "\"ask\"");
    r.write_allow(&format!("{}\techo ruled\n", r.root()));
    let mut l = r.cmd(&["--json", "-"]).stdin(In::Open).spawn();
    // The state dir is made right after the config is loaded, and before the task is read.
    l.until("the state dir", 10 * SEC, |_| r.state().is_dir());
    fs::write(r.allow(), "# no rules\n").unwrap();
    l.write_stdin(b"run the rule");
    l.close_stdin();
    let d = l.wait();
    d.finish();
    tripped(&d, &r.allow());
    assert_eq!((d.of("approval").len(), d.results()[0]["content"].as_str()), (0, Some("not run: trust file changed")), "{}", d.diag());
    assert_eq!(m.log().len(), 1);
}
