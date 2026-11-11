#!/usr/bin/env node
"use strict";

// Runs the prebuilt binary that npm installed from permlock-<platform>-<arch>.
// No install script: npm picks the right optional dependency for this machine.
const { spawnSync } = require("child_process");

function main() {
  const target = `${process.platform}-${process.arch}`;
  const exe = process.platform === "win32" ? "permlock.exe" : "permlock";
  let binary;
  try {
    binary = require.resolve(`permlock-${target}/bin/${exe}`);
  } catch {
    console.error(`permlock: no prebuilt binary for ${target}. Build it with: cargo install permlock`);
    return 2;
  }
  const result = spawnSync(binary, process.argv.slice(2), { stdio: "inherit" });
  if (result.error) {
    console.error(`permlock: ${result.error.message}`);
    return 2;
  }
  return result.status ?? 1;
}

process.exitCode = main();
