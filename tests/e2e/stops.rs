#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
//! The rules that stop a turn: three identical failing calls in a row, the step cap, and an empty answer.
use crate::harness::*;
use crate::mock::*;

#[test]
fn e2e_stuck() {
    let m = Mock::start(vec![shell("ls missing-dir"), shell("ls missing-dir"), shell("ls missing-dir"), say("never")]);
    let r = Run::new(&m.url());
    let d = r.json("list it");
    d.finish();
    assert_eq!((d.code, d.result()["reason"].as_str()), (Some(1), Some("stuck")), "{}", d.diag());
    assert_eq!((m.log().len(), d.results().len()), (3, 3), "stuck after the third call, before another request");
    assert!(d.results().iter().all(|x| x["is_error"] == true));
}

#[test]
fn e2e_stuck_reset() {
    let script = ["false", "false", "true", "false", "false", "false"];
    let mut steps: Vec<Step> = script.iter().map(|c| shell(c)).collect();
    steps.push(say("never"));
    let m = Mock::start(steps);
    let r = Run::new(&m.url());
    let d = r.json("fail and recover");
    d.finish();
    assert_eq!((d.code, d.result()["reason"].as_str()), (Some(1), Some("stuck")), "{}", d.diag());
    assert_eq!((m.log().len(), d.results().len()), (6, 6), "a success resets the run: 6 calls");
}

#[test]
fn e2e_same_ok_not_stuck() {
    let m = Mock::start(vec![shell("cat status"), shell("cat status"), shell("cat status"), shell("cat status"), say("it is running")]);
    let r = Run::new(&m.url());
    std::fs::write(r.path("status"), "booting\n").unwrap();
    let d = r.json("poll the status");
    d.finish();
    assert_eq!((d.code, d.result()["text"].as_str()), (Some(0), Some("it is running")), "identical successes never count\n{}", d.diag());
    assert_eq!(d.results().len(), 4);
}

#[test]
fn e2e_max_steps() {
    let m = Mock::start((1..=5).map(|i| shell(&format!("echo {i}"))).collect());
    let mut r = Run::new(&m.url());
    r.set("max_steps", "3");
    let d = r.json("count");
    d.finish();
    assert_eq!((d.code, d.result()["reason"].as_str(), d.result()["steps"].as_u64()), (Some(1), Some("max_steps"), Some(3)), "{}", d.diag());
    assert_eq!((m.log().len(), d.results().len()), (3, 3), "every call of the last step has its result");
}

#[test]
fn e2e_empty_answer() {
    let tail: String = (0..200).map(|i| format!("r{i} ")).collect();
    let m = Mock::start(vec![reason(&tail)]);
    let r = Run::new(&m.url());
    let d = r.cmd(&["answer"]).go();
    d.finish();
    assert_eq!((d.code, d.out()), (Some(1), String::new()), "never ok with an empty answer\n{}", d.diag());
    let n = d.notices();
    let notice = n.iter().find(|x| x.starts_with("the model ended without an answer; its reasoning ended: ")).unwrap_or_else(|| panic!("{n:?}"));
    let shown = notice.strip_prefix("the model ended without an answer; its reasoning ended: ").unwrap();
    assert!(shown.len() <= 500 && tail.trim_end().ends_with(shown.trim()) && shown.contains("r199"), "the last 500 bytes of reasoning: {shown}");
    assert!(d.err().lines().any(|l| l == "(gave up: empty_answer)"), "{}", d.err());
    let m = Mock::start(vec![say("   ")]);
    let d = Run::new(&m.url()).json("answer blank");
    d.finish();
    assert_eq!((d.code, d.result()["reason"].as_str()), (Some(1), Some("empty_answer")), "a blank answer is empty\n{}", d.diag());
}
