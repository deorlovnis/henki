#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
//! Nothing inert: every config key and every `run` flag changes the request, the child or the result.
use crate::harness::*;
use crate::mock::*;
use serde_json::json;
use std::time::Duration;

fn content(d: &Done, i: usize) -> String {
    d.results().get(i).and_then(|r| r["content"].as_str()).unwrap_or_else(|| panic!("no tool result {i}\n{}", d.diag())).to_string()
}

/// One run of `steps` with config `keys`; returns the run and the mock's log.
fn with_keys(keys: &[(&str, &str)], steps: Vec<Step>, args: &[&str]) -> (Done, Vec<Logged>) {
    let m = Mock::start(steps);
    let mut r = Run::new(&m.url());
    for (k, v) in keys {
        r.set(k, v);
    }
    let d = r.cmd(&[&["--json"], args].concat()).go();
    d.finish();
    (d, m.log())
}

#[test]
fn config_key_effect_model() {
    let (d, log) = with_keys(&[("model", "\"alpha-7\"")], vec![say("ok")], &["hi"]);
    assert_eq!((log[0].json["model"].as_str(), d.of("session")[0]["model"].as_str()), (Some("alpha-7"), Some("alpha-7")));
}

#[test]
fn config_key_effect_base_url() {
    let m = Mock::start(vec![say("ok")]);
    let r = Run::new(&format!("http://127.0.0.1:{}/custom/v1/", m.port()));
    let d = r.json("hi");
    d.finish();
    assert_eq!(m.log()[0].line, "POST /custom/v1/chat/completions HTTP/1.1", "one trailing / stripped, the path kept");
    assert_eq!(d.of("session")[0]["base_url"], format!("http://127.0.0.1:{}/custom/v1", m.port()));
}

#[test]
fn config_key_effect_dialect() {
    let (d, log) = with_keys(&[("dialect", "\"openai\"")], vec![say("ok")], &["hi"]);
    assert_eq!((d.code, d.of("session")[0]["dialect"].as_str()), (Some(0), Some("openai")), "{}", d.diag());
    assert_eq!(log[0].line, "POST /v1/chat/completions HTTP/1.1");
    let m = Mock::start_api(Api::Anthropic, vec![say("ok")]);
    let mut r = Run::new(&m.url());
    r.set("dialect", "\"anthropic\"");
    let d = r.json("hi");
    d.finish();
    assert_eq!((d.code, d.of("session")[0]["dialect"].as_str()), (Some(0), Some("anthropic")), "{}", d.diag());
    assert_eq!((m.log()[0].line.as_str(), m.rejections().len()), ("POST /v1/messages HTTP/1.1", 0), "{:?}", m.rejections());
    let (d, log) = with_keys(&[("dialect", "\"gemini\"")], vec![say("ok")], &["hi"]);
    assert_eq!((d.code, d.result()["error"]["kind"].as_str(), log.len()), (Some(2), Some("config"), 0), "{}", d.diag());
    assert!(d.result()["error"]["message"].as_str().unwrap().contains("unknown dialect gemini; fix: use dialect = \"openai\" or \"anthropic\""), "{}", d.result());
}

#[test]
fn config_key_effect_api_key() {
    let (_, log) = with_keys(&[("api_key", "\"sk-local-1\"")], vec![say("ok")], &["hi"]);
    assert_eq!(log[0].header("authorization"), Some("Bearer sk-local-1"));
    let (_, log) = with_keys(&[], vec![say("ok")], &["hi"]);
    assert_eq!(log[0].header("authorization"), None);
}

#[test]
fn config_key_effect_approve() {
    let (d, _) = with_keys(&[("approve", "\"ask\"")], vec![shell("touch ran"), say("ok")], &["touch"]);
    assert_eq!((d.code, d.of("session")[0]["approve"].as_str()), (Some(3), Some("ask")), "{}", d.diag());
    assert!(d.of("session")[0]["approve_from"].as_str().unwrap().ends_with("config.toml:3"), "{}", d.of("session")[0]);
    let (d, _) = with_keys(&[("approve", "\"auto\"")], vec![shell("touch ran"), say("ok")], &["touch"]);
    assert_eq!((d.code, d.of("approval")[0]["verdict"].as_str()), (Some(0), Some("auto")), "{}", d.diag());
}

#[test]
fn config_key_effect_tools() {
    let (d, log) = with_keys(&[("tools", "[\"read\", \"shell\",]")], vec![say("ok")], &["hi"]);
    let names: Vec<&str> = log[0].json["tools"].as_array().unwrap().iter().map(|t| t["function"]["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["shell", "read"], "canonical order");
    assert_eq!(d.of("session")[0]["tools"], json!(["shell", "read"]));
    let (d, _) = with_keys(&[("tools", "[\"read\"]")], vec![shell("touch ran"), say("ok")], &["touch"]);
    assert_eq!(content(&d, 0), "unknown tool shell; available: read");
}

#[test]
fn config_key_effect_ext() {
    let m = Mock::start(vec![say("ok"), say("ok")]);
    let mut r = Run::new(&m.url());
    let ext = r.dir.join("ext-dir");
    std::fs::create_dir_all(ext.join(".hidden")).unwrap();
    r.set("ext", &format!("[\"{}\"]", ext.display()));
    let d = r.json("hi");
    d.finish();
    assert_eq!((d.code, &d.of("session")[0]["exts"]), (Some(0), &json!([])), "an ext path with only dot entries is fine\n{}", d.diag());
    std::fs::create_dir_all(ext.join("notes")).unwrap();
    std::fs::write(ext.join("notes/doc.md"), "# notes: keeps notes\n").unwrap();
    let d = r.json("hi");
    d.finish();
    assert_eq!((d.code, &d.of("session")[0]["exts"]), (Some(0), &json!(["notes"])), "{}", d.diag());
    assert!(m.log()[1].system().contains(&format!("\n- notes: notes: keeps notes [{}]", ext.join("notes/doc.md").display())), "{}", m.log()[1].system());
    r.set("ext", &format!("[\"{}\"]", r.dir.join("missing").display()));
    let d = r.json("hi");
    d.finish();
    assert_eq!((d.code, d.result()["error"]["kind"].as_str(), m.log().len()), (Some(2), Some("config"), 2), "a listed ext path must exist\n{}", d.diag());
}

#[test]
fn config_key_effect_shadow() {
    let m = Mock::start(vec![say("ok")]);
    let mut r = Run::new(&m.url());
    let ext = r.dir.join("ext-dir");
    std::fs::create_dir_all(ext.join("wrap/bin")).unwrap();
    std::fs::write(ext.join("wrap/doc.md"), "wraps ls\n").unwrap();
    std::fs::write(ext.join("wrap/bin/ls"), "#!/bin/sh\necho wrapped\n").unwrap();
    std::fs::set_permissions(ext.join("wrap/bin/ls"), std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    r.set("ext", &format!("[\"{}\"]", ext.display()));
    let d = r.json("hi");
    d.finish();
    assert_eq!((d.code, d.result()["error"]["kind"].as_str()), (Some(2), Some("config")), "a command that shadows one on PATH needs shadow\n{}", d.diag());
    r.set("shadow", "[\"wrap/ls\"]");
    let d = r.json("hi");
    d.finish();
    assert_eq!((d.code, m.log().len()), (Some(0), 1), "{}", d.diag());
}

#[test]
fn config_key_effect_pass_env() {
    let steps = || vec![shell("echo \"[$FOO_SETTING]\""), say("ok")];
    let m = Mock::start(steps());
    let mut r = Run::new(&m.url());
    r.env("FOO_SETTING", "bar");
    let d = r.json("show");
    d.finish();
    assert!(content(&d, 0).starts_with("[]\n"), "not passed by default: {}", content(&d, 0));
    let m = Mock::start(steps());
    r.at(&m.url()).set("pass_env", "[\"FOO_SETTING\"]");
    let d = r.json("show");
    d.finish();
    assert!(content(&d, 0).starts_with("[bar]\n"), "{}", content(&d, 0));
    assert_eq!(d.of("session")[0]["pass_env"], json!(["FOO_SETTING"]));
}

#[test]
fn config_key_effect_max_steps() {
    let (d, log) = with_keys(&[("max_steps", "2")], (1..=4).map(|i| shell(&format!("echo {i}"))).collect(), &["count"]);
    assert_eq!((d.result()["reason"].as_str(), log.len()), (Some("max_steps"), 2), "{}", d.diag());
}

#[test]
fn config_key_effect_max_seconds() {
    let stall = Step::Stall { before_first_byte: true, after_words: 0, for_ms: 30_000 };
    let (d, _) = with_keys(&[("max_seconds", "2")], vec![stall], &["wait"]);
    assert_eq!(d.code, Some(124), "{}", d.diag());
    assert!(d.elapsed <= Duration::from_secs(3), "{:?}", d.elapsed);
}

#[test]
fn config_key_effect_tool_timeout_s() {
    let mut r = Run::new("");
    let m = Mock::start(vec![shell(&r.sleep()), say("ok")]);
    r.at(&m.url()).set("tool_timeout_s", "1");
    let d = r.json("wait");
    d.finish();
    assert!(content(&d, 0).ends_with("[timed out after 1 s; process group killed]"), "{}", content(&d, 0));
}

#[test]
fn config_key_effect_output_cap() {
    let five_k = || vec![shell("head -c 5000 /dev/zero | tr '\\0' c"), say("ok")];
    let (d, _) = with_keys(&[], five_k(), &["output"]);
    assert!(!content(&d, 0).contains("bytes cut"), "5,000 bytes fit the default cap");
    let (d, _) = with_keys(&[("output_cap", "1024")], five_k(), &["output"]);
    let c = content(&d, 0);
    assert!(c.contains("bytes cut; full output: ") && c.len() <= 1024 + 400, "{} bytes: {c}", c.len());
}

#[test]
fn config_key_effect_context_window() {
    let m = Mock::start(vec![shell("head -c 5000 /dev/zero | tr '\\0' w"), say("ok"), shell("head -c 5000 /dev/zero | tr '\\0' w"), say("ok")]);
    let mut r = Run::new(&m.url());
    r.unset("context_window");
    let d = r.json("output");
    d.finish();
    assert_eq!(d.of("session")[0]["ctx"], 32768);
    assert!(d.notices().contains(&"context_window is unknown; using 32,768 tokens".to_string()), "{:?}", d.notices());
    assert!(!content(&d, 0).contains("bytes cut"));
    r.set("context_window", "2048");
    let d = r.json("output");
    d.finish();
    assert_eq!(d.of("session")[0]["ctx"], 2048);
    assert!(d.notices().iter().all(|n| !n.starts_with("context_window is unknown")));
    let c = content(&d, 0);
    assert!(c.contains("bytes cut") && c.len() <= 1024 + 400, "the cap is half the window: {} bytes", c.len());
}

#[test]
fn config_key_effect_max_tokens() {
    let (_, log) = with_keys(&[("max_tokens", "77")], vec![say("ok")], &["hi"]);
    assert_eq!(log[0].json["max_tokens"], 77);
    let (_, log) = with_keys(&[], vec![say("ok")], &["hi"]);
    assert_eq!(log[0].json["max_tokens"], 4096);
}

#[test]
fn flag_effect_json() {
    let m = Mock::start(vec![say("plain answer"), say("plain answer")]);
    let r = Run::new(&m.url());
    let d = r.cmd(&["hi"]).go();
    d.finish();
    assert_eq!(d.out(), "plain answer\n");
    let d = r.cmd(&["--json", "hi"]).go();
    d.finish();
    let out = d.records();
    assert_eq!((out[0]["t"].as_str(), out.last().unwrap()["t"].as_str()), (Some("session"), Some("result")));
}

#[test]
fn flag_effect_max_steps() {
    let (d, log) = with_keys(&[("max_steps", "5")], (1..=3).map(|i| shell(&format!("echo {i}"))).collect(), &["--max-steps", "1", "count"]);
    assert_eq!((d.result()["reason"].as_str(), log.len()), (Some("max_steps"), 1), "the flag overrides the file\n{}", d.diag());
}

#[test]
fn flag_effect_max_seconds() {
    let stall = Step::Stall { before_first_byte: true, after_words: 0, for_ms: 30_000 };
    let (d, _) = with_keys(&[("max_seconds", "0")], vec![stall], &["--max-seconds", "2", "wait"]);
    assert_eq!(d.code, Some(124), "{}", d.diag());
    assert!(d.elapsed <= Duration::from_secs(3), "{:?}", d.elapsed);
}

#[test]
fn flag_effect_approve() {
    let (d, _) = with_keys(&[("approve", "\"ask\"")], vec![shell("echo ran"), say("ok")], &["--approve", "auto", "run"]);
    assert_eq!((d.code, d.of("session")[0]["approve_from"].as_str()), (Some(0), Some("--approve")), "{}", d.diag());
    let (d, _) = with_keys(&[("approve", "\"auto\"")], vec![shell("echo ran"), say("ok")], &["--approve", "ask", "run"]);
    assert_eq!(d.code, Some(3), "{}", d.diag());
    let (d, log) = with_keys(&[], vec![say("ok")], &["--approve", "maybe", "run"]);
    assert_eq!((d.code, d.result()["error"]["kind"].as_str(), log.len()), (Some(2), Some("config"), 0));
}

#[test]
fn flag_effect_config() {
    let m = Mock::start(vec![say("from the other file")]);
    let r = Run::new(&m.url());
    let alt = r.dir.join("alt");
    std::fs::create_dir_all(alt.join("ext")).unwrap();
    std::fs::set_permissions(&alt, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    let file = alt.join("henki.toml");
    std::fs::write(&file, format!("model = \"alt-model\"\nbase_url = \"{}\"\napprove = \"auto\"\n", m.url())).unwrap();
    std::fs::set_permissions(&file, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    let d = r.cmd(&["--json", "--config", file.to_str().unwrap(), "hi"]).go();
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    assert_eq!((m.log()[0].json["model"].as_str(), d.of("session")[0]["config"].as_str()), (Some("alt-model"), file.to_str()));
    let d = r.cmd(&["--json", "--config", alt.join("missing.toml").to_str().unwrap(), "hi"]).go();
    d.finish();
    assert_eq!((d.code, d.result()["error"]["kind"].as_str()), (Some(2), Some("config")), "an explicit config must exist");
}

#[test]
fn flag_effect_model() {
    let (d, log) = with_keys(&[], vec![say("ok")], &["--model", "flag-model", "hi"]);
    assert_eq!((log[0].json["model"].as_str(), d.of("session")[0]["model"].as_str()), (Some("flag-model"), Some("flag-model")));
}

#[test]
fn flag_effect_resume() {
    let m = Mock::start(vec![say("first"), say("second"), say("third"), say("fourth")]);
    let r = Run::new(&m.url());
    let a = r.json("session a");
    a.finish();
    let b = r.json("session b");
    b.finish();
    assert_ne!(a.journal, b.journal);
    let d = r.cmd(&["--json", format!("--resume={}", a.id()).as_str(), "back to a"]).go();
    d.finish();
    assert_eq!((d.journal.as_ref(), d.id()), (a.journal.as_ref(), a.id()), "--resume=ID continues that session");
    assert_eq!(m.log()[2].contents("user").len(), 2);
    assert!(m.log()[2].contents("user")[0].ends_with("session a"));
    let d = r.cmd(&["--json", "--resume", "the newest"]).go();
    d.finish();
    assert_eq!(d.journal.as_ref(), a.journal.as_ref(), "--resume takes the newest session");
    assert_eq!(r.journals().len(), 2, "no new session file");
}

#[test]
fn flag_effect_nested() {
    let m = Mock::start(vec![say("ok")]);
    let mut r = Run::new(&m.url());
    r.env("HENKI_DEPTH", "1");
    let d = r.json("hi");
    d.finish();
    assert_eq!((d.code, d.result()["error"]["kind"].as_str()), (Some(2), Some("nested")));
    let d = r.cmd(&["--json", "--nested", "hi"]).go();
    d.finish();
    assert_eq!((d.code, m.log().len()), (Some(0), 1), "{}", d.diag());
}
