#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
//! CONTRACTS.md read as data: its index, and the field tables of journal/1 and json/1 that every record henki writes
//! is checked against. Parsing the document itself, rather than a copy of its tables here, keeps the two from drifting.
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub fn text() -> String {
    std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/CONTRACTS.md")).unwrap()
}

/// The `## NAME` section: its lines up to the next `## ` heading.
pub fn section(doc: &str, name: &str) -> String {
    let head = format!("## {name}\n");
    let at = doc.find(&head).unwrap_or_else(|| panic!("CONTRACTS.md has no section {name:?}"));
    let rest = &doc[at + head.len()..];
    rest[..rest.find("\n## ").unwrap_or(rest.len())].to_string()
}

/// The rows ``| `KEY` | CELL |`` of the table under the `### SUB` heading (or of the whole section when `sub` is
/// empty): KEY, and the backticked words of CELL.
pub fn table(sec: &str, sub: &str) -> Vec<(String, Vec<String>)> {
    let body = if sub.is_empty() {
        sec.to_string()
    } else {
        let head = format!("### {sub}\n");
        let at = sec.find(&head).unwrap_or_else(|| panic!("no subsection {sub:?}"));
        let rest = &sec[at + head.len()..];
        rest[..rest.find("\n### ").unwrap_or(rest.len())].to_string()
    };
    body.lines()
        .filter_map(|l| {
            let cells: Vec<&str> = l.strip_prefix("| `")?.splitn(3, " | ").collect();
            let key = cells.first()?.strip_suffix('`')?.to_string();
            let words = cells.get(1)?.split('`').skip(1).step_by(2).map(String::from).collect();
            Some((key, words))
        })
        .collect()
}

/// A record kind's documented fields: path -> required.
pub type Kinds = BTreeMap<String, BTreeMap<String, bool>>;

/// The journal/1 record kinds, and json/1's stream-only kinds.
pub fn kinds() -> Kinds {
    let doc = text();
    let rows = table(&section(&doc, "journal/1"), "Record kinds").into_iter().chain(table(&section(&doc, "json/1"), "Stream-only kinds"));
    rows.map(|(k, fields)| (k, fields.iter().map(|f| f.strip_suffix('?').map_or((f.clone(), true), |o| (o.to_string(), false))).collect())).collect()
}

/// Every field path of a record, in the notation of the tables: `a`, `a.b`, `a[].b`. `args` and `blocks` are opaque.
pub fn paths(v: &Value) -> BTreeSet<String> {
    fn walk(v: &Value, at: &str, out: &mut BTreeSet<String>) {
        let objects: Vec<(&str, &serde_json::Map<String, Value>)> = match v {
            Value::Object(o) => vec![("", o)],
            Value::Array(a) => a.iter().filter_map(Value::as_object).map(|o| ("[]", o)).collect(),
            _ => vec![],
        };
        for (sep, o) in objects {
            for (k, x) in o {
                let p = if at.is_empty() { k.clone() } else { format!("{at}{sep}.{k}") };
                out.insert(p.clone());
                if k != "args" && k != "blocks" {
                    walk(x, &p, out);
                }
            }
        }
    }
    let mut out = BTreeSet::new();
    walk(v, "", &mut out);
    out
}

/// What a set of records showed: the kinds and the fields of each.
#[derive(Default)]
pub struct Seen(BTreeMap<String, BTreeSet<String>>);

impl Seen {
    /// Holds `rec` to its kind's table (no field the table lacks, every required one outside arrays), and notes it.
    pub fn check(&mut self, kinds: &Kinds, rec: &Value, from: &str) {
        let t = rec["t"].as_str().unwrap_or_else(|| panic!("{from}: a record without t: {rec}"));
        let doc = kinds.get(t).unwrap_or_else(|| panic!("{from}: kind {t:?} is not in CONTRACTS.md: {rec}"));
        let got = paths(rec);
        let extra: Vec<&String> = got.iter().filter(|p| !doc.contains_key(*p)).collect();
        assert!(extra.is_empty(), "{from}: {t} has fields CONTRACTS.md does not list: {extra:?}\n{rec}");
        let missing: Vec<&String> = doc.iter().filter(|(p, req)| **req && !p.contains("[]") && !got.contains(*p)).map(|(p, _)| p).collect();
        assert!(missing.is_empty(), "{from}: {t} lacks required fields {missing:?}\n{rec}");
        self.0.entry(t.into()).or_default().extend(got);
    }

    /// Every documented kind and field was seen at least once, so none is dead text.
    pub fn covers(&self, kinds: &Kinds) {
        for (t, fields) in kinds {
            let seen = self.0.get(t).unwrap_or_else(|| panic!("no {t} record was seen"));
            let unseen: Vec<&String> = fields.keys().filter(|p| !seen.contains(*p)).collect();
            assert!(unseen.is_empty(), "{t}: documented fields never seen: {unseen:?}");
        }
    }
}

/// The index names every contract this henki ships, with its owner, and each has its section.
#[test]
fn contracts_index() {
    let doc = text();
    let rows = table(&section(&doc, "Index"), "");
    let names: Vec<&str> = rows.iter().map(|(k, _)| k.as_str()).collect();
    let owners = [
        ("journal/1", "henki"),
        ("json/1", "henki"),
        ("wire/1", "henki"),
        ("ext/1", "henki"),
        ("port/1", "henki"),
        ("history/1", "henki"),
        ("conform/1", "henki"),
        ("module/1", "henki-mods"),
        ("lock/1", "henki-mods"),
        ("hd1", "henki-mods"),
    ];
    assert_eq!(names, owners.map(|o| o.0));
    for (name, owner) in owners {
        assert!(doc.contains(&format!("| `{name}` | {owner} |")), "{name} is not owned by {owner}");
        let sec = section(&doc, name);
        assert!(owner == "henki" || sec.starts_with("\nOwned by henki-mods"), "{name}: the section does not say it is owned by henki-mods");
    }
}

/// The stability rule: additive within a major, a break needs a new major and a release of dual support.
#[test]
fn contracts_stability_rule() {
    let s = section(&text(), "Stability").split_whitespace().collect::<Vec<_>>().join(" ");
    for words in [
        "Within a major, changes are additive",
        "A reader skips a record kind it does not know and ignores a field it does not know",
        "A break needs a new major",
        "supports the old and the new major side by side for at least one release",
    ] {
        assert!(s.contains(words), "the stability rule lost {words:?}");
    }
}

/// The notation the tables use, on one record.
#[test]
fn contracts_paths_notation() {
    let r = serde_json::json!({"t": "assistant", "msg": {"tool_calls": [{"args": {"command": "x"}, "id": "a"}]}, "blocks": [{"type": "text"}], "resumed": null});
    let want = ["blocks", "msg", "msg.tool_calls", "msg.tool_calls[].args", "msg.tool_calls[].id", "resumed", "t"];
    assert_eq!(paths(&r).iter().map(String::as_str).collect::<Vec<_>>(), want);
}
