use std::collections::BTreeMap;
use std::path::Path;

use permlock::graph;

fn names(entry: &Path, dev: bool) -> BTreeMap<String, String> {
    graph::walk(entry, dev)
        .unwrap()
        .into_iter()
        .map(|p| (p.canonical, p.version))
        .collect()
}

fn app() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/graph/app")
}

fn expect(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn names_packages_like_endo() {
    assert_eq!(
        names(&app(), false),
        expect(&[
            ("$root$", "1.0.0"),
            ("aaaa", "1.0.0"),
            ("linked", "1.0.0"),
            // pnpm layout: resolved from the symlink's real location
            ("linked>sibling", "1.0.0"),
            ("nested", "1.0.0"),
            // nested copy wins over the hoisted inner@1.0.0
            ("nested>inner", "2.0.0"),
            ("zz", "1.0.0"),
            // equal hops, so the shorter location string wins over the alphabetical one
            ("zz>shared", "1.0.0"),
        ])
    );
}

#[test]
fn dev_dependencies_only_count_when_asked() {
    let dev = names(&app(), true);
    assert_eq!(dev.get("shared").map(String::as_str), Some("1.0.0"));
    assert!(!dev.contains_key("zz>shared"));
}

/// Every name in LavaMoat's recall policy must come out of our walk unchanged.
/// Run with PERMS_RECALL_API=<recall-spike>/packages/api cargo test -- --ignored
#[test]
#[ignore = "needs an installed recall api tree in PERMS_RECALL_API"]
fn recall_names_match_lavamoat() {
    let dir = std::env::var("PERMS_RECALL_API").expect("PERMS_RECALL_API");
    let ours = names(Path::new(&dir), false);
    let policy: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/recall-api/lavamoat-policy.json")).unwrap();
    let resources = policy["resources"].as_object().unwrap();
    let mut missing = vec![];
    for (name, resource) in resources {
        let linked = resource["packages"]
            .as_object()
            .into_iter()
            .flat_map(|p| p.keys());
        for n in std::iter::once(name).chain(linked) {
            if !ours.contains_key(n) {
                missing.push(n.clone());
            }
        }
    }
    assert!(missing.is_empty(), "not found by our walk: {missing:?}");
}
