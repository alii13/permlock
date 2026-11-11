//! One module in, the facts LavaMoat would record out: globals (lavamoat-tofu's
//! inspectGlobals), builtins (inspectEsmImports + inspectRequires), the specifiers
//! Endo would follow, and raw observations with line numbers for the report.

use std::collections::{BTreeMap, BTreeSet};

use oxc::allocator::Allocator;
use oxc::ast::AstKind;
use oxc::ast::ast::{
    Argument, BindingPattern, Expression, ImportDeclarationSpecifier, ModuleExportName, PropertyKey,
};
use oxc::parser::{ParseOptions, Parser};
use oxc::semantic::{AstNodes, NodeId, Scoping, SemanticBuilder};
use oxc::span::{GetSpan, SourceType, Span};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Use {
    Read,
    Write,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Seen {
    /// a global path like `process.env.FOO`, or a builtin module like `node:fs`
    pub key: String,
    pub builtin: bool,
    pub line: u32,
}

#[derive(Debug, Default)]
pub struct Facts {
    /// LavaMoat's per-module globals policy, already reduced
    pub globals: BTreeMap<String, Use>,
    pub builtins: BTreeSet<String>,
    /// static specifiers Endo follows
    pub imports: Vec<String>,
    pub seen: Vec<Seen>,
}

/// Node 26.12 `require('module').builtinModules`; `node:` forms added at lookup.
pub const NODE_BUILTINS: &[&str] = &[
    "_http_agent",
    "_http_client",
    "_http_common",
    "_http_incoming",
    "_http_outgoing",
    "_http_server",
    "_tls_common",
    "_tls_wrap",
    "assert",
    "assert/strict",
    "async_hooks",
    "buffer",
    "child_process",
    "cluster",
    "console",
    "constants",
    "crypto",
    "dgram",
    "diagnostics_channel",
    "dns",
    "dns/promises",
    "domain",
    "events",
    "fs",
    "fs/promises",
    "http",
    "http2",
    "https",
    "inspector",
    "inspector/promises",
    "module",
    "net",
    "os",
    "path",
    "path/posix",
    "path/win32",
    "perf_hooks",
    "process",
    "punycode",
    "querystring",
    "readline",
    "readline/promises",
    "repl",
    "stream",
    "stream/consumers",
    "stream/promises",
    "stream/web",
    "string_decoder",
    "sys",
    "timers",
    "timers/promises",
    "tls",
    "trace_events",
    "tty",
    "url",
    "util",
    "util/types",
    "v8",
    "vm",
    "wasi",
    "worker_threads",
    "zlib",
    "node:ffi",
    "node:sea",
    "node:sqlite",
    "node:test",
    "node:test/reporters",
];

pub fn is_builtin(spec: &str) -> bool {
    NODE_BUILTINS.contains(&spec)
        || spec
            .strip_prefix("node:")
            .is_some_and(|s| NODE_BUILTINS.contains(&s))
}

/// lavamoat-core GLOBAL_THIS_REFS: stripped from the front of a path.
const GLOBAL_THIS_REFS: &[&str] = &[
    "window",
    "self",
    "global",
    "globalThis",
    "top",
    "frames",
    "parent",
];
/// lavamoat-tofu primordials: globalPropertyNames + languageRefs.
const PRIMORDIALS: &[&str] = &[
    "eval",
    "isFinite",
    "isNaN",
    "parseFloat",
    "parseInt",
    "decodeURI",
    "decodeURIComponent",
    "encodeURI",
    "encodeURIComponent",
    "Array",
    "ArrayBuffer",
    "Boolean",
    "DataView",
    "Date",
    "Error",
    "EvalError",
    "Function",
    "Int8Array",
    "Int16Array",
    "Int32Array",
    "Map",
    "Number",
    "Object",
    "Promise",
    "Proxy",
    "RangeError",
    "ReferenceError",
    "RegExp",
    "Set",
    "SharedArrayBuffer",
    "String",
    "Symbol",
    "SyntaxError",
    "TypeError",
    "Uint8Array",
    "Uint8ClampedArray",
    "Uint16Array",
    "Uint32Array",
    "URIError",
    "WeakMap",
    "WeakSet",
    "Atomics",
    "JSON",
    "Math",
    "Reflect",
    "escape",
    "unescape",
    "BigInt",
    "BigInt64Array",
    "BigUint64Array",
    "NaN",
    "Infinity",
    "this",
    "arguments",
    "undefined",
];
/// Object.prototype own property names, ignored by @lavamoat/node.
const OBJECT_PROTO: &[&str] = &[
    "constructor",
    "__defineGetter__",
    "__defineSetter__",
    "hasOwnProperty",
    "__lookupGetter__",
    "__lookupSetter__",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toString",
    "valueOf",
    "__proto__",
    "toLocaleString",
];

pub fn inspect(source: &str, mjs: bool) -> Result<Facts, String> {
    let allocator = Allocator::default();
    let source_type = if mjs {
        SourceType::mjs()
    } else {
        SourceType::cjs()
    };
    let options = ParseOptions {
        allow_return_outside_function: true,
        preserve_parens: false,
        ..ParseOptions::default()
    };
    let parsed = Parser::new(&allocator, source, source_type)
        .with_options(options)
        .parse();
    if parsed.fatal_error || parsed.diagnostics.has_errors() {
        return Err(parsed
            .diagnostics
            .errors()
            .next()
            .map_or("parse failed".into(), |e| e.to_string()));
    }
    let semantic = SemanticBuilder::new()
        .with_build_nodes(true)
        .build(&parsed.program)
        .semantic;
    let nodes = semantic.nodes();
    let scoping = semantic.scoping();
    let lines = LineIndex::new(source);
    let module_refs: &[&str] = if mjs {
        &["arguments", "import", "export"]
    } else {
        &["arguments", "require", "module", "exports"]
    };
    let ignored = |name: &str| module_refs.contains(&name) || OBJECT_PROTO.contains(&name);

    let mut facts = Facts::default();
    let mut usage: Vec<(String, Use)> = vec![];

    let mut refs: Vec<(String, NodeId)> = vec![];
    for (name, ids) in scoping.root_unresolved_references() {
        for &id in ids {
            let node = scoping.get_reference(id).node_id();
            if !babel_skips(nodes, node) {
                refs.push((name.to_string(), node));
            }
        }
    }
    // Babel reports `ns` in `export * as ns from "x"` as a free identifier.
    for node in nodes.iter() {
        if let AstKind::ExportAllDeclaration(e) = node.kind()
            && let Some(ModuleExportName::IdentifierName(n)) = &e.exported
            && scoping.get_root_binding(n.name.as_str().into()).is_none()
        {
            refs.push((n.name.to_string(), node.id()));
        }
    }

    for (name, node) in refs {
        if ignored(&name) {
            continue;
        }
        let chain = member_chain(nodes, node);
        let use_ = match chain.parent {
            Some(AstKind::AssignmentExpression(a)) if a.left.span() == chain.topmost => Use::Write,
            _ => Use::Read,
        };
        let mut key_path = vec![name];
        key_path.extend(chain.props);
        if GLOBAL_THIS_REFS.contains(&key_path[0].as_str()) {
            key_path.remove(0);
        }
        let paths = match chain.parent {
            Some(AstKind::VariableDeclarator(d)) if is_pattern(&d.id) => pattern_keys(&d.id)
                .into_iter()
                .map(|p| [key_path.clone(), p].concat())
                .collect(),
            _ => vec![key_path],
        };
        let line = lines.line(nodes.kind(node).span().start);
        for path in paths {
            let Some(top) = path.first() else { continue };
            if ["this", "arguments", "undefined"].contains(&top.as_str()) || ignored(top) {
                continue;
            }
            let key = path.join(".");
            facts.seen.push(Seen {
                key: key.clone(),
                builtin: false,
                line,
            });
            if PRIMORDIALS.contains(&top.as_str()) {
                continue;
            }
            // tofu addGlobalUsage: a write always lands, a read only if new
            match usage.iter_mut().find(|(k, _)| *k == key) {
                Some(entry) if use_ == Use::Write => entry.1 = Use::Write,
                Some(_) => {}
                None => usage.push((key, use_)),
            }
        }
    }
    facts.globals = globals_policy(reduce_to_topmost(usage));

    let mut builtins = BTreeSet::new();
    for node in nodes.iter() {
        match node.kind() {
            AstKind::ImportDeclaration(d) => {
                let src = d.source.value.as_str().unwrap_or_default();
                facts.imports.push(src.to_string());
                if !is_builtin(src) {
                    continue;
                }
                facts.seen.push(seen_builtin(src, &lines, d.span));
                let specs: &[ImportDeclarationSpecifier] =
                    d.specifiers.as_ref().map_or(&[], |s| s.as_slice());
                let whole = specs.is_empty()
                    || specs.iter().any(|s| {
                        matches!(s, ImportDeclarationSpecifier::ImportNamespaceSpecifier(_))
                    })
                    || specs.iter().all(|s| {
                        matches!(s, ImportDeclarationSpecifier::ImportDefaultSpecifier(_))
                    });
                if whole {
                    builtins.insert(src.to_string());
                    continue;
                }
                for s in specs {
                    match s {
                        ImportDeclarationSpecifier::ImportSpecifier(s) => {
                            builtins.insert(format!("{src}.{}", export_name(&s.imported)));
                        }
                        _ => {
                            builtins.insert(src.to_string());
                        }
                    }
                }
            }
            AstKind::ExportFromDeclaration(d) => {
                let src = d.source.value.as_str().unwrap_or_default();
                facts.imports.push(src.to_string());
                if !is_builtin(src) {
                    continue;
                }
                facts.seen.push(seen_builtin(src, &lines, d.span));
                if d.specifiers.is_empty() {
                    builtins.insert(src.to_string());
                }
                for s in &d.specifiers {
                    match export_name(&s.local) {
                        "default" => builtins.insert(src.to_string()),
                        name => builtins.insert(format!("{src}.{name}")),
                    };
                }
            }
            AstKind::ExportAllDeclaration(d) => {
                let src = d.source.value.as_str().unwrap_or_default();
                facts.imports.push(src.to_string());
                if is_builtin(src) {
                    facts.seen.push(seen_builtin(src, &lines, d.span));
                    builtins.insert(src.to_string());
                }
            }
            AstKind::ImportExpression(e) => {
                // report only: LavaMoat's builtin inspection does not see dynamic import()
                if let Expression::StringLiteral(s) = &e.source
                    && is_builtin(s.value.as_str().unwrap_or_default())
                {
                    facts.seen.push(seen_builtin(
                        s.value.as_str().unwrap_or_default(),
                        &lines,
                        e.span,
                    ));
                }
            }
            AstKind::CallExpression(call) => {
                let Some(src) = require_arg(call, scoping) else {
                    continue;
                };
                if !mjs {
                    facts.imports.push(src.to_string());
                }
                if !is_builtin(src) {
                    continue;
                }
                facts.seen.push(seen_builtin(src, &lines, call.span));
                for path in require_paths(nodes, scoping, node.id(), src) {
                    builtins.insert(path);
                }
            }
            _ => {}
        }
    }
    facts.builtins = reduce_strings(builtins);
    Ok(facts)
}

fn seen_builtin(src: &str, lines: &LineIndex, span: Span) -> Seen {
    Seen {
        key: src.to_string(),
        builtin: true,
        line: lines.line(span.start),
    }
}

fn export_name<'a>(n: &'a ModuleExportName<'a>) -> &'a str {
    match n {
        ModuleExportName::IdentifierName(i) => i.name.as_str(),
        ModuleExportName::IdentifierReference(i) => i.name.as_str(),
        ModuleExportName::StringLiteral(s) => s.value.as_str().unwrap_or_default(),
    }
}

/// `require("x")` with an unbound `require` and a string literal argument.
fn require_arg<'a>(
    call: &'a oxc::ast::ast::CallExpression<'a>,
    scoping: &Scoping,
) -> Option<&'a str> {
    let Expression::Identifier(callee) = &call.callee else {
        return None;
    };
    if callee.name != "require" {
        return None;
    }
    let id = callee.reference_id.get()?;
    if scoping.get_reference(id).symbol_id().is_some() {
        return None;
    }
    match call.arguments.first()? {
        Argument::StringLiteral(s) => Some(s.value.as_str().unwrap_or_default()),
        _ => None,
    }
}

/// inspectRequires: the member chain on the call, or every use of the variables it is bound to.
fn require_paths(nodes: &AstNodes, scoping: &Scoping, call: NodeId, src: &str) -> Vec<String> {
    let chain = member_chain(nodes, call);
    let key_path: Vec<String> = std::iter::once(src.to_string())
        .chain(chain.props)
        .collect();
    let Some(AstKind::VariableDeclarator(d)) = chain.parent else {
        return vec![key_path.join(".")];
    };
    let mut out = vec![];
    for (binding, path) in pattern_declarations(&d.id, key_path) {
        let refs: Vec<_> = binding
            .and_then(|b| b.symbol_id.get())
            .map(|s| {
                scoping
                    .get_resolved_reference_ids(s)
                    .iter()
                    .filter(|&&r| scoping.get_reference(r).is_read())
                    .map(|&r| scoping.get_reference(r).node_id())
                    .collect()
            })
            .unwrap_or_default();
        if refs.is_empty() {
            out.push(path.join("."));
        }
        for r in refs {
            let sub = member_chain(nodes, r).props;
            out.push([path.clone(), sub].concat().join("."));
        }
    }
    out
}

struct Chain<'a> {
    props: Vec<String>,
    topmost: Span,
    parent: Option<AstKind<'a>>,
}

/// tofu getMemberExpressionNesting: the unbroken non-computed member chain above `node`,
/// and the parent of the member closest to `node` (tofu's "topmost"), which for chains
/// of two or more is the next member up. Parens and oxc's ChainExpression are see-through.
fn member_chain<'a>(nodes: &AstNodes<'a>, node: NodeId) -> Chain<'a> {
    let mut up = nodes
        .ancestors(node)
        .map(|n| n.kind())
        .filter(|k| {
            !matches!(
                k,
                AstKind::ChainExpression(_) | AstKind::ParenthesizedExpression(_)
            )
        })
        .peekable();
    let mut props = vec![];
    let mut topmost = nodes.kind(node).span();
    let mut parent = None;
    loop {
        let prop = match up.peek() {
            Some(AstKind::StaticMemberExpression(m)) => Some((m.property.name.to_string(), m.span)),
            Some(AstKind::PrivateFieldExpression(m)) => {
                Some((format!("#{}", m.field.name), m.span))
            }
            _ => None,
        };
        let Some((name, span)) = prop else { break };
        up.next();
        if props.is_empty() {
            topmost = span;
            parent = up.peek().copied();
        }
        props.push(name);
    }
    if props.is_empty() {
        parent = up.next();
    }
    Chain {
        props,
        topmost,
        parent,
    }
}

/// References Babel's findGlobals never visits as identifiers.
fn babel_skips(nodes: &AstNodes, node: NodeId) -> bool {
    let span = nodes.kind(node).span();
    let parent = nodes.ancestors(node).map(|n| n.kind()).find(|k| {
        !matches!(
            k,
            AstKind::ChainExpression(_) | AstKind::ParenthesizedExpression(_)
        )
    });
    let is_key = |k: &PropertyKey| k.span() == span;
    match parent {
        Some(AstKind::ComputedMemberExpression(m)) => m.expression.span() == span,
        Some(AstKind::ObjectProperty(p)) => is_key(&p.key),
        Some(AstKind::BindingProperty(p)) => is_key(&p.key),
        Some(AstKind::AssignmentTargetPropertyProperty(p)) => p.name.span() == span,
        Some(AstKind::MethodDefinition(m)) => is_key(&m.key),
        Some(AstKind::PropertyDefinition(p)) => is_key(&p.key),
        Some(AstKind::AccessorProperty(p)) => is_key(&p.key),
        Some(
            AstKind::ExportSpecifier(_)
            | AstKind::AssignmentTargetRest(_)
            | AstKind::JSXOpeningElement(_)
            | AstKind::JSXClosingElement(_)
            | AstKind::JSXMemberExpression(_),
        ) => true,
        _ => false,
    }
}

fn is_pattern(p: &BindingPattern) -> bool {
    matches!(
        p,
        BindingPattern::ObjectPattern(_) | BindingPattern::ArrayPattern(_)
    )
}

fn key_name(k: &PropertyKey) -> Option<String> {
    match k {
        PropertyKey::StaticIdentifier(i) => Some(i.name.to_string()),
        PropertyKey::Identifier(i) => Some(i.name.to_string()),
        _ => None,
    }
}

/// tofu inspectPatternElementForKeys: property paths read by a destructuring pattern.
fn pattern_keys(p: &BindingPattern) -> Vec<Vec<String>> {
    match p {
        BindingPattern::ObjectPattern(o) if o.rest.is_none() => o
            .properties
            .iter()
            .filter_map(|prop| Some((key_name(&prop.key)?, &prop.value)))
            .flat_map(|(name, value)| {
                pattern_keys(value)
                    .into_iter()
                    .map(move |rest| [vec![name.clone()], rest].concat())
            })
            .collect(),
        BindingPattern::ArrayPattern(a) if a.rest.is_none() => a
            .elements
            .iter()
            .enumerate()
            .filter_map(|(i, e)| Some((i, e.as_ref()?)))
            .flat_map(|(i, e)| {
                pattern_keys(e)
                    .into_iter()
                    .map(move |rest| [vec![i.to_string()], rest].concat())
            })
            .collect(),
        _ => vec![vec![]],
    }
}

/// tofu inspectPatternElementForDeclarations: each bound name and the path it holds.
fn pattern_declarations<'a>(
    p: &'a BindingPattern<'a>,
    path: Vec<String>,
) -> Vec<(
    Option<&'a oxc::ast::ast::BindingIdentifier<'a>>,
    Vec<String>,
)> {
    match p {
        BindingPattern::BindingIdentifier(b) => vec![(Some(b), path)],
        BindingPattern::AssignmentPattern(a) => pattern_declarations(&a.left, path),
        BindingPattern::ObjectPattern(o) if o.rest.is_none() => o
            .properties
            .iter()
            .filter_map(|prop| Some((key_name(&prop.key)?, &prop.value)))
            .flat_map(|(name, value)| {
                pattern_declarations(value, [path.clone(), vec![name]].concat())
            })
            .collect(),
        BindingPattern::ArrayPattern(a) if a.rest.is_none() => a
            .elements
            .iter()
            .enumerate()
            .filter_map(|(i, e)| Some((i, e.as_ref()?)))
            .flat_map(|(i, e)| {
                pattern_declarations(e, [path.clone(), vec![i.to_string()]].concat())
            })
            .collect(),
        _ => vec![(None, path)],
    }
}

/// tofu reduceToTopmostApiCalls: drop a path when any dotted prefix of it is present.
pub fn reduce_to_topmost(usage: Vec<(String, Use)>) -> Vec<(String, Use)> {
    let keys: BTreeSet<&str> = usage.iter().map(|(k, _)| k.as_str()).collect();
    let keep: Vec<bool> = usage.iter().map(|(k, _)| !has_parent(k, &keys)).collect();
    usage
        .into_iter()
        .zip(keep)
        .filter(|(_, k)| *k)
        .map(|(u, _)| u)
        .collect()
}

pub fn reduce_strings(items: BTreeSet<String>) -> BTreeSet<String> {
    let keys: BTreeSet<&str> = items.iter().map(String::as_str).collect();
    items
        .iter()
        .filter(|k| !has_parent(k, &keys))
        .cloned()
        .collect()
}

fn has_parent(key: &str, keys: &BTreeSet<&str>) -> bool {
    key.match_indices('.')
        .any(|(i, _)| keys.contains(&key[..i]))
}

/// @lavamoat/node globalMapToGlobalPolicy: a write to `a.b` becomes a read of `a`.
fn globals_policy(usage: Vec<(String, Use)>) -> BTreeMap<String, Use> {
    let mut out = BTreeMap::new();
    for (key, use_) in usage {
        match (use_, key.find('.')) {
            (Use::Write, Some(dot)) if dot > 0 && !key.ends_with('.') => {
                out.insert(key[..dot].to_string(), Use::Read);
            }
            _ => {
                out.insert(key, use_);
            }
        }
    }
    out
}

struct LineIndex(Vec<u32>);

impl LineIndex {
    fn new(src: &str) -> Self {
        Self(src.match_indices('\n').map(|(i, _)| i as u32).collect())
    }
    fn line(&self, offset: u32) -> u32 {
        self.0.partition_point(|&nl| nl < offset) as u32 + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn globals(src: &str, mjs: bool) -> Vec<String> {
        inspect(src, mjs)
            .unwrap()
            .globals
            .into_iter()
            .map(|(k, u)| {
                if u == Use::Write {
                    format!("{k}=write")
                } else {
                    k
                }
            })
            .collect()
    }

    fn builtins(src: &str, mjs: bool) -> Vec<String> {
        inspect(src, mjs).unwrap().builtins.into_iter().collect()
    }

    #[test]
    fn member_chains_are_kept_and_reduced() {
        assert_eq!(
            globals("console.error(1); console.log(2)", true),
            ["console.error", "console.log"]
        );
        assert_eq!(globals("console.log(1); console(2)", true), ["console"]);
        assert_eq!(
            globals("process.env.FOO; const { env } = process", true),
            ["process.env"]
        );
        assert_eq!(globals("x?.y.z", true), ["x.y.z"]);
    }

    #[test]
    fn global_this_aliases_are_stripped() {
        assert_eq!(
            globals("globalThis.fetch(1); window.foo.bar(); typeof self", true),
            ["fetch", "foo.bar"]
        );
        assert_eq!(
            globals("const { a: { b }, c } = window.d", true),
            ["d.a.b", "d.c"]
        );
    }

    #[test]
    fn writes_follow_tofu() {
        assert_eq!(
            globals("globalThis.__x = 1; a.b = 2; c = 3; d.e.f = 4", true),
            ["__x=write", "a", "c=write", "d.e.f"]
        );
    }

    #[test]
    fn language_and_module_refs_are_ignored() {
        assert_eq!(
            globals(
                "Object.keys(1); JSON.parse(''); undefined; this; require('x'); new URL('a')",
                true
            ),
            ["URL", "require"]
        );
        assert_eq!(
            globals(
                "module.exports = { a: require('x') }; exports.b = 1; URL",
                false
            ),
            ["URL"]
        );
    }

    #[test]
    fn babel_quirks_are_kept() {
        assert_eq!(
            globals("let obj; obj[foo]; ({ [bar]: 1 })", true),
            Vec::<String>::new()
        );
        assert_eq!(globals("export * as ns from './x.js'", true), ["ns"]);
    }

    #[test]
    fn esm_builtins() {
        let src = "import { createServer, STATUS_CODES } from 'node:http'; import fs from 'fs';
            import * as os from 'os'; export { Readable } from 'node:stream'; export * from 'util';
            import 'dns'; import x from 'lodash'";
        assert_eq!(
            builtins(src, true),
            [
                "dns",
                "fs",
                "node:http.STATUS_CODES",
                "node:http.createServer",
                "node:stream.Readable",
                "os",
                "util"
            ]
        );
        assert_eq!(
            builtins("import fs from 'fs'; import { readFile } from 'fs'", true),
            ["fs"]
        );
    }

    #[test]
    fn cjs_builtins_follow_bindings() {
        let src = "const { readFileSync } = require('fs'); const cp = require('child_process');
            cp.spawn(); cp.exec(); require('os').cpus(); const p = require('path')";
        assert_eq!(
            builtins(src, false),
            [
                "child_process.exec",
                "child_process.spawn",
                "fs.readFileSync",
                "os.cpus",
                "path"
            ]
        );
    }

    #[test]
    fn imports_endo_follows() {
        let esm = inspect(
            "import a from 'a'; export * from './b.js'; export { c } from 'c'; import('d')",
            true,
        );
        assert_eq!(esm.unwrap().imports, ["a", "./b.js", "c"]);
        assert_eq!(
            inspect("require('x'); require(y)", false).unwrap().imports,
            ["x"]
        );
    }

    #[test]
    fn observations_include_what_lavamoat_misses() {
        let src = include_str!("../fixtures/tiny-slug/1.0.1/index.js");
        let seen = inspect(src, true).unwrap().seen;
        let has = |key: &str, builtin: bool| {
            seen.iter()
                .any(|s| s.key.starts_with(key) && s.builtin == builtin)
        };
        assert!(has("process.env", false));
        assert!(
            has("node:fs", true),
            "dynamic import() of a builtin is observed"
        );
        assert!(has("fetch", false));
        let fs = seen.iter().find(|s| s.key == "node:fs").unwrap();
        assert_eq!(fs.line, 13);
    }
}
