use std::path::Path;

use permlock::{analyze, graph, policy};

/// Byte-for-byte against LavaMoat 2.0.1's policy for the recall api.
/// Run with PERMS_RECALL_API=<recall-spike>/packages/api cargo test -- --ignored
#[test]
#[ignore = "needs an installed recall api tree in PERMS_RECALL_API"]
fn recall_policy_is_byte_identical() {
    let dir = std::env::var("PERMS_RECALL_API").expect("PERMS_RECALL_API");
    let dir = Path::new(&dir);
    let pkgs = graph::walk(dir, false).unwrap();
    let facts = analyze::analyze(&pkgs, &[dir.join("dist/index.js")]);
    let overrides = std::fs::read_to_string(dir.join("lavamoat/node/policy-override.json"))
        .ok()
        .map(|s| serde_json::from_str(&s).unwrap());
    let ours = policy::stringify(&policy::build(&pkgs, &facts, overrides.as_ref()));
    let theirs = include_str!("../fixtures/recall-api/lavamoat-policy.json");
    if ours != theirs {
        let only = |a: &str, b: &str| -> Vec<String> {
            let b: Vec<&str> = b.lines().collect();
            a.lines()
                .filter(|l| !b.contains(l))
                .map(str::to_string)
                .collect()
        };
        panic!(
            "policy differs\nonly ours:\n{}\nonly LavaMoat:\n{}",
            only(&ours, theirs).join("\n"),
            only(theirs, &ours).join("\n")
        );
    }
}
