#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
//! Allow rules in `run`: an exact command for this project root runs without asking, anything else still asks (exit
//! 3); the file is read only in ask mode, fails closed when it is unsafe or invalid, cannot be written by the model,
//! is watched, and its rules for this root are announced at every start.
use crate::harness::*;
use crate::mock::*;
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;

/// A run in ask mode at `m`.
fn asking(m: &Mock) -> Run {
    let mut r = Run::new(&m.url());
    r.set("approve", "\"ask\"");
    r
}

fn verdicts(d: &Done) -> Vec<Value> {
    d.of("approval").iter().map(|a| json!([a["verdict"], a["rule"]])).collect()
}

#[test]
fn e2e_allow_rule_runs() {
    let m = Mock::start(vec![shell("echo ruled"), say("done")]);
    let r = asking(&m);
    r.write_allow(&format!("# mine\n/elsewhere\tls\n{}\techo ruled\n", r.root()));
    let d = r.cmd(&["use the rule"]).go();
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    let line = format!("$ echo ruled  (allowed by rule 2 for {}, ~/.config/henki/allow)", r.root());
    assert!(d.err().lines().any(|l| l == line), "no rule line {line:?}:\n{}", d.err());
    assert_eq!((verdicts(&d), d.of("ask").len()), (vec![json!(["rule", 2])], 0), "{}", d.diag());
    assert!(d.results()[0]["content"].as_str().unwrap().starts_with("ruled\n"));
}

#[test]
fn e2e_allow_exact_only() {
    for near in ["git status; rm -rf ~", "git status && x", "git status ", "git status --short"] {
        let m = Mock::start(vec![shell(near), say("never")]);
        let r = asking(&m);
        r.write_allow(&format!("{}\tgit status\n", r.root()));
        let d = r.json("check");
        d.finish();
        assert_eq!((d.code, d.of("ask").len(), d.of("approval").len()), (Some(3), 1, 0), "{near:?} matched\n{}", d.diag());
    }
}

#[test]
fn e2e_allow_other_root_asks() {
    let m = Mock::start(vec![shell("echo other"), say("never")]);
    let r = asking(&m);
    r.write_allow("/somewhere/else\techo other\n");
    let d = r.json("check");
    d.finish();
    assert_eq!((d.code, d.status().as_str()), (Some(3), "needs_approval"), "{}", d.diag());
}

#[test]
fn e2e_allow_not_read_in_auto() {
    let m = Mock::start(vec![shell("echo auto"), say("done")]);
    let r = Run::new(&m.url());
    r.write_allow("not a rule\n");
    std::fs::set_permissions(r.allow(), std::fs::Permissions::from_mode(0o666)).unwrap();
    let d = r.json("go");
    d.finish();
    assert_eq!((d.code, &d.of("session")[0]["allow_hash"]), (Some(0), &Value::Null), "{}", d.diag());
    assert_eq!(verdicts(&d), [json!(["auto", null])]);
}

#[test]
fn e2e_allow_fail_closed() {
    let m = Mock::start(vec![]);
    let r = asking(&m);
    let cases = [
        (
            format!("{}\techo ok\n# a note\nnot a rule\n", r.root()),
            0o600,
            format!("{}:3: no TAB between ROOT and COMMAND; fix: henki allow rm 2, or edit the file", r.allow().display()),
        ),
        (format!("{}\techo ok\n", r.root()), 0o666, format!("chmod 600 {}", r.allow().display())),
    ];
    for (text, mode, says) in cases {
        r.write_allow(&text);
        std::fs::set_permissions(r.allow(), std::fs::Permissions::from_mode(mode)).unwrap();
        let d = r.json("go");
        d.finish();
        let e = &d.result()["error"];
        assert_eq!((d.code, e["kind"].as_str()), (Some(2), Some("config")), "{}", d.diag());
        assert!(e["message"].as_str().unwrap().contains(&says), "{says:?} not in {e}");
    }
    assert!(m.log().is_empty() && r.journals().is_empty());
}

#[test]
fn e2e_allow_protected_write() {
    let m = Mock::start(vec![call("write", json!({"path": "~/.config/henki/allow", "content": "*\trm -rf ~\n"})), say("refused")]);
    let r = Run::new(&m.url());
    r.write_allow("");
    let d = r.json("add a rule");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    assert_eq!(d.results()[0]["content"], "protected: ~/.config/henki/allow is henki's own file; change it through shell so the user sees it");
    assert_eq!(std::fs::read_to_string(r.allow()).unwrap(), "");
}

/// A command that adds a rule from inside a session (here one an allow rule ran) trips the tripwire at once.
#[test]
fn e2e_allow_outside_edit_trips() {
    let add = "printf '*\\tls\\n' >> ~/.config/henki/allow";
    let m = Mock::start(vec![shell(add), say("never")]);
    let r = asking(&m);
    r.write_allow(&format!("{}\t{add}\n", r.root()));
    let d = r.json("add a rule");
    d.finish();
    let e = &d.result()["error"];
    let says = format!("trust file changed: {}", r.allow().display());
    assert_eq!((d.code, e["kind"].as_str(), e["message"].as_str()), (Some(2), Some("changed"), Some(says.as_str())), "{}", d.diag());
    assert!(d.notices().contains(&says), "{:?}", d.notices());
    assert_eq!(m.log().len(), 1, "no request after the change");
}

#[test]
fn e2e_allow_rules_notice() {
    let m = Mock::start(vec![say("hi"), say("hi"), say("hi"), say("hi")]);
    let mut r = asking(&m);
    r.write_allow(&format!("{}\techo noticed\n", r.root()));
    let notice = "allow: 1 rules apply to this root (~/.config/henki/allow; /allow lists them)";
    let d = r.cmd(&["hello"]).go();
    d.finish();
    assert!(d.err().lines().any(|l| l == format!("henki: {notice}")), "{}", d.err());
    assert!(d.notices().contains(&notice.to_string()), "journaled: {:?}", d.notices());
    let d = r.json("hello");
    d.finish();
    assert!(d.records().iter().any(|x| x["t"] == "notice" && x["text"] == notice), "{}", d.diag());
    r.write_allow("/somewhere/else\techo noticed\n");
    let d = r.json("hello");
    d.finish();
    assert!(d.notices().iter().all(|n| !n.starts_with("allow: ")), "rules for other roots only: {:?}", d.notices());
    r.write_allow(&format!("{}\techo noticed\n", r.root()));
    r.set("approve", "\"auto\"");
    let d = r.json("hello");
    d.finish();
    assert!(d.notices().iter().all(|n| !n.starts_with("allow: ")), "auto mode reads no rules: {:?}", d.notices());
}
