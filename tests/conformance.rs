//! Each case in spec/conformance: the app in fixtures/e2e/<case> must produce
//! spec/conformance/<case>/expected.json exactly. A second implementation passes
//! the same cases.

use std::path::Path;
use std::process::Command;

#[test]
fn every_conformance_case_matches() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let cases = std::fs::read_dir(root.join("spec/conformance")).unwrap();
    let mut checked = 0;
    for case in cases {
        let case = case.unwrap().path();
        let name = case.file_name().unwrap().to_str().unwrap();
        let out = std::env::temp_dir().join(format!("permlock-conf-{}-{name}", std::process::id()));
        let app = root.join("fixtures/e2e").join(name);
        let run = Command::new(env!("CARGO_BIN_EXE_permlock"))
            .args([
                "generate",
                app.to_str().unwrap(),
                "--out",
                out.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(
            run.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&run.stderr)
        );
        let ours = std::fs::read_to_string(out.join("permlock.json")).unwrap();
        let expected = std::fs::read_to_string(case.join("expected.json")).unwrap();
        assert_eq!(ours, expected, "conformance case {name}");
        checked += 1;
    }
    assert!(checked >= 4, "found {checked} cases");
}
