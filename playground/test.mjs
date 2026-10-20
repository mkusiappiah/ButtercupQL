// The playground's compiler, as the page loads it: the WebAssembly build compiles every example the page offers.
// Usage: node playground/test.mjs SITE, where SITE is the folder `build.mjs` wrote.

import assert from "node:assert";
import { readFileSync } from "node:fs";
import { join } from "node:path";

const site = process.argv[2];
if (!site) {
  console.error("usage: node playground/test.mjs SITE");
  process.exit(2);
}

const { instance } = await WebAssembly.instantiate(readFileSync(join(site, "buttercupql.wasm")), {});
const exports = instance.exports;
const encoder = new TextEncoder();
const decoder = new TextDecoder();

function compile(text, dialect, schema, query) {
  const parts = [text, dialect, schema, query].map((part) => {
    const bytes = encoder.encode(part);
    const pointer = exports.bcql_alloc(bytes.length);
    new Uint8Array(exports.memory.buffer, pointer, bytes.length).set(bytes);
    return [pointer, bytes.length];
  });
  const length = exports.bcql_compile(...parts.flat());
  const answer = JSON.parse(decoder.decode(new Uint8Array(exports.memory.buffer, exports.bcql_output(), length)));
  for (const [pointer, size] of parts) {
    exports.bcql_free(pointer, size);
  }
  return answer;
}

const examples = JSON.parse(readFileSync(join(site, "examples.json"), "utf8"));
const schema = readFileSync(join(site, "schema.sql"), "utf8");
assert.ok(examples.length >= 7, `only ${examples.length} examples`);
let compiled = 0;
for (const example of examples) {
  for (const dialect of ["postgres", "sqlite", "duckdb", "clickhouse"]) {
    for (const query of ["", ...compile(example.text, dialect, schema, "").queries]) {
      const answer = compile(example.text, dialect, schema, query);
      assert.ok(answer.ok, `${example.name} (${query || "main"}) on ${dialect}: ${JSON.stringify(answer)}`);
      compiled += 1;
    }
  }
}

// A text of several statements gives each, with its own placeholders.
const script = compile(
  examples.find((example) => example.name === "changing_data").text,
  "postgres",
  schema,
  "",
);
assert.deepStrictEqual(
  script.statements.map((statement) => statement.kind),
  ["definition", "change", "change"],
);
assert.deepStrictEqual(script.statements[2].params, ["stale", "open"]);
assert.deepStrictEqual(script.statements[2].names, ["p1", "p2"]);

// A mistake comes back with its place, and the memory is given back after each call.
const before = exports.memory.buffer.byteLength;
for (let i = 0; i < 2000; i++) {
  compile("from cases\nwhere status = 'open'\n", "postgres", schema, "");
}
const wrong = compile("from cases\nwhere status = 'open'\n", "postgres", schema, "");
assert.strictEqual(wrong.ok, false);
assert.deepStrictEqual([wrong.problems[0].line, wrong.problems[0].column], [2, 14]);
assert.strictEqual(exports.memory.buffer.byteLength, before, "the memory grew: something is not given back");
console.log(`playground: ${compiled} compiles of ${examples.length} examples in WebAssembly`);
