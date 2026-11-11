use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use permlock::{analyze, graph, policy, report};
use serde_json::Value;

const LOCK: &str = "permlock.json";
const BADGE: &str = "permlock-badge.json";
const LAVAMOAT_POLICY: &str = "lavamoat/node/policy.json";
const ACTION: &str = "alii13/permlock@v0";
const HOME: &str = "https://github.com/alii13/permlock";

const USAGE: &str = "usage:
  permlock init [dir]                 lock what every dependency can touch, print a summary
  permlock check [dir]                fail when a package asks for more than permlock.json allows
  permlock accept <package> [dir]     record a package's new powers after you review them
  permlock generate [dir]             rewrite permlock.json from scratch
  permlock declare [dir]              write this package's own capabilities and badge
  permlock verify [dir]               fail when this package needs more than it declares
  permlock check --base a.json --head b.json

options:
  --out <dir>      where permlock.json lives (default: the project dir)
  --entry <file>   analyze from this file instead of the package's entry points
  --dev            include the root's devDependencies
  --lavamoat       also write lavamoat/node/policy.json";

// ponytail: std args, no clap. Add clap when commands grow more flags.
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let o = Opts::parse(args.get(1..).unwrap_or(&[]));
    let result = match args.first().map(String::as_str) {
        Some("--version" | "-V") => {
            println!("permlock {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Some("--help" | "-h" | "help") => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Some("init") => init(&o),
        Some("generate") => generate(&o),
        Some("check") => check(&o),
        Some("accept") => accept(&o),
        Some("declare") => declare(&o),
        Some("verify") => verify(&o),
        Some(other) => Err(format!("unknown command '{other}'\n{USAGE}")),
        None => Err(USAGE.to_string()),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("permlock: {e}");
            ExitCode::from(2)
        }
    }
}

#[derive(Default)]
struct Opts {
    positional: Vec<String>,
    entry: Option<PathBuf>,
    out: Option<PathBuf>,
    base: Option<PathBuf>,
    head: Option<PathBuf>,
    dev: bool,
    lavamoat: bool,
}

impl Opts {
    fn parse(args: &[String]) -> Self {
        let mut o = Opts::default();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            let mut value = || it.next().map(PathBuf::from);
            match a.as_str() {
                "--entry" => o.entry = value(),
                "--out" => o.out = value(),
                "--base" => o.base = value(),
                "--head" => o.head = value(),
                "--dev" => o.dev = true,
                "--lavamoat" => o.lavamoat = true,
                _ => o.positional.push(a.clone()),
            }
        }
        o
    }

    /// The project dir is the first positional, or the second for `accept <package> [dir]`.
    fn dir_at(&self, i: usize) -> PathBuf {
        self.positional
            .get(i)
            .map_or_else(|| PathBuf::from("."), PathBuf::from)
    }

    fn lock(&self, dir: &Path) -> PathBuf {
        self.out
            .clone()
            .unwrap_or_else(|| dir.to_path_buf())
            .join(LOCK)
    }
}

struct Analysis {
    pkgs: Vec<graph::Package>,
    facts: Vec<analyze::PackageFacts>,
}

fn analyze_dir(dir: &Path, o: &Opts) -> Result<Analysis, String> {
    if !dir.join("package.json").is_file() {
        return Err(format!("no package.json in {}", dir.display()));
    }
    let pkgs = graph::walk(dir, o.dev).map_err(|e| format!("{}: {e}", dir.display()))?;
    let entries = match &o.entry {
        Some(e) => vec![e.clone()],
        None => analyze::entries(&pkgs[0]),
    };
    let facts = analyze::analyze(&pkgs, &entries);
    Ok(Analysis { pkgs, facts })
}

fn lavamoat_policy(dir: &Path, a: &Analysis) -> Result<String, String> {
    let overrides = fs::read_to_string(dir.join("lavamoat/node/policy-override.json"))
        .ok()
        .map(|s| serde_json::from_str::<Value>(&s).map_err(|e| format!("policy override: {e}")))
        .transpose()?;
    Ok(policy::stringify(&policy::build(
        &a.pkgs,
        &a.facts,
        overrides.as_ref(),
    )))
}

fn write(path: &Path, text: String) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    println!("wrote {}", path.display());
    Ok(())
}

/// Writes permlock.json (and the LavaMoat policy when asked) and returns the report.
fn lock_everything(dir: &Path, o: &Opts) -> Result<Value, String> {
    let a = analyze_dir(dir, o)?;
    let lock = report::build(&a.pkgs, &a.facts);
    write(&o.lock(dir), pretty(&lock))?;
    if o.lavamoat {
        let out = o.out.clone().unwrap_or_else(|| dir.to_path_buf());
        write(&out.join(LAVAMOAT_POLICY), lavamoat_policy(dir, &a)?)?;
    }
    Ok(lock)
}

fn generate(o: &Opts) -> Result<ExitCode, String> {
    let lock = lock_everything(&o.dir_at(0), o)?;
    let n = lock["packages"].as_object().map_or(0, |p| p.len());
    println!("{n} package{} locked", plural(n));
    Ok(ExitCode::SUCCESS)
}

fn init(o: &Opts) -> Result<ExitCode, String> {
    let lock = lock_everything(&o.dir_at(0), o)?;
    let n = lock["packages"].as_object().map_or(0, |p| p.len());
    println!("\n  {n} package{} locked\n", plural(n));
    print!("{}", report::summary(&lock));
    println!(
        "\n  next: commit {LOCK}, then add one step to your workflow after installing dependencies:\n\n    - uses: {ACTION}\n\n✓ nothing fails until a package asks for more."
    );
    Ok(ExitCode::SUCCESS)
}

fn check(o: &Opts) -> Result<ExitCode, String> {
    let (base, head) = match (&o.base, &o.head) {
        (Some(b), Some(h)) => (read_json(b)?, read_json(h)?),
        (None, None) => {
            let dir = o.dir_at(0);
            let baseline = o.lock(&dir);
            if !baseline.is_file() {
                println!(
                    "no {LOCK} at {} yet. Run permlock init and commit it.",
                    baseline.display()
                );
                return Ok(ExitCode::SUCCESS);
            }
            let a = analyze_dir(&dir, o)?;
            (read_json(&baseline)?, report::build(&a.pkgs, &a.facts))
        }
        _ => return Err("--base and --head go together".into()),
    };
    let n = head["packages"].as_object().map_or(0, |p| p.len());
    let outcome = report::check(&base, &head);
    print!("{}", report::render(&outcome, n));
    Ok(if outcome.escalations.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

fn accept(o: &Opts) -> Result<ExitCode, String> {
    let Some(name) = o.positional.first() else {
        return Err("usage: permlock accept <package> [dir]".into());
    };
    let dir = o.dir_at(1);
    let path = o.lock(&dir);
    let mut lock = read_json(&path).map_err(|e| format!("{e}. Run permlock init first."))?;
    let a = analyze_dir(&dir, o)?;
    let fresh = report::build(&a.pkgs, &a.facts);
    let Some(entry) = fresh["packages"].get(name).cloned() else {
        return Err(format!("no package named {name} in {}", dir.display()));
    };
    let packages = lock["packages"]
        .as_object_mut()
        .ok_or("permlock.json has no packages")?;
    packages.insert(name.clone(), entry.clone());
    packages.sort_keys();
    write(&path, pretty(&lock))?;
    let classes = classes_of(&entry);
    let powers = if classes.is_empty() {
        "nothing".to_string()
    } else {
        classes.join(", ")
    };
    println!(
        "accepted {name} {}: {powers}",
        entry["version"].as_str().unwrap_or("")
    );
    Ok(ExitCode::SUCCESS)
}

fn declare(o: &Opts) -> Result<ExitCode, String> {
    let dir = o.dir_at(0);
    let a = analyze_dir(&dir, o)?;
    let classes = classes_of(&Value::Object(report::observed(&a.pkgs[0], &a.facts[0])));
    let manifest_path = dir.join("package.json");
    let mut manifest = read_json(&manifest_path)?;
    manifest["capabilities"] = Value::from(classes.clone());
    write(&manifest_path, pretty(&manifest))?;
    write(&dir.join(BADGE), pretty(&report::badge(&classes)))?;
    let name = manifest["name"].as_str().unwrap_or("this package");
    let needs = if classes.is_empty() {
        "nothing".to_string()
    } else {
        classes.join(", ")
    };
    let repo = github_repo(&manifest).unwrap_or_else(|| "<owner>/<repo>".into());
    let raw = format!("https://raw.githubusercontent.com/{repo}/HEAD/{BADGE}");
    println!(
        "\n  {name} needs {needs}\n\n  badge for your README:\n\n    [![permlock](https://img.shields.io/endpoint?url={})]({HOME})\n\n  keep it true in CI:\n\n    - uses: {ACTION}\n      with:\n        mode: verify",
        raw.replace(':', "%3A").replace('/', "%2F")
    );
    Ok(ExitCode::SUCCESS)
}

fn verify(o: &Opts) -> Result<ExitCode, String> {
    let dir = o.dir_at(0);
    let a = analyze_dir(&dir, o)?;
    let root = &a.pkgs[0];
    let name = root.descriptor["name"].as_str().unwrap_or("this package");
    let Some(declared) = root.descriptor["capabilities"].as_array() else {
        return Err(format!(
            "{name} has no capabilities field in package.json. Run permlock declare."
        ));
    };
    let declared: Vec<&str> = declared.iter().filter_map(Value::as_str).collect();
    let observed = report::observed(root, &a.facts[0]);
    let mut lines = vec![];
    for (class, evidence) in &observed {
        if declared.contains(&class.as_str()) {
            continue;
        }
        for ev in evidence.as_array().into_iter().flatten() {
            let file = ev["file"].as_str().unwrap_or("");
            let at = match ev["line"].as_u64() {
                Some(0) | None => file.to_string(),
                Some(l) => format!("{file}:{l}"),
            };
            lines.push(format!(
                "    + {class:<7} {:<20} {at}",
                ev["via"].as_str().unwrap_or("")
            ));
        }
    }
    for unused in declared.iter().filter(|c| !observed.contains_key(**c)) {
        println!("  declared but not used: {unused}");
    }
    if lines.is_empty() {
        let list = if declared.is_empty() {
            "nothing".to_string()
        } else {
            declared.join(", ")
        };
        println!("✓ {name} uses only what it declares: {list}");
        return Ok(ExitCode::SUCCESS);
    }
    println!(
        "\n  {name}  {}    needs more than package.json declares\n",
        root.version
    );
    println!("{}", lines.join("\n"));
    println!("\n✗ run permlock declare if these are expected, and say why in the pull request.");
    Ok(ExitCode::from(1))
}

fn classes_of(entry: &Value) -> Vec<String> {
    let observed = entry.get("observed").unwrap_or(entry);
    observed
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default()
}

/// `owner/repo` from package.json `repository`, when it points at GitHub.
fn github_repo(d: &Value) -> Option<String> {
    let url = d["repository"]
        .as_str()
        .or_else(|| d["repository"]["url"].as_str())?;
    let url = url.trim_end_matches(".git").trim_end_matches('/');
    let rest = if let Some(r) = url.strip_prefix("github:") {
        r
    } else if let Some(i) = url.find("github.com") {
        url[i + "github.com".len()..].trim_start_matches([':', '/'])
    } else if !url.contains(':') {
        url
    } else {
        return None;
    };
    (rest.split('/').count() == 2).then(|| rest.to_string())
}

fn read_json(path: &Path) -> Result<Value, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).expect("json") + "\n"
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}
