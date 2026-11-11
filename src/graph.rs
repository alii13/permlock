//! The package graph exactly as Endo's compartment-mapper builds it: start at the
//! entry package.json, follow dependency names the way Node walks up node_modules,
//! and name every package by its cheapest path from the entry.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;

pub const ROOT: &str = "$root$";

pub struct Package {
    pub canonical: String,
    pub version: String,
    pub dir: PathBuf,
    pub descriptor: Value,
    /// dependency name -> index into the walk result
    pub deps: BTreeMap<String, usize>,
}

/// Index 0 is the entry package. `dev` adds the entry's devDependencies only.
pub fn walk(entry: &Path, dev: bool) -> io::Result<Vec<Package>> {
    let root = fs::canonicalize(entry)?;
    let mut dirs = vec![root.clone()];
    let mut names = vec![ROOT.to_string()];
    let mut descriptors = vec![read_descriptor(&root)?];
    let mut deps: Vec<BTreeMap<String, usize>> = vec![BTreeMap::new()];
    let mut seen = HashMap::from([(root, 0)]);

    let mut i = 0;
    while i < dirs.len() {
        for name in dependency_names(&descriptors[i], dev && i == 0) {
            let Some(dir) = find_package(&dirs[i], &name) else {
                continue; // ponytail: missing deps are skipped, like Endo with strict off
            };
            let j = match seen.get(&dir) {
                Some(&j) => j,
                None => {
                    descriptors.push(read_descriptor(&dir)?);
                    seen.insert(dir.clone(), dirs.len());
                    dirs.push(dir);
                    // ponytail: first visitor's key names the node; differs from Endo only for npm aliases
                    names.push(name.clone());
                    deps.push(BTreeMap::new());
                    dirs.len() - 1
                }
            };
            deps[i].insert(name, j);
        }
        i += 1;
    }

    let paths = shortest_paths(&dirs, &deps);
    Ok(dirs
        .into_iter()
        .zip(descriptors)
        .zip(deps)
        .enumerate()
        .map(|(i, ((dir, descriptor), deps))| Package {
            canonical: if i == 0 {
                ROOT.to_string()
            } else {
                let path = paths[i]
                    .as_ref()
                    .expect("every walked package is reachable");
                path.nodes
                    .iter()
                    .map(|&n| names[n].as_str())
                    .collect::<Vec<_>>()
                    .join(">")
            },
            version: descriptor["version"].as_str().unwrap_or("").to_string(),
            dir,
            descriptor,
            deps,
        })
        .collect())
}

fn read_descriptor(dir: &Path) -> io::Result<Value> {
    let text = fs::read_to_string(dir.join("package.json"))?;
    serde_json::from_str(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Endo's graphPackage: dependencies, peers, bundled, optionals, optional peer metas,
/// and devDependencies when asked, visited in sorted order.
fn dependency_names(d: &Value, dev: bool) -> BTreeSet<String> {
    let mut fields = vec![
        "dependencies",
        "peerDependencies",
        "bundleDependencies",
        "optionalDependencies",
    ];
    if dev {
        fields.push("devDependencies");
    }
    let mut names: BTreeSet<String> = fields
        .into_iter()
        .filter_map(|f| d[f].as_object())
        .flat_map(|o| o.keys().cloned())
        .collect();
    if let Some(meta) = d["peerDependenciesMeta"].as_object() {
        names.extend(
            meta.iter()
                .filter(|(_, m)| m["optional"] == true)
                .map(|(k, _)| k.clone()),
        );
    }
    names
}

/// Node's lookup: `<dir>/node_modules/<name>`, then each parent, never `node_modules/node_modules`.
fn find_package(from: &Path, name: &str) -> Option<PathBuf> {
    let mut dir = from.to_path_buf();
    loop {
        let candidate = dir.join("node_modules").join(name);
        if candidate.join("package.json").is_file() {
            return fs::canonicalize(candidate).ok();
        }
        if !dir.pop() {
            return None;
        }
        if dir.file_name().is_some_and(|n| n == "node_modules") && !dir.pop() {
            return None;
        }
    }
}

#[derive(Clone)]
struct Route {
    nodes: Vec<usize>,
    len: usize,
}

/// Endo's Dijkstra with @endo/path-compare: fewest hops, then shortest total location
/// string, then the locations compared in order. Locations end in `/` like Endo's.
// ponytail: O(V^2) linear min like Endo. A heap if trees pass ~20k packages.
fn shortest_paths(dirs: &[PathBuf], deps: &[BTreeMap<String, usize>]) -> Vec<Option<Route>> {
    let locs: Vec<String> = dirs.iter().map(|d| format!("{}/", d.display())).collect();
    let cmp = |a: &Route, b: &Route| -> Ordering {
        a.nodes
            .len()
            .cmp(&b.nodes.len())
            .then(a.len.cmp(&b.len))
            .then_with(|| {
                a.nodes
                    .iter()
                    .map(|&n| &locs[n])
                    .cmp(b.nodes.iter().map(|&n| &locs[n]))
            })
    };
    let mut best: Vec<Option<Route>> = vec![None; dirs.len()];
    best[0] = Some(Route {
        nodes: vec![],
        len: 0,
    });
    let mut done = vec![false; dirs.len()];
    while let Some(u) = (0..dirs.len())
        .filter(|&i| !done[i] && best[i].is_some())
        .min_by(|&a, &b| cmp(best[a].as_ref().unwrap(), best[b].as_ref().unwrap()))
    {
        done[u] = true;
        for &v in deps[u].values() {
            let mut route = best[u].clone().unwrap();
            route.nodes.push(v);
            route.len += locs[v].len();
            if best[v].as_ref().is_none_or(|old| cmp(&route, old).is_lt()) {
                best[v] = Some(route);
            }
        }
    }
    best
}
