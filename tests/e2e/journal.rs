#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
//! The journal across processes: resume after a crash at any point, resume rules, spill files, a fresh home, and how
//! the task reaches henki.
use crate::harness::*;
use crate::mock::*;
use serde_json::{Value, json};
use std::io::Seek;
use std::time::Duration;

const REPAIR: &str = "interrupted; the command may have partly run";

fn scripted(steps: impl FnOnce(&Run) -> Vec<Step>) -> (Run, Mock) {
    let mut r = Run::new("");
    let m = Mock::start(steps(&r));
    r.at(&m.url());
    (r, m)
}

#[test]
fn e2e_resume_kill9() {
    let (mut r, m) = scripted(|r| vec![shell("echo one"), shell(&r.exec_sleep()), say("never")]);
    let mut l = r.cmd(&["--json", "two calls"]).spawn();
    l.wait_marked(5 * SEC);
    let sleeping = r.exec_sleep();
    l.wait_journal(SEC, |x| x["t"] == "assistant" && x["msg"]["tool_calls"][0]["args"]["command"] == sleeping.as_str());
    l.kill9();
    let d = l.wait();
    d.finish();
    let path = d.journal.clone().expect("a journal");
    let len = std::fs::metadata(&path).unwrap().len();
    let dangling = d.of("assistant").last().unwrap()["msg"]["tool_calls"][0]["id"].clone();
    assert_eq!(m.log().len(), 2);
    let m2 = Mock::start(vec![say("resumed once")]);
    r.at(&m2.url());
    let d = r.cmd(&["--json", "--resume"]).go();
    d.finish();
    assert_eq!((d.code, d.journal.as_ref()), (Some(0), Some(&path)), "{}", d.diag());
    let all = d.journal_all();
    let repair = all.iter().position(|x| x["t"] == "tool_result" && x["id"] == dangling).expect("a repair for the dangling call");
    assert_eq!((all[repair]["content"].as_str(), &all[repair]["is_error"]), (Some(REPAIR), &json!(true)));
    let session = all.iter().rposition(|x| x["t"] == "session").unwrap();
    assert!(repair < session, "the repair comes before anything else");
    assert_eq!(all[session]["resumed"]["from_bytes"].as_u64(), Some(len));
    assert!(all.iter().any(|x| x["t"] == "notice" && x["text"] == "previous run did not close cleanly"), "{}", d.diag());
    let tool = m2.log()[0].messages().into_iter().rfind(|x| x["role"] == "tool").unwrap();
    assert_eq!((tool["content"].as_str(), &tool["tool_call_id"]), (Some(REPAIR), &dangling));
    // `--json` stdout shows the repair as journaled, before the new `session` record.
    let out = d.records();
    let shown = out.iter().position(|x| x["t"] == "tool_result").expect("the repair on stdout");
    assert_eq!((&out[shown], out[shown + 1]["t"].as_str()), (&all[repair], Some("session")), "{}", d.diag());
    let m3 = Mock::start(vec![say("resumed twice")]);
    r.at(&m3.url());
    let d = r.cmd(&["--json", "--resume", "and once more"]).go();
    d.finish();
    assert_eq!((d.code, d.journal.as_ref()), (Some(0), Some(&path)), "{}", d.diag());
    assert_eq!(d.journal_all().iter().filter(|x| x["t"] == "session").count(), 3);
    assert!(m3.log()[0].text().starts_with(messages_prefix(m2.log()[0].text())));
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Point {
    Request(usize),
    Call(usize),
}

#[test]
fn e2e_kill9_every_point() {
    for p in [Point::Request(1), Point::Call(1), Point::Request(2), Point::Call(2), Point::Request(3), Point::Call(3)] {
        let (mut r, m) = scripted(|r| {
            let mut steps: Vec<Step> = (1..=3)
                .map(|k| match p {
                    Point::Request(j) if j == k => Step::Stall { before_first_byte: k != 2, after_words: 2, for_ms: 30_000 },
                    Point::Call(j) if j == k => shell(&format!("echo {k}; {}", r.exec_sleep())),
                    _ => shell(&format!("echo {k}")),
                })
                .collect();
            steps.push(say("done"));
            steps
        });
        let mut l = r.cmd(&["--json", "three calls"]).spawn();
        match p {
            Point::Request(2) => drop(l.wait_record(5 * SEC, |x| x["t"] == "text")),
            Point::Request(k) => l.until("the request", 5 * SEC, |_| m.log().len() >= k),
            Point::Call(_) => drop(l.wait_marked(5 * SEC)),
        }
        l.kill9();
        let d = l.wait();
        d.finish();
        let pre = m.log().last().unwrap().text().to_string();
        let m2 = Mock::start(vec![say("resumed")]);
        r.at(&m2.url());
        let d = r.cmd(&["--json", "--resume"]).go();
        d.finish();
        d.check(d.code == Some(0), &format!("{p:?}: resume failed"));
        let first = m2.log()[0].text().to_string();
        assert!(first.starts_with(messages_prefix(&pre)), "{p:?}: the first request after resume does not start with the request before the crash:\n{pre}\n{first}");
        let repairs = d.journal_all().into_iter().filter(|x| x["t"] == "tool_result" && x["content"] == REPAIR).count();
        assert_eq!(repairs, usize::from(matches!(p, Point::Call(_))), "{p:?}: repairs\n{}", d.diag());
    }
}

#[test]
fn e2e_run_resume_no_task() {
    let m = Mock::start(vec![shell("echo partial"), say("never")]);
    let mut r = Run::new(&m.url());
    let d = r.cmd(&["--json", "--max-steps", "1", "work"]).go();
    d.finish();
    assert_eq!(d.result()["reason"], "max_steps", "{}", d.diag());
    let len = d.journal_bytes.len() as u64;
    let m2 = Mock::start(vec![say("continued")]);
    r.at(&m2.url());
    let d = r.cmd(&["--json", "--resume"]).go();
    d.finish();
    assert_eq!((d.code, d.result()["text"].as_str()), (Some(0), Some("continued")), "{}", d.diag());
    assert!(d.of("user").is_empty(), "no new user record");
    assert_eq!(d.of("session")[0]["resumed"], json!({"from_bytes": len}));
    let msgs = m2.log()[0].messages();
    assert_eq!(msgs.iter().filter(|x| x["role"] == "user").count(), 1);
    assert_eq!(msgs.last().unwrap()["role"], "tool", "the request continues from the last result");
}

/// An interrupt in the middle of a streamed reply leaves a partial assistant record (`finish: interrupted`). A request
/// may not end with it, so `run --resume` without a TASK refuses, and says why.
#[test]
fn e2e_run_resume_no_task_after_cut() {
    let m = Mock::start(vec![Step::Stall { before_first_byte: false, after_words: 2, for_ms: 30_000 }]);
    let r = Run::new(&m.url());
    let mut l = r.cmd(&["--json", "talk"]).spawn();
    l.ready();
    l.wait_record(5 * SEC, |x| x["t"] == "text");
    l.signal(libc::SIGINT);
    let d = l.wait();
    d.finish();
    assert_eq!(d.code, Some(130), "{}", d.diag());
    assert_eq!(d.of("assistant").last().unwrap()["finish"], "interrupted", "{}", d.diag());
    let d = r.cmd(&["--json", "--resume"]).go();
    d.finish();
    let e = &d.result()["error"];
    assert_eq!((d.code, e["kind"].as_str(), e["message"].as_str()), (Some(2), Some("config"), Some("the last reply was cut short; give a TASK")), "{}", d.diag());
    assert_eq!(m.log().len(), 1, "nothing was sent");
}

#[test]
fn e2e_resume_after_answer_refused() {
    let m = Mock::start(vec![say("finished")]);
    let r = Run::new(&m.url());
    r.json("do it").finish();
    let d = r.cmd(&["--json", "--resume"]).go();
    d.finish();
    let e = &d.result()["error"];
    assert_eq!((d.code, e["kind"].as_str(), e["message"].as_str()), (Some(2), Some("config"), Some("nothing to continue; give a TASK")), "{}", d.diag());
    assert_eq!(m.log().len(), 1, "nothing was sent");
}

/// The spill path a tool result names.
fn spill_of(content: &str, home: &std::path::Path) -> std::path::PathBuf {
    let shown = content.split("full output: ").nth(1).and_then(|s| s.split(" ...]").next()).unwrap_or_else(|| panic!("no spill marker: {content}"));
    shown.strip_prefix("~/").map_or_else(|| shown.into(), |rest| home.join(rest))
}

#[test]
fn e2e_resume_spill_readable() {
    let big = "head -c 100000 /dev/zero | tr '\\0' z";
    let m = Mock::start(vec![shell(big), say("first")]);
    let mut r = Run::new(&m.url());
    let d = r.json("make output");
    d.finish();
    let first = spill_of(d.results()[0]["content"].as_str().unwrap(), &r.home);
    assert!(first.file_name().unwrap().to_str().unwrap().ends_with(&format!("{}-1.log", d.id())), "{}", first.display());
    let m2 = Mock::start(vec![shell(big), say("second")]);
    r.at(&m2.url());
    let d = r.cmd(&["--json", "--resume", "again"]).go();
    d.finish();
    let second = spill_of(d.results()[0]["content"].as_str().unwrap(), &r.home);
    assert!(second.file_name().unwrap().to_str().unwrap().ends_with("-2.log"), "the counter continues: {}", second.display());
    for f in [first, second] {
        let text = std::fs::read(&f).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        assert!(text.len() >= 100_000, "{} holds {} bytes", f.display(), text.len());
    }
}

#[test]
fn e2e_fresh_home() {
    let m = Mock::start(vec![say("fresh")]);
    let mut r = Run::new(&m.url());
    r.config_text(None).unenv("XDG_STATE_HOME");
    let d = r.json("hello");
    d.finish();
    let e = &d.result()["error"];
    assert_eq!(d.code, Some(2));
    assert_eq!(
        (e["kind"].as_str(), e["message"].as_str()),
        (Some("config"), Some("no config; copy one: mkdir -p ~/.config/henki && cp $(brew --prefix)/share/henki/config.mac.toml ~/.config/henki/config.toml"))
    );
    r.env("HENKI_BASE_URL", &m.url()).env("HENKI_MODEL", "env-model");
    let d = r.json("hello");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    let s = &d.of("session")[0];
    assert_eq!((s["model"].as_str(), s["base_url_from"].as_str(), &s["config"]), (Some("env-model"), Some("HENKI_BASE_URL"), &Value::Null));
    assert_eq!(m.log()[0].json["model"], "env-model");
    let mode = |p: &std::path::Path| std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(p).unwrap().permissions()) & 0o777;
    let state = r.home.join(".local/state/henki");
    assert_eq!((mode(&state), mode(&state.join("sessions")), mode(&state.join("spill"))), (0o700, 0o700, 0o700));
    assert_eq!(mode(d.journal.as_ref().unwrap()), 0o600);
    assert!(d.journal.as_ref().unwrap().starts_with(&state));
}

#[test]
fn e2e_empty_task_refused() {
    let m = Mock::start(vec![]);
    let r = Run::new(&m.url());
    for task in ["", "  \n\t "] {
        let d = r.json(task);
        d.finish();
        let e = &d.result()["error"];
        assert_eq!((d.code, e["kind"].as_str(), e["message"].as_str()), (Some(2), Some("config"), Some("task is empty")), "{}", d.diag());
    }
    let d = r.cmd(&[""]).go();
    d.finish();
    assert_eq!((d.code, d.err()), (Some(2), "henki: task is empty\n".to_string()));
    assert!(m.log().is_empty() && r.journals().is_empty());
}

#[test]
fn e2e_task_slash_is_text() {
    let m = Mock::start(vec![say("not a command")]);
    let r = Run::new(&m.url());
    let d = r.json("/help me with this");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    assert!(m.log()[0].contents("user")[0].ends_with("\n\n/help me with this"));
}

#[test]
fn e2e_task_stdin_1mib() {
    let m = Mock::start(vec![say("read it all")]);
    let mut r = Run::new(&m.url());
    r.set("context_window", "2147483647");
    let task = "x".repeat(1 << 20);
    let d = r.cmd(&["--json", "-"]).stdin(In::Bytes(task.clone().into_bytes())).go();
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    assert!(m.log()[0].contents("user")[0].ends_with(&format!("\n\n{task}")), "the whole task arrived");
    let d = r.cmd(&["--json", "-"]).stdin(In::Bytes(vec![b'y'; (1 << 20) + 1])).go();
    d.finish();
    assert_eq!((d.code, d.result()["error"]["kind"].as_str()), (Some(2), Some("config")), "{}", d.diag());
    assert_eq!(m.log().len(), 1);
}

#[test]
fn e2e_run_stdin_untouched() {
    let m = Mock::start(vec![shell("cat; echo rc=$?"), say("done")]);
    let r = Run::new(&m.url());
    let input = r.dir.join("input.txt");
    std::fs::write(&input, "not for henki\n".repeat(1000)).unwrap();
    let d = r.cmd(&["--json", "do not read stdin"]).stdin(In::File(std::fs::File::open(&input).unwrap())).timeout(Duration::from_secs(20)).go();
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    assert!(d.results()[0]["content"].as_str().unwrap().starts_with("rc=0\n"), "a tool's stdin is empty");
    let mut f = d.stdin_file.as_ref().unwrap();
    assert_eq!(f.stream_position().unwrap(), 0, "henki read its stdin");
}
