// Puts the playground together for the web: its page, the compiler built for WebAssembly, the examples and the
// schema. Usage: node playground/build.mjs WASM OUT, where WASM is the built `buttercupql_wasm.wasm`.

import { copyFileSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "..");
const [wasm, out] = process.argv.slice(2);
if (!wasm || !out) {
  console.error("usage: node playground/build.mjs WASM OUT");
  process.exit(2);
}

mkdirSync(out, { recursive: true });
for (const file of ["index.html", "playground.css", "playground.js", "schema.sql"]) {
  copyFileSync(join(here, file), join(out, file));
}
copyFileSync(join(root, "docs", "assets", "buttercup.svg"), join(out, "buttercup.svg"));
copyFileSync(wasm, join(out, "buttercupql.wasm"));

// The examples of the language, then the playground's own. Each one's title is its first line's comment.
const examples = [];
for (const folder of [join(root, "examples"), join(here, "examples")]) {
  const names = readdirSync(folder)
    .filter((name) => name.endsWith(".bcql"))
    .sort();
  for (const name of names) {
    const text = readFileSync(join(folder, name), "utf8");
    const first = text.split("\n")[0];
    const title = first.startsWith("#") ? first.replace(/^#\s*/, "") : basename(name, ".bcql");
    examples.push({ name: basename(name, ".bcql"), title, text });
  }
}
// The playground opens with the query the language's design starts with.
examples.sort((a, b) => Number(b.name === "open_critical") - Number(a.name === "open_critical"));
writeFileSync(join(out, "examples.json"), JSON.stringify(examples, null, 1) + "\n");
console.log(`playground: ${examples.length} examples, in ${out}`);
