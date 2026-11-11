use std::process::{Command, Output};

fn permlock(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_permlock"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn version_prints_name_and_version() {
    let out = permlock(&["--version"]);
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        format!("permlock {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn unknown_command_exits_2() {
    let out = permlock(&["frobnicate"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .contains("frobnicate")
    );
}
