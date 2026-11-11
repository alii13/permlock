//! Every command and every capability class, driven through the real binary.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn e2e(name: &str) -> String {
    root().join("fixtures/e2e").join(name).display().to_string()
}

fn temp(name: &str) -> PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("permlock-feat-{}-{n}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Copy a fixture with symlinks resolved, so the copy works anywhere.
fn copy(from: &str, name: &str) -> PathBuf {
    let dir = temp(name);
    let ok = Command::new("cp")
        .args(["-RL", &format!("{from}/."), dir.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(ok.success());
    dir
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_permlock"))
        .args(args)
        .output()
        .unwrap()
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

#[test]
fn init_summarises_the_tree_and_never_fails() {
    let out = temp("init");
    let o = run(&["init", &e2e("kitchen"), "--out", s(&out)]);
    let t = text(&o);
    assert_eq!(o.status.code(), Some(0), "{t}");
    for want in [
        "touch the network",
        "netpkg",
        "run install scripts",
        "installer",
        "load native code",
        "addon",
        "run child processes",
        "liar",
        "read env",
        "honest",
        "need nothing",
        "uses: alii13/permlock@v0",
    ] {
        assert!(t.contains(want), "missing {want:?} in:\n{t}");
    }
    assert!(out.join("permlock.json").is_file());
}

#[test]
fn every_class_lands_in_the_lock() {
    let out = temp("classes");
    assert!(
        run(&["generate", &e2e("kitchen"), "--out", s(&out)])
            .status
            .success()
    );
    let lock = json(&out.join("permlock.json"));
    let p = &lock["packages"];
    let via = |pkg: &str, class: &str| -> Vec<String> {
        p[pkg]["observed"][class]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["via"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(via("netpkg", "net"), ["fetch"]);
    assert_eq!(via("installer", "install"), ["postinstall"]);
    assert_eq!(via("addon", "native"), ["binding.gyp", "addon.node"]);
    assert_eq!(via("honest", "env"), ["process.env"]);
    assert_eq!(via("liar", "exec"), ["child_process"]);
    assert_eq!(p["pure"]["observed"], serde_json::json!({}));
}

#[test]
fn a_lying_declaration_fails_even_when_nothing_changed() {
    let out = temp("liar");
    assert!(
        run(&["generate", &e2e("kitchen"), "--out", s(&out)])
            .status
            .success()
    );
    let lock = out.join("permlock.json");
    let o = run(&["check", "--base", s(&lock), "--head", s(&lock)]);
    let t = text(&o);
    assert_eq!(o.status.code(), Some(1), "{t}");
    assert!(
        t.contains("liar") && t.contains("not in its declared capabilities"),
        "{t}"
    );
    assert!(!t.contains("honest"), "an honest declaration passes:\n{t}");
}

#[test]
fn accept_records_the_new_powers_and_check_passes_again() {
    let lock = temp("accept");
    assert!(
        run(&["generate", &e2e("before"), "--out", s(&lock)])
            .status
            .success()
    );
    assert_eq!(
        run(&["check", &e2e("after"), "--out", s(&lock)])
            .status
            .code(),
        Some(1)
    );
    let o = run(&["accept", "tiny-slug", &e2e("after"), "--out", s(&lock)]);
    assert!(o.status.success(), "{}", text(&o));
    assert!(
        text(&o).contains("accepted tiny-slug 1.0.1: env, fs, net"),
        "{}",
        text(&o)
    );
    assert_eq!(
        json(&lock.join("permlock.json"))["packages"]["tiny-slug"]["version"],
        "1.0.1"
    );
    assert_eq!(
        run(&["check", &e2e("after"), "--out", s(&lock)])
            .status
            .code(),
        Some(0)
    );
    assert_eq!(
        run(&["accept", "nope", &e2e("after"), "--out", s(&lock)])
            .status
            .code(),
        Some(2)
    );
}

#[test]
fn declare_writes_the_field_and_the_badge() {
    let fixtures = root().join("fixtures/tiny-slug");
    let pure = copy(
        &fixtures.join("1.0.0").display().to_string(),
        "declare-pure",
    );
    let o = run(&["declare", s(&pure)]);
    assert!(o.status.success(), "{}", text(&o));
    assert!(
        text(&o).contains("https://img.shields.io/endpoint?url="),
        "{}",
        text(&o)
    );
    assert_eq!(
        json(&pure.join("package.json"))["capabilities"],
        serde_json::json!([])
    );
    let badge = json(&pure.join("permlock-badge.json"));
    assert_eq!(badge["message"], "needs nothing");
    assert_eq!(badge["schemaVersion"], 1);

    let evil = copy(
        &fixtures.join("1.0.1").display().to_string(),
        "declare-evil",
    );
    assert!(run(&["declare", s(&evil)]).status.success());
    assert_eq!(
        json(&evil.join("package.json"))["capabilities"],
        serde_json::json!(["env", "fs", "net"])
    );
    assert_eq!(
        json(&evil.join("permlock-badge.json"))["message"],
        "needs env, fs, net"
    );
}

#[test]
fn verify_fails_when_the_code_outgrows_the_declaration() {
    let pkg = copy(
        &root()
            .join("fixtures/tiny-slug/1.0.1")
            .display()
            .to_string(),
        "verify",
    );
    assert_eq!(
        run(&["verify", s(&pkg)]).status.code(),
        Some(2),
        "no field yet"
    );
    let manifest = pkg.join("package.json");
    let mut m = json(&manifest);
    m["capabilities"] = serde_json::json!([]);
    std::fs::write(&manifest, serde_json::to_string_pretty(&m).unwrap()).unwrap();
    let o = run(&["verify", s(&pkg)]);
    let t = text(&o);
    assert_eq!(o.status.code(), Some(1), "{t}");
    assert!(
        t.contains("+ env") && t.contains("+ fs") && t.contains("+ net"),
        "{t}"
    );
    assert!(run(&["declare", s(&pkg)]).status.success());
    assert_eq!(run(&["verify", s(&pkg)]).status.code(), Some(0));
}

#[test]
fn the_github_action_script_fails_the_job_and_writes_a_summary() {
    let project = copy(&e2e("after"), "action");
    let base = temp("action-base");
    assert!(
        run(&["generate", &e2e("before"), "--out", s(&base)])
            .status
            .success()
    );
    std::fs::copy(base.join("permlock.json"), project.join("permlock.json")).unwrap();
    let summary = temp("summary").join("summary.md");
    let o = Command::new("bash")
        .arg(root().join("action/run.sh"))
        .env("PERMLOCK_BIN", env!("CARGO_BIN_EXE_permlock"))
        .env("PERMLOCK_MODE", "check")
        .env("PERMLOCK_DIR", s(&project))
        .env("GITHUB_STEP_SUMMARY", &summary)
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    let md = std::fs::read_to_string(&summary).unwrap();
    assert!(md.contains("tiny-slug") && md.contains("+ net"), "{md}");

    let bad = Command::new("bash")
        .arg(root().join("action/run.sh"))
        .env("PERMLOCK_BIN", env!("CARGO_BIN_EXE_permlock"))
        .env("PERMLOCK_MODE", "rm -rf /")
        .env("GITHUB_STEP_SUMMARY", &summary)
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(2), "unknown modes are refused");
}

#[test]
fn the_npm_wrapper_runs_the_platform_binary_and_declares_itself_honestly() {
    let platform = Command::new("node")
        .args(["-p", "process.platform + '-' + process.arch"])
        .output()
        .unwrap();
    let platform = String::from_utf8(platform.stdout)
        .unwrap()
        .trim()
        .to_string();
    let project = temp("npm");
    let modules = project.join("node_modules");
    let wrapper = copy(
        &root().join("npm/permlock").display().to_string(),
        "wrapper",
    );
    std::fs::create_dir_all(&modules).unwrap();
    std::fs::rename(&wrapper, modules.join("permlock")).unwrap();
    let bin_dir = modules.join(format!("permlock-{platform}")).join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_permlock"), bin_dir.join("permlock")).unwrap();
    let o = Command::new("node")
        .arg(modules.join("permlock/bin/permlock.js"))
        .arg("--version")
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&o.stdout).trim(),
        format!("permlock {}", env!("CARGO_PKG_VERSION")),
        "{}",
        text(&o)
    );

    let v = run(&["verify", &root().join("npm/permlock").display().to_string()]);
    assert_eq!(v.status.code(), Some(0), "{}", text(&v));
}

#[test]
fn typescript_vue_aliases_and_lazy_imports_are_all_read() {
    let out = temp("ts-vue");
    let o = run(&["generate", &e2e("ts-vue"), "--out", s(&out)]);
    assert!(o.status.success(), "{}", text(&o));
    assert!(text(&o).contains("read 6 files"), "{}", text(&o));
    let p = json(&out.join("permlock.json"))["packages"].clone();
    let via = |pkg: &str, class: &str| p[pkg]["observed"][class][0]["via"].clone();
    assert_eq!(
        via("fetcher", "net"),
        "fetch",
        "reached through @/ alias and a .ts file"
    );
    assert_eq!(
        via("vue-dep", "net"),
        "WebSocket",
        "reached from a .vue script block"
    );
    assert_eq!(
        via("lazy", "env"),
        "process.env",
        "reached by a dynamic import()"
    );
    assert_eq!(
        p["typedep"]["observed"],
        serde_json::json!({}),
        "type-only imports load no code"
    );

    let app = copy(&e2e("ts-vue"), "ts-vue-declare");
    assert!(run(&["declare", s(&app)]).status.success());
    assert_eq!(
        json(&app.join("package.json"))["capabilities"],
        serde_json::json!([]),
        "a WebSocket type is not a use"
    );
}

#[test]
fn no_entry_point_is_a_loud_warning_not_a_silent_pass() {
    let dir = temp("no-entry");
    std::fs::write(
        dir.join("package.json"),
        r#"{"name":"app","version":"1.0.0"}"#,
    )
    .unwrap();
    let o = run(&["init", s(&dir)]);
    assert!(text(&o).contains("no entry point found"), "{}", text(&o));
}

#[test]
fn the_report_is_the_same_on_every_machine() {
    // npm installs only the optional package that matches the machine, like
    // @esbuild/darwin-arm64 on a Mac and @esbuild/linux-x64 in CI.
    let install = |name: &str, os: &str| {
        let app = copy(&e2e("before"), name);
        let pkg = app.join("node_modules").join(format!("bin-{os}"));
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(
            pkg.join("package.json"),
            format!(r#"{{"name":"bin-{os}","version":"1.0.0","os":["{os}"],"cpu":["x64"]}}"#),
        )
        .unwrap();
        let manifest = app.join("package.json");
        let mut root = json(&manifest);
        root["optionalDependencies"] =
            serde_json::json!({ "bin-darwin": "1.0.0", "bin-linux": "1.0.0" });
        std::fs::write(&manifest, root.to_string()).unwrap();
        let o = run(&["generate", s(&app)]);
        assert_eq!(o.status.code(), Some(0), "{}", text(&o));
        std::fs::read_to_string(app.join("permlock.json")).unwrap()
    };
    assert_eq!(install("mac", "darwin"), install("linux", "linux"));
}
