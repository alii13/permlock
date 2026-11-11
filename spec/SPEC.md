# permlock.json, v0 draft

Status: draft. It becomes 1.0 when a second implementation passes every case in `conformance/`.

permlock.json records what each package in a JavaScript dependency tree was observed touching. It is generated, committed next to the package lockfile, and compared on every dependency change. It never grants anything. Enforcement at runtime stays with tools like LavaMoat, whose policy format this spec does not replace.

## Three pieces

| Piece | File | Who writes it |
| --- | --- | --- |
| Vocabulary | [`capabilities.json`](capabilities.json) | This spec |
| Declaration | `"capabilities"` in a package's own package.json | The package maintainer |
| Report | `permlock.json` | A tool, from the installed tree |

All three are JSON.

## Vocabulary

A capability class is a closed name for a kind of power. Profile `node` has nine.

| Class | Means | Matched by |
| --- | --- | --- |
| `net` | talks to the network | builtins net, tls, http, https, http2, dns, dgram; globals fetch, WebSocket, XMLHttpRequest, EventSource, navigator.sendBeacon |
| `fs` | reads or writes files | builtins fs, fs/promises |
| `env` | reads environment variables | global process.env |
| `exec` | starts other programs | builtin child_process |
| `eval` | runs code built at runtime | builtin vm; globals eval, Function, WebAssembly |
| `worker` | starts threads or processes | builtins worker_threads, cluster; global Worker |
| `native` | loads compiled code | loaded `.node` files, a `binding.gyp`, global process.dlopen |
| `system` | changes the process or machine | builtins os, inspector, v8; globals process.kill, process.exit, process.chdir |
| `install` | runs at install time | package.json scripts preinstall, install, postinstall. Not prepare, which npm skips for published packages |

Rules:

1. A builtin matches its listed name with or without the `node:` prefix, and any subpath of it. `node:fs/promises` is `fs`.
2. A global matches its listed name and any member path below it. `process.env.HOME` is `env` via `process.env`. A leading `globalThis`, `window`, `self`, `global`, `top`, `frames` or `parent` is removed first.
3. Anything that matches no class is pure and is not recorded.
4. Classes are closed per profile. A new class needs a new spec version.

## Declaration

```json
{ "capabilities": ["net", "env"] }
```

An empty list means the package needs nothing. A missing field means the package has not declared. Tools must treat a missing field as no claim, never as an empty list.

## Report

```json
{
  "schema": "permlock/v0",
  "profile": "node",
  "packages": {
    "tiny-slug": {
      "version": "1.0.1",
      "observed": {
        "env": [{ "file": "index.js", "line": 6, "via": "process.env" }],
        "fs": [{ "file": "index.js", "line": 13, "via": "node:fs" }],
        "net": [{ "file": "index.js", "line": 29, "via": "fetch" }]
      },
      "declared": null
    }
  }
}
```

1. `packages` is keyed by canonical name: the cheapest dependency path from the root, names joined with `>`. Cheapest means fewest hops, then the shortest total length of the resolved package locations, then those locations compared in order. This is the naming LavaMoat and Endo use, so names line up with a LavaMoat policy.
2. Every package reachable from the root's dependencies appears, including those that need nothing, which have `"observed": {}`.
3. `observed` holds one array per class. Each item is the earliest place a vocabulary entry matched: `file` relative to the package, 1-based `line`, and `via`, the entry that matched. `line` is 0 for a whole file or a package.json script.
4. `declared` copies the package's own `capabilities` field, or is `null`.
5. Order is fixed so a git diff of two reports is readable: top-level keys as shown, package names sorted, classes sorted, evidence by file then line, and each evidence item as `file`, `line`, `via`. Output is two-space indented JSON with a trailing newline.
6. Only modules reachable from a package's entry points are analyzed: `exports` subpaths, `module` or `main`, and `bin`, then everything they import. Dynamic `import()` of a builtin counts.

[`permlock.schema.json`](permlock.schema.json) is the JSON Schema for this file.

## Comparing two reports

A change is an escalation when a package in the new report:

1. has a class the old report did not have for it, or
2. is new to the tree and has any class, or
3. has a class its own declaration leaves out.

Everything else is info: removed classes, version bumps that add nothing, new packages that need nothing, removed packages.

## Conformance

Each folder in [`conformance/`](conformance) names an app in `fixtures/e2e/`. Running an implementation on that app must produce `expected.json` byte for byte. Open an issue to add your implementation to the list once it passes.

## Changes

Changes to this spec go through a pull request with a rationale section. Class names follow Google Capslock's names where the meaning matches.
