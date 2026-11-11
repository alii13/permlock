//! The open capability report: what each package was seen touching, by class,
//! with the first place it happens. And the check that compares two reports.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::analyze::PackageFacts;
use crate::graph::Package;
use crate::inspect::Seen;

pub const SCHEMA: &str = "permlock/v0";

struct Class {
    name: String,
    builtins: Vec<String>,
    globals: Vec<String>,
    scripts: Vec<String>,
    /// suffixes of loaded files, like `.node`
    files: Vec<String>,
    /// files whose presence in the package counts, like `binding.gyp`
    package_files: Vec<String>,
}

fn vocabulary() -> Vec<Class> {
    let v: Value =
        serde_json::from_str(include_str!("../spec/capabilities.json")).expect("vocabulary");
    let list = |c: &Value, k: &str| -> Vec<String> {
        c[k].as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| s.as_str().map(String::from))
            .collect()
    };
    v["classes"]
        .as_object()
        .expect("classes")
        .iter()
        .map(|(name, c)| Class {
            name: name.clone(),
            builtins: list(c, "builtins"),
            globals: list(c, "globals"),
            scripts: list(c, "scripts"),
            files: list(c, "files"),
            package_files: list(c, "package_files"),
        })
        .collect()
}

/// (class, via) for an observation, or None when it is pure.
fn classify<'a>(vocab: &'a [Class], s: &Seen) -> Option<(&'a str, String)> {
    if s.builtin {
        let m = s.key.strip_prefix("node:").unwrap_or(&s.key);
        let c = vocab.iter().find(|c| {
            c.builtins
                .iter()
                .any(|b| m == b || m.starts_with(&format!("{b}/")))
        })?;
        return Some((&c.name, s.key.clone()));
    }
    vocab.iter().find_map(|c| {
        let g = c
            .globals
            .iter()
            .find(|g| s.key == **g || s.key.starts_with(&format!("{g}.")))?;
        Some((c.name.as_str(), g.clone()))
    })
}

/// What one package was seen touching: class -> evidence, earliest place first.
pub fn observed(pkg: &Package, f: &PackageFacts) -> serde_json::Map<String, Value> {
    let vocab = vocabulary();
    let rel = |file: &std::path::Path| {
        file.strip_prefix(&pkg.dir)
            .unwrap_or(file)
            .display()
            .to_string()
    };
    // class -> via -> earliest (file, line)
    let mut found: BTreeMap<&str, BTreeMap<String, (String, u32)>> = BTreeMap::new();
    for (file, s) in &f.seen {
        let Some((class, via)) = classify(&vocab, s) else {
            continue;
        };
        let at = (rel(file), s.line);
        let slot = found
            .entry(class)
            .or_default()
            .entry(via)
            .or_insert(at.clone());
        if at < *slot {
            *slot = at;
        }
    }
    for c in &vocab {
        let mut note = |via: String, file: String| {
            found.entry(&c.name).or_default().insert(via, (file, 0));
        };
        for script in &c.scripts {
            if pkg.descriptor["scripts"][script].is_string() {
                note(script.clone(), "package.json".into());
            }
        }
        for file in &f.native {
            let name = file
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if c.files.iter().any(|suffix| name.ends_with(suffix.as_str())) {
                note(name, rel(file));
            }
        }
        for pf in &c.package_files {
            if pkg.dir.join(pf).is_file() {
                note(pf.clone(), pf.clone());
            }
        }
    }
    found
        .into_iter()
        .map(|(class, vias)| {
            let mut ev: Vec<_> = vias.into_iter().collect();
            ev.sort_by(|a, b| a.1.cmp(&b.1));
            let ev = ev
                .into_iter()
                .map(|(via, (file, line))| json!({ "file": file, "line": line, "via": via }));
            (class.to_string(), Value::Array(ev.collect()))
        })
        .collect()
}

pub fn build(pkgs: &[Package], facts: &[PackageFacts]) -> Value {
    let mut ordered: Vec<(&Package, &PackageFacts)> = pkgs.iter().zip(facts).skip(1).collect();
    ordered.sort_by(|a, b| a.0.canonical.cmp(&b.0.canonical));
    let packages: serde_json::Map<String, Value> = ordered
        .into_iter()
        .map(|(pkg, f)| {
            let entry = json!({
                "version": pkg.version,
                "observed": observed(pkg, f),
                "declared": pkg.descriptor.get("capabilities").cloned().unwrap_or(Value::Null),
            });
            (pkg.canonical.clone(), entry)
        })
        .collect();
    json!({ "schema": SCHEMA, "profile": "node", "packages": packages })
}

/// Plain-words label per class, in the order `init` prints them.
const LABELS: &[(&str, &str)] = &[
    ("net", "touch the network"),
    ("fs", "touch files"),
    ("env", "read env"),
    ("exec", "run child processes"),
    ("eval", "evaluate code"),
    ("worker", "start workers"),
    ("native", "load native code"),
    ("system", "touch the system"),
    ("install", "run install scripts"),
];

/// The table `init` prints: how many packages hold each power, and who.
pub fn summary(report: &Value) -> String {
    let empty = serde_json::Map::new();
    let packages = report["packages"].as_object().unwrap_or(&empty);
    let mut s = String::new();
    for (class, label) in LABELS {
        let names: Vec<&str> = packages
            .iter()
            .filter(|(_, p)| p["observed"].get(*class).is_some())
            .map(|(n, _)| n.as_str())
            .collect();
        if names.is_empty() {
            continue;
        }
        let mut who = names.iter().take(3).copied().collect::<Vec<_>>().join(", ");
        if names.len() > 3 {
            who.push_str(&format!(", +{}", names.len() - 3));
        }
        s.push_str(&format!("  {label:<22} {:>5}   {who}\n", names.len()));
    }
    let pure = packages
        .values()
        .filter(|p| p["observed"].as_object().is_none_or(|o| o.is_empty()))
        .count();
    s.push_str(&format!("  {:<22} {pure:>5}\n", "need nothing"));
    s
}

/// A shields.io endpoint badge: `needs nothing`, or the classes a package declares.
pub fn badge(classes: &[String]) -> Value {
    let (message, color) = if classes.is_empty() {
        ("needs nothing".to_string(), "111111")
    } else {
        (format!("needs {}", classes.join(", ")), "ffcc00")
    };
    json!({ "schemaVersion": 1, "label": "permlock", "message": message, "color": color })
}

pub struct Escalation {
    pub package: String,
    pub before: Option<String>,
    pub after: String,
    /// (class, via, file:line)
    pub powers: Vec<(String, String, String)>,
    pub undeclared: bool,
}

pub struct Outcome {
    pub escalations: Vec<Escalation>,
    pub info: usize,
}

fn classes(p: &Value) -> BTreeSet<String> {
    p["observed"]
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default()
}

/// A new class is an escalation, and so is a class the package's own declaration leaves out.
/// Removed powers, version bumps, and new packages that touch nothing are info.
pub fn check(base: &Value, head: &Value) -> Outcome {
    let empty = serde_json::Map::new();
    let base_pkgs = base["packages"].as_object().unwrap_or(&empty);
    let head_pkgs = head["packages"].as_object().unwrap_or(&empty);
    let mut out = Outcome {
        escalations: vec![],
        info: 0,
    };
    for (name, h) in head_pkgs {
        let b = base_pkgs.get(name);
        let had = b.map(classes).unwrap_or_default();
        let declared: Option<BTreeSet<String>> = h["declared"].as_array().map(|d| {
            d.iter()
                .filter_map(|c| c.as_str().map(String::from))
                .collect()
        });
        let mut powers = vec![];
        let mut undeclared = false;
        for class in classes(h) {
            let off_list = declared.as_ref().is_some_and(|d| !d.contains(&class));
            if had.contains(&class) && !off_list {
                continue;
            }
            undeclared |= off_list;
            for ev in h["observed"][&class].as_array().into_iter().flatten() {
                let at = match ev["line"].as_u64() {
                    Some(0) | None => ev["file"].as_str().unwrap_or("").to_string(),
                    Some(l) => format!("{}:{l}", ev["file"].as_str().unwrap_or("")),
                };
                powers.push((
                    class.clone(),
                    ev["via"].as_str().unwrap_or("").to_string(),
                    at,
                ));
            }
        }
        let version = |p: &Value| p["version"].as_str().unwrap_or("").to_string();
        if !powers.is_empty() {
            out.escalations.push(Escalation {
                package: name.clone(),
                before: b.map(version),
                after: version(h),
                powers,
                undeclared,
            });
        } else if b.is_none_or(|b| b != h) {
            out.info += 1;
        }
    }
    out.info += base_pkgs
        .keys()
        .filter(|k| !head_pkgs.contains_key(*k))
        .count();
    out
}

pub fn render(o: &Outcome, packages: usize) -> String {
    if o.escalations.is_empty() {
        let info = if o.info > 0 {
            format!(", {} changes are info", o.info)
        } else {
            String::new()
        };
        return format!("✓ no new powers ({packages} packages{info})\n");
    }
    let mut s = String::new();
    for e in &o.escalations {
        let version = match &e.before {
            Some(b) if *b != e.after => format!("{b} → {}", e.after),
            Some(_) => e.after.clone(),
            None => format!("{} (new package)", e.after),
        };
        let note = if e.undeclared {
            ", not in its declared capabilities"
        } else {
            ""
        };
        let n = e.powers.len();
        s.push_str(&format!(
            "\n  {}  {version}    {n} escalation{}{note}\n\n",
            e.package,
            if n == 1 { "" } else { "s" }
        ));
        for (class, via, at) in &e.powers {
            s.push_str(&format!("    + {class:<7} {via:<20} {at}\n"));
        }
    }
    if o.info > 0 {
        s.push_str(&format!("\n  {} other changes are info.\n", o.info));
    }
    let n = o.escalations.len();
    s.push_str(&format!(
        "\n✗ {n} package{} asked for new powers.\n",
        if n == 1 { "" } else { "s" }
    ));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock(packages: Value) -> Value {
        json!({ "schema": SCHEMA, "profile": "node", "packages": packages })
    }

    fn pkg(version: &str, classes: &[&str]) -> Value {
        let observed: serde_json::Map<String, Value> = classes
            .iter()
            .map(|c| {
                (
                    c.to_string(),
                    json!([{ "file": "index.js", "line": 1, "via": "x" }]),
                )
            })
            .collect();
        json!({ "version": version, "observed": observed, "declared": null })
    }

    #[test]
    fn a_new_package_with_powers_escalates_and_one_without_is_info() {
        let base = lock(json!({}));
        let head = lock(
            json!({ "flatmap-stream": pkg("0.1.1", &["net"]), "left-pad": pkg("1.3.0", &[]) }),
        );
        let out = check(&base, &head);
        assert_eq!(out.escalations.len(), 1);
        assert_eq!(out.escalations[0].package, "flatmap-stream");
        assert_eq!(out.escalations[0].before, None);
        assert_eq!(out.info, 1);
        assert!(render(&out, 2).contains("0.1.1 (new package)"));
    }

    #[test]
    fn removed_powers_and_plain_bumps_are_info() {
        let base = lock(
            json!({ "a": pkg("1.0.0", &["net", "fs"]), "b": pkg("1.0.0", &["env"]), "gone": pkg("1.0.0", &[]) }),
        );
        let head = lock(json!({ "a": pkg("1.1.0", &["net"]), "b": pkg("2.0.0", &["env"]) }));
        let out = check(&base, &head);
        assert!(out.escalations.is_empty());
        assert_eq!(out.info, 3);
        assert_eq!(
            render(&out, 2),
            "✓ no new powers (2 packages, 3 changes are info)\n"
        );
    }
}
