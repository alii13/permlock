use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::analyze::PackageFacts;
use crate::graph::Package;
use crate::inspect::{Use, reduce_strings};

#[derive(Clone, Debug)]
pub enum Json {
    Bool(bool),
    Str(String),
    Obj(BTreeMap<String, Json>),
}

/// Byte-identical to lavamoat-core's `jsonStringifySortedPolicy`: json-stable-stringify
/// with 2-space indent, no trailing newline, and empty objects split over two lines.
pub fn stringify(v: &Json) -> String {
    let mut out = String::new();
    write(v, 0, &mut out);
    out
}

fn write(v: &Json, level: usize, out: &mut String) {
    match v {
        Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Json::Str(s) => quote(s, out),
        Json::Obj(map) => {
            let indent = "  ".repeat(level);
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by_cached_key(|(k, _)| sort_key(k));
            out.push('{');
            for (i, (k, v)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push('\n');
                out.push_str(&indent);
                out.push_str("  ");
                quote(k, out);
                out.push_str(": ");
                write(v, level + 1, out);
            }
            out.push('\n');
            out.push_str(&indent);
            out.push('}');
        }
    }
}

/// LavaMoat's diff-friendly comparator: `a>b>c` sorts as `c<b<a`, so packages group
/// by their own name, not by who pulled them in.
// ponytail: Rust compares UTF-8 bytes, JS compares UTF-16 units. Same order for npm
// names and JS identifiers; differs only above U+FFFF.
fn sort_key(k: &str) -> String {
    k.split('>').rev().collect::<Vec<_>>().join("<")
}

fn quote(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// @lavamoat/node compilePolicy, then lavamoat-core mergePolicy with policy-override.json.
pub fn build(pkgs: &[Package], facts: &[PackageFacts], overrides: Option<&Value>) -> Json {
    let mut resources = BTreeMap::new();
    for (pkg, f) in pkgs.iter().zip(facts).skip(1) {
        let mut entry = BTreeMap::new();
        if !f.globals.is_empty() {
            let keys = reduce_strings(f.globals.keys().cloned().collect());
            let globals = keys.into_iter().map(|k| {
                let v = match f.globals[&k] {
                    Use::Read => Json::Bool(true),
                    Use::Write => Json::Str("write".into()),
                };
                (k, v)
            });
            entry.insert("globals".to_string(), Json::Obj(globals.collect()));
        }
        if !f.builtins.is_empty() {
            let builtins = reduce_strings(f.builtins.clone());
            entry.insert("builtin".to_string(), all_true(builtins));
        }
        if !f.packages.is_empty() {
            let names = f.packages.iter().map(|&i| pkgs[i].canonical.clone());
            entry.insert("packages".to_string(), all_true(names));
        }
        if !entry.is_empty() {
            resources.insert(pkg.canonical.clone(), Json::Obj(entry));
        }
    }
    if let Some(o) = overrides.and_then(|o| o["resources"].as_object()) {
        merge_overrides(&mut resources, o);
    }
    // ponytail: only `resources` is merged from overrides; `include`/`root` copy when a repo needs them
    Json::Obj(BTreeMap::from([(
        "resources".to_string(),
        Json::Obj(resources),
    )]))
}

fn all_true(keys: impl IntoIterator<Item = String>) -> Json {
    Json::Obj(keys.into_iter().map(|k| (k, Json::Bool(true))).collect())
}

fn from_value(v: &Value) -> Json {
    match v {
        Value::Bool(b) => Json::Bool(*b),
        Value::String(s) => Json::Str(s.clone()),
        Value::Object(o) => Json::Obj(o.iter().map(|(k, v)| (k.clone(), from_value(v))).collect()),
        other => Json::Str(other.to_string()),
    }
}

fn merge_overrides(
    resources: &mut BTreeMap<String, Json>,
    overrides: &serde_json::Map<String, Value>,
) {
    for (name, theirs) in overrides {
        let Json::Obj(b) = from_value(theirs) else {
            continue;
        };
        let mut merged = match resources.remove(name) {
            Some(Json::Obj(a)) => merge_resource(a, &b),
            _ => b.clone(),
        };
        for section in ["globals", "builtin"] {
            if let Some(Json::Obj(items)) = merged.get_mut(section) {
                dedupe(items, b.get(section));
            }
        }
        resources.insert(name.clone(), Json::Obj(merged));
    }
}

/// lavamoat-core mergeResourcePolicy: override keys win inside each section.
fn merge_resource(
    mut a: BTreeMap<String, Json>,
    b: &BTreeMap<String, Json>,
) -> BTreeMap<String, Json> {
    let mut out = BTreeMap::new();
    for section in ["globals", "builtin", "packages", "meta"] {
        let (x, y) = (a.remove(section), b.get(section));
        if x.is_none() && y.is_none() {
            continue;
        }
        let mut items = match x {
            Some(Json::Obj(m)) => m,
            _ => BTreeMap::new(),
        };
        if let Some(Json::Obj(m)) = y {
            items.extend(m.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        out.insert(section.to_string(), Json::Obj(items));
    }
    for section in ["native", "env"] {
        if let Some(v) = b.get(section).cloned().or_else(|| a.remove(section)) {
            out.insert(section.to_string(), v);
        }
    }
    out
}

/// lavamoat-core dedupePolicyPaths: drop children of present parents, then re-add
/// override entries that are not `false`.
fn dedupe(items: &mut BTreeMap<String, Json>, overrides: Option<&Json>) {
    let keys: BTreeSet<String> = items.keys().cloned().collect();
    let keep = reduce_strings(keys);
    items.retain(|k, _| keep.contains(k));
    if let Some(Json::Obj(o)) = overrides {
        for (k, v) in o {
            if !matches!(v, Json::Bool(false)) && !items.contains_key(k) {
                items.insert(k.clone(), v.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj<const N: usize>(entries: [(&str, Json); N]) -> Json {
        Json::Obj(
            entries
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        )
    }

    #[test]
    fn writes_like_lavamoat() {
        let policy = obj([(
            "resources",
            obj([
                (
                    "b>zod",
                    obj([("globals", obj([("process.env", Json::Bool(true))]))]),
                ),
                ("a", obj([])),
                (
                    "c>a",
                    obj([
                        (
                            "builtin",
                            obj([
                                ("node:http.createServer", Json::Bool(true)),
                                ("node:http.STATUS_CODES", Json::Bool(true)),
                            ]),
                        ),
                        ("globals", obj([("Request", Json::Str("write".into()))])),
                    ]),
                ),
            ]),
        )]);
        let expected = r#"{
  "resources": {
    "a": {
    },
    "c>a": {
      "builtin": {
        "node:http.STATUS_CODES": true,
        "node:http.createServer": true
      },
      "globals": {
        "Request": "write"
      }
    },
    "b>zod": {
      "globals": {
        "process.env": true
      }
    }
  }
}"#;
        assert_eq!(stringify(&policy), expected);
    }

    #[test]
    fn escapes_strings_like_json_stringify() {
        let v = obj([("a\"b\\c\n\u{1}", Json::Str("\t/é".into()))]);
        assert_eq!(
            stringify(&v),
            "{\n  \"a\\\"b\\\\c\\n\\u0001\": \"\\t/é\"\n}"
        );
    }

    #[test]
    fn orders_resources_like_lavamoat_on_recall() {
        let file = include_str!("../fixtures/recall-api/lavamoat-policy.json");
        let lavamoat: Vec<&str> = file
            .lines()
            .filter_map(|l| l.strip_prefix("    \"")?.strip_suffix("\": {"))
            .collect();
        assert!(
            lavamoat.len() >= 15,
            "fixture parse found {}",
            lavamoat.len()
        );
        let mut ours = lavamoat.clone();
        ours.reverse();
        ours.sort_by_key(|k| sort_key(k));
        assert_eq!(ours, lavamoat);
    }
}
