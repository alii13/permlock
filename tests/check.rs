use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn permlock(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_permlock"))
        .args(args)
        .output()
        .unwrap()
}

fn fixture(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/e2e")
        .join(name)
        .display()
        .to_string()
}

fn out_dir(name: &str) -> PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("permlock-test-{}-{n}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn generate(app: &str) -> PathBuf {
    let out = out_dir(app);
    let run = permlock(&[
        "generate",
        &fixture(app),
        "--out",
        out.to_str().unwrap(),
        "--lavamoat",
    ]);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    out
}

fn stdout(o: &Output) -> String {
    String::from_utf8(o.stdout.clone()).unwrap()
}

#[test]
fn malicious_update_is_flagged_with_file_and_line() {
    let before = generate("before");
    let after = generate("after");
    let run = permlock(&[
        "check",
        "--base",
        before.join("permlock.json").to_str().unwrap(),
        "--head",
        after.join("permlock.json").to_str().unwrap(),
    ]);
    let text = stdout(&run);
    assert_eq!(run.status.code(), Some(1), "{text}");
    assert!(text.contains("tiny-slug  1.0.0 → 1.0.1"), "{text}");
    for line in [
        "+ env",
        "process.env",
        "+ fs",
        "node:fs",
        "index.js:13",
        "+ net",
        "fetch",
    ] {
        assert!(text.contains(line), "missing {line:?} in:\n{text}");
    }
    assert!(text.contains("1 package asked for new powers"), "{text}");
}

#[test]
fn same_version_is_clean() {
    let a = generate("before");
    let report = a.join("permlock.json");
    let run = permlock(&[
        "check",
        "--base",
        report.to_str().unwrap(),
        "--head",
        report.to_str().unwrap(),
    ]);
    assert_eq!(run.status.code(), Some(0), "{}", stdout(&run));
    assert!(stdout(&run).contains("no new powers"));
}

#[test]
fn report_lists_what_each_package_touches() {
    let after = generate("after");
    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(after.join("permlock.json")).unwrap())
            .unwrap();
    let slug = &report["packages"]["tiny-slug"];
    assert_eq!(slug["version"], "1.0.1");
    let classes: Vec<&str> = slug["observed"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(classes, ["env", "fs", "net"]);
    assert_eq!(slug["observed"]["fs"][0]["line"], 13);
    assert!(after.join("lavamoat/node/policy.json").is_file());
}

#[test]
fn typeless_esm_files_are_still_read() {
    let out = generate("esm-detect");
    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("permlock.json")).unwrap()).unwrap();
    assert_eq!(
        report["packages"]["sneaky"]["observed"]["net"][0]["via"],
        "fetch"
    );
}
