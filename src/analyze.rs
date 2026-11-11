//! Walk modules from the entry file the way Endo loads them, inspect each one, and
//! fold the results into one set of facts per package.

use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::graph::Package;
use crate::inspect::{self, Seen, Use};

#[derive(Default, Debug)]
pub struct PackageFacts {
    pub globals: BTreeMap<String, Use>,
    pub builtins: BTreeSet<String>,
    /// other packages this one actually imports, by index
    pub packages: BTreeSet<usize>,
    pub seen: Vec<(PathBuf, Seen)>,
    /// native addons (`.node` files) this package loads
    pub native: Vec<PathBuf>,
    pub inspected: bool,
}

/// LavaMoat's conditions plus the two Endo always adds.
const CONDITIONS: &[&str] = &["node", "import", "default"];
/// Endo's default search suffixes for a specifier without an exact file.
const SUFFIXES: &[&str] = &[
    "",
    ".js",
    ".json",
    ".node",
    "/index.js",
    "/index.json",
    "/index.node",
];

/// Index 0 is the root package; its facts are kept for `declare` and `verify`,
/// and the policy and report leave it out.
pub fn analyze(pkgs: &[Package], entries: &[PathBuf]) -> Vec<PackageFacts> {
    let mut out: Vec<PackageFacts> = pkgs.iter().map(|_| PackageFacts::default()).collect();
    let mut seen = HashSet::new();
    let mut queue = VecDeque::new();
    for entry in entries.iter().filter_map(|e| fs::canonicalize(e).ok()) {
        if seen.insert(entry.clone()) {
            queue.push_back((0usize, entry, false));
        }
    }
    while let Some((p, file, forced_mjs)) = queue.pop_front() {
        let Some(mjs) = language(&pkgs[p], &file, forced_mjs) else {
            continue;
        };
        let Ok(source) = fs::read_to_string(&file) else {
            continue;
        };
        // Node 22+ runs a typeless .js file as ESM when it only parses as ESM; so do we,
        // or a package could hide code in files we never read.
        let parsed = inspect::inspect(&source, mjs).or_else(|e| {
            if mjs {
                Err(e)
            } else {
                inspect::inspect(&source, true)
            }
        });
        let facts = match parsed {
            Ok(f) => f,
            Err(e) => {
                eprintln!("permlock: skipped {}: {e}", file.display());
                continue;
            }
        };
        for spec in &facts.imports {
            if inspect::is_builtin(spec) {
                continue;
            }
            let Some((q, target, forced)) = resolve(pkgs, p, &file, spec) else {
                continue;
            };
            if p != 0 && q != p {
                out[p].packages.insert(q);
            }
            if target.extension().is_some_and(|e| e == "node") && !out[q].native.contains(&target) {
                out[q].native.push(target.clone());
            }
            if seen.insert(target.clone()) {
                queue.push_back((q, target, forced));
            }
        }
        let pkg = &mut out[p];
        pkg.inspected = true;
        // ponytail: later module wins on a read/write clash, like LavaMoat's object spread
        pkg.globals.extend(facts.globals);
        pkg.builtins.extend(facts.builtins);
        pkg.seen
            .extend(facts.seen.into_iter().map(|s| (file.clone(), s)));
    }
    out
}

/// Endo's language choice: extension first, then forced `module`/`import` targets,
/// then the `type` of the nearest package.json inside the package, like Node.
/// `None` for files that are not inspected (json, native).
fn language(pkg: &Package, file: &Path, forced_mjs: bool) -> Option<bool> {
    match file.extension()?.to_str()? {
        "mjs" => Some(true),
        "cjs" => Some(false),
        "js" => Some(forced_mjs || nearest_type(pkg, file) == "module"),
        _ => None,
    }
}

fn nearest_type(pkg: &Package, file: &Path) -> String {
    for dir in file.ancestors().skip(1) {
        if dir == pkg.dir {
            break;
        }
        if let Ok(text) = fs::read_to_string(dir.join("package.json")) {
            let d: Value = serde_json::from_str(&text).unwrap_or_default();
            return d["type"].as_str().unwrap_or("").to_string();
        }
    }
    pkg.descriptor["type"].as_str().unwrap_or("").to_string()
}

fn resolve(pkgs: &[Package], p: usize, from: &Path, spec: &str) -> Option<(usize, PathBuf, bool)> {
    if spec.starts_with("./") || spec.starts_with("../") || spec.starts_with('/') {
        return Some((p, search(&from.parent()?.join(spec))?, false));
    }
    let (name, sub) = split_bare(spec);
    let q = *pkgs[p].deps.get(name)?;
    let (target, forced) = export_target(&pkgs[q].descriptor, &sub).unwrap_or((sub.clone(), false));
    Some((q, search(&pkgs[q].dir.join(&target))?, forced))
}

/// `@scope/name/sub/path` -> (`@scope/name`, `./sub/path`); a bare name maps to `.`.
fn split_bare(spec: &str) -> (&str, String) {
    let cut = if spec.starts_with('@') { 2 } else { 1 };
    match spec.match_indices('/').nth(cut - 1) {
        Some((i, _)) => (&spec[..i], format!(".{}", &spec[i..])),
        None => (spec, ".".to_string()),
    }
}

/// Every public way into a package: `exports` subpaths (or `module`/`main`), and `bin`.
pub fn entries(pkg: &Package) -> Vec<PathBuf> {
    let d = &pkg.descriptor;
    let mut subs = vec![".".to_string()];
    if let Some(map) = d["exports"].as_object() {
        subs.extend(
            map.keys()
                .filter(|k| k.starts_with("./") && !k.contains('*'))
                .cloned(),
        );
    }
    let mut files: Vec<PathBuf> = subs
        .iter()
        .filter_map(|sub| export_target(d, sub))
        .filter_map(|(target, _)| search(&pkg.dir.join(target)))
        .collect();
    let bins: Vec<&str> = match &d["bin"] {
        Value::String(b) => vec![b.as_str()],
        Value::Object(m) => m.values().filter_map(Value::as_str).collect(),
        _ => vec![],
    };
    files.extend(bins.into_iter().filter_map(|b| search(&pkg.dir.join(b))));
    files.dedup();
    files
}

fn search(base: &Path) -> Option<PathBuf> {
    let base = base.to_str()?;
    SUFFIXES
        .iter()
        .map(|s| PathBuf::from(format!("{base}{s}")))
        .find(|p| p.is_file())
        .and_then(|p| fs::canonicalize(p).ok())
}

/// Endo's inferExports: `module` (parsed as ESM) or `main` for `.`, then `exports`
/// entries on top. Unlisted subpaths fall back to plain files.
fn export_target(d: &Value, sub: &str) -> Option<(String, bool)> {
    let mut map: BTreeMap<String, (String, bool)> = BTreeMap::new();
    if let Some(m) = d["module"].as_str() {
        map.insert(".".into(), (m.to_string(), true));
    } else if let Some(m) = d["main"].as_str() {
        map.insert(".".into(), (m.to_string(), false));
    }
    if !d["exports"].is_null() {
        let mut entries = vec![];
        interpret(".", &d["exports"], false, &mut entries);
        map.extend(entries.into_iter().map(|(k, v, f)| (k, (v, f))));
    }
    if let Some(hit) = map.get(sub) {
        return Some(hit.clone());
    }
    // single `*` patterns like "./*": "./dist/*.js"
    map.iter()
        .find_map(|(k, (v, f))| {
            let (pre, post) = k.split_once('*')?;
            let star = sub.strip_prefix(pre)?.strip_suffix(post)?;
            Some((v.replace('*', star), *f))
        })
        .or_else(|| (sub == ".").then(|| ("./index.js".to_string(), false)))
}

fn interpret(name: &str, exports: &Value, import: bool, out: &mut Vec<(String, String, bool)>) {
    match exports {
        Value::String(s) => out.push((name.to_string(), s.clone(), import)),
        Value::Array(sections) => {
            for s in sections {
                let before = out.len();
                interpret(name, s, import, out);
                if out.len() > before {
                    break;
                }
            }
        }
        Value::Object(map) => {
            for (key, value) in map {
                if key == "./" {
                    continue;
                } else if key == "." || key.starts_with("./") {
                    interpret(key, value, import, out);
                } else if CONDITIONS.contains(&key.as_str()) {
                    interpret(name, value, key == "import" && value.is_string(), out);
                    break;
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bare_specifiers_split_like_node() {
        assert_eq!(split_bare("hono"), ("hono", ".".to_string()));
        assert_eq!(split_bare("hono/cors"), ("hono", "./cors".to_string()));
        assert_eq!(
            split_bare("@hono/node-server"),
            ("@hono/node-server", ".".to_string())
        );
        assert_eq!(split_bare("@a/b/c/d"), ("@a/b", "./c/d".to_string()));
    }

    #[test]
    fn exports_pick_the_first_matching_condition_in_file_order() {
        let d = json!({ "main": "./main.js", "exports": {
            ".": { "require": "./r.cjs", "import": "./i.mjs", "default": "./d.js" },
            "./sub/*": "./dist/sub/*.js"
        }});
        assert_eq!(export_target(&d, "."), Some(("./i.mjs".into(), true)));
        assert_eq!(
            export_target(&d, "./sub/x"),
            Some(("./dist/sub/x.js".into(), false))
        );
        let m = json!({ "main": "./main.js", "module": "./esm.js" });
        assert_eq!(export_target(&m, "."), Some(("./esm.js".into(), true)));
        assert_eq!(
            export_target(&json!({}), "."),
            Some(("./index.js".into(), false))
        );
    }
}
