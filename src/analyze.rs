//! Walk modules from the entry files and fold what each one does into one set of
//! facts per package. Two passes: the first reads exactly what Endo loads for a
//! LavaMoat policy, plain JavaScript reached by static imports, and feeds both the
//! policy and the report. The second reads everything else a real app runs,
//! TypeScript, Vue components, tsconfig aliases and lazy `import()` targets, and
//! feeds only the report, so the policy stays byte-identical to LavaMoat's.

use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::graph::Package;
use crate::inspect::{self, Facts, Kind, Seen, Use};

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
    /// source files read for this package
    pub files: usize,
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
/// What TypeScript and bundlers also try, only in the second pass.
const LOOSE_SUFFIXES: &[&str] = &[
    ".ts",
    ".tsx",
    ".mts",
    ".cts",
    ".jsx",
    ".vue",
    "/index.ts",
    "/index.tsx",
    "/index.jsx",
];

enum Lang {
    Js(bool),
    Source(Kind),
    Vue,
}

type Item = (usize, PathBuf, bool);

/// Index 0 is the root package; its facts are kept for `declare` and `verify`,
/// and the policy and report leave it out.
pub fn analyze(pkgs: &[Package], entries: &[PathBuf]) -> Vec<PackageFacts> {
    let aliases = tsconfig_aliases(&pkgs[0].dir);
    let mut out: Vec<PackageFacts> = pkgs.iter().map(|_| PackageFacts::default()).collect();
    let mut q = Queues::default();
    for entry in entries.iter().filter_map(|e| fs::canonicalize(e).ok()) {
        q.add((0, entry, false), true);
    }
    loop {
        let (policy, (p, file, forced_mjs)) = match q.strict.pop_front() {
            Some(item) => (true, item),
            None => match q.loose.pop_front() {
                Some(item) if !q.seen_strict.contains(&item.1) => (false, item),
                Some(_) => continue,
                None => break,
            },
        };
        let Some(lang) = language(&pkgs[p], &file, forced_mjs) else {
            continue;
        };
        let Ok(source) = fs::read_to_string(&file) else {
            continue;
        };
        let facts = match parse(&source, lang) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("permlock: skipped {}: {e}", file.display());
                continue;
            }
        };
        let follow = facts
            .imports
            .iter()
            .map(|s| (s, policy))
            .chain(facts.dynamic.iter().map(|s| (s, false)));
        for (spec, endo) in follow {
            if inspect::is_builtin(spec) {
                continue;
            }
            let (to, target, forced, exact) = match resolve(pkgs, p, &file, spec) {
                Some((to, t, f)) => (to, t, f, true),
                None => match resolve_loose(p, &file, spec, &aliases) {
                    Some((to, t)) => (to, t, false, false),
                    None => continue,
                },
            };
            let endo = endo && exact;
            if endo && p != 0 && to != p {
                out[p].packages.insert(to);
            }
            if target.extension().is_some_and(|e| e == "node") && !out[to].native.contains(&target)
            {
                out[to].native.push(target.clone());
            }
            q.add((to, target, forced), endo);
        }
        let pkg = &mut out[p];
        pkg.inspected = true;
        pkg.files += 1;
        if policy {
            // ponytail: later module wins on a read/write clash, like LavaMoat's object spread
            pkg.globals.extend(facts.globals);
            pkg.builtins.extend(facts.builtins);
        }
        pkg.seen
            .extend(facts.seen.into_iter().map(|s| (file.clone(), s)));
    }
    out
}

#[derive(Default)]
struct Queues {
    /// files Endo loads, in the order it would load them
    strict: VecDeque<Item>,
    /// everything else, read after `strict` is empty
    loose: VecDeque<Item>,
    seen_strict: HashSet<PathBuf>,
    seen_any: HashSet<PathBuf>,
}

impl Queues {
    fn add(&mut self, item: Item, endo: bool) {
        if endo && is_endo_source(&item.1) {
            if self.seen_strict.insert(item.1.clone()) {
                self.seen_any.insert(item.1.clone());
                self.strict.push_back(item);
            }
        } else if !self.seen_strict.contains(&item.1) && self.seen_any.insert(item.1.clone()) {
            self.loose.push_back(item);
        }
    }
}

fn is_endo_source(file: &Path) -> bool {
    file.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e, "js" | "mjs" | "cjs" | "json" | "node"))
}

fn parse(source: &str, lang: Lang) -> Result<Facts, String> {
    match lang {
        // Node 22+ runs a typeless .js file as ESM when it only parses as ESM; so do we,
        // or a package could hide code in files we never read.
        Lang::Js(mjs) => inspect::inspect(source, mjs).or_else(|e| {
            if mjs {
                Err(e)
            } else {
                inspect::inspect(source, true)
            }
        }),
        Lang::Source(kind) => inspect::inspect_as(source, kind),
        Lang::Vue => {
            let mut all = Facts::default();
            for (offset, kind, body) in inspect::vue_scripts(source) {
                let f = inspect::inspect_as(body, kind)?;
                all.globals.extend(f.globals);
                all.builtins.extend(f.builtins);
                all.imports.extend(f.imports);
                all.dynamic.extend(f.dynamic);
                all.seen.extend(f.seen.into_iter().map(|s| Seen {
                    line: s.line + offset,
                    ..s
                }));
            }
            Ok(all)
        }
    }
}

/// Endo's language choice for JavaScript: extension first, then forced `module`/`import`
/// targets, then the `type` of the nearest package.json inside the package, like Node.
/// TypeScript and Vue files are read in the second pass. `None` for files that are
/// not inspected (json, native, type declarations).
fn language(pkg: &Package, file: &Path, forced_mjs: bool) -> Option<Lang> {
    let name = file.file_name()?.to_str()?;
    if name.ends_with(".d.ts") || name.ends_with(".d.mts") || name.ends_with(".d.cts") {
        return None;
    }
    match file.extension()?.to_str()? {
        "mjs" => Some(Lang::Js(true)),
        "cjs" => Some(Lang::Js(false)),
        "js" => Some(Lang::Js(forced_mjs || nearest_type(pkg, file) == "module")),
        "ts" | "mts" | "cts" => Some(Lang::Source(Kind::TypeScript)),
        "tsx" => Some(Lang::Source(Kind::Tsx)),
        "jsx" => Some(Lang::Source(Kind::Jsx)),
        "vue" => Some(Lang::Vue),
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

/// What a bundler resolves and Endo does not: TypeScript extensions, `./x.js`
/// meaning `./x.ts`, and the root's tsconfig `paths` aliases.
fn resolve_loose(
    p: usize,
    from: &Path,
    spec: &str,
    aliases: &[(String, Vec<PathBuf>)],
) -> Option<(usize, PathBuf)> {
    if spec.starts_with("./") || spec.starts_with("../") || spec.starts_with('/') {
        return Some((p, search_loose(&from.parent()?.join(spec))?));
    }
    if p != 0 {
        return None;
    }
    aliases.iter().find_map(|(pattern, targets)| {
        let star = match pattern.split_once('*') {
            Some((pre, post)) => spec.strip_prefix(pre)?.strip_suffix(post)?,
            None if pattern == spec => "",
            None => return None,
        };
        targets.iter().find_map(|t| {
            let t = t.to_str()?.replace('*', star);
            Some((0, search_loose(Path::new(&t))?))
        })
    })
}

fn search_loose(base: &Path) -> Option<PathBuf> {
    if let Some(found) = search(base) {
        return Some(found);
    }
    let text = base.to_str()?;
    let swapped = [
        (".js", ".ts"),
        (".js", ".tsx"),
        (".mjs", ".mts"),
        (".cjs", ".cts"),
        (".jsx", ".tsx"),
    ]
    .iter()
    .filter_map(|(from, to)| Some(format!("{}{to}", text.strip_suffix(from)?)));
    LOOSE_SUFFIXES
        .iter()
        .map(|s| format!("{text}{s}"))
        .chain(swapped)
        .map(PathBuf::from)
        .find(|p| p.is_file())
        .and_then(|p| fs::canonicalize(p).ok())
}

/// `compilerOptions.paths` from the root's tsconfig.json and the configs it
/// references, as (pattern, absolute targets).
fn tsconfig_aliases(root: &Path) -> Vec<(String, Vec<PathBuf>)> {
    let mut out = vec![];
    let mut configs = vec![root.join("tsconfig.json")];
    let mut i = 0;
    while i < configs.len() && i < 16 {
        let config = configs[i].clone();
        i += 1;
        let Some(v) = fs::read_to_string(&config)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&strip_jsonc(&t)).ok())
        else {
            continue;
        };
        let dir = config.parent().unwrap_or(root).to_path_buf();
        for r in v["references"].as_array().into_iter().flatten() {
            if let Some(path) = r["path"].as_str() {
                let target = dir.join(path);
                configs.push(if target.is_dir() {
                    target.join("tsconfig.json")
                } else {
                    target
                });
            }
        }
        let options = &v["compilerOptions"];
        let base = options["baseUrl"]
            .as_str()
            .map_or(dir.clone(), |b| dir.join(b));
        for (pattern, targets) in options["paths"].as_object().into_iter().flatten() {
            let targets = targets
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str);
            out.push((pattern.clone(), targets.map(|t| base.join(t)).collect()));
        }
    }
    out
}

/// tsconfig files allow comments and trailing commas; JSON does not.
fn strip_jsonc(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let (mut i, mut in_string) = (0, false);
    while i < chars.len() {
        let c = chars[i];
        if in_string {
            out.push(c);
            if c == '\\' && i + 1 < chars.len() {
                out.push(chars[i + 1]);
                i += 1;
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
            out.push(c);
        } else if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2;
            continue;
        } else if c == ',' {
            let next = chars[i + 1..].iter().find(|c| !c.is_whitespace());
            if !matches!(next, Some('}' | ']')) {
                out.push(c);
            }
        } else {
            out.push(c);
        }
        i += 1;
    }
    out
}

/// `<script type="module" src="...">` entries in the package's index.html.
fn html_entries(dir: &Path) -> Vec<PathBuf> {
    let Ok(html) = fs::read_to_string(dir.join("index.html")) else {
        return vec![];
    };
    html.split("<script")
        .skip(1)
        .filter_map(|tag| {
            let attrs = &tag[..tag.find('>')?];
            if !attrs.contains("type=\"module\"") && !attrs.contains("type='module'") {
                return None;
            }
            let start = attrs.find("src=")? + 5;
            let src = &attrs[start..start + attrs[start..].find(['"', '\''])?];
            if src.contains("://") {
                return None;
            }
            fs::canonicalize(dir.join(src.trim_start_matches('/'))).ok()
        })
        .collect()
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
    files.extend(html_entries(&pkg.dir));
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
    fn tsconfig_comments_and_trailing_commas_parse() {
        let text = "{ // c\n \"a\": [\"x//y\", 1,], /* d */ \"b\": { \"c\": \"/*\", }, }";
        let v: Value = serde_json::from_str(&strip_jsonc(text)).unwrap();
        assert_eq!(v, json!({ "a": ["x//y", 1], "b": { "c": "/*" } }));
    }

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
