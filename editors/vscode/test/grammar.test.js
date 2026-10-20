// The grammar, run as the editor runs it: each word of these lines gets the colour it should.

"use strict";

const assert = require("assert");
const fs = require("fs");
const path = require("path");
const oniguruma = require("vscode-oniguruma");
const textmate = require("vscode-textmate");

async function load() {
  const wasm = fs.readFileSync(require.resolve("vscode-oniguruma/release/onig.wasm"));
  await oniguruma.loadWASM(wasm.buffer.slice(wasm.byteOffset, wasm.byteOffset + wasm.byteLength));
  const file = path.join(__dirname, "..", "syntaxes", "buttercupql.tmLanguage.json");
  const registry = new textmate.Registry({
    onigLib: Promise.resolve({
      createOnigScanner: (patterns) => new oniguruma.OnigScanner(patterns),
      createOnigString: (text) => new oniguruma.OnigString(text),
    }),
    loadGrammar: async (scopeName) =>
      scopeName === "source.buttercupql" ? textmate.parseRawGrammar(fs.readFileSync(file, "utf8"), file) : null,
  });
  return registry.loadGrammar("source.buttercupql");
}

/** Every token of a text, with its scopes. */
function tokenize(grammar, text) {
  const tokens = [];
  let state = textmate.INITIAL;
  for (const line of text.split("\n")) {
    const result = grammar.tokenizeLine(line, state);
    for (const token of result.tokens) {
      tokens.push({ text: line.slice(token.startIndex, token.endIndex), scopes: token.scopes });
    }
    state = result.ruleStack;
  }
  return tokens;
}

const TEXT = `# Critical cases nobody has taken
from shared import recent
from cases
where status == "open" and band in ["critical", "high"]
recent(30)
group by operator:
    total = count(where=amount > 1_000_000)
sort -risk
take 20
select key, risk, label = f"{key}: {risk}"
def recent(days_back=7):
    """The rows raised in the last days."""
    where \`from\` == None
`;

// A word, which of its appearances, and a scope it must have.
const EXPECTED = [
  ["#", 0, "comment.line.number-sign.buttercupql"],
  [" Critical cases nobody has taken", 0, "comment.line.number-sign.buttercupql"],
  ["from", 0, "keyword.control.import.buttercupql"],
  ["shared", 0, "entity.name.namespace.buttercupql"],
  ["import", 0, "keyword.control.import.buttercupql"],
  ["from", 1, "keyword.control.step.buttercupql"],
  ["cases", 0, "variable.other.buttercupql"],
  ["where", 0, "keyword.control.step.buttercupql"],
  ["==", 0, "keyword.operator.comparison.buttercupql"],
  ["open", 0, "string.quoted.double.buttercupql"],
  ["and", 0, "keyword.operator.logical.buttercupql"],
  ["in", 0, "keyword.operator.logical.buttercupql"],
  ["critical", 0, "string.quoted.double.buttercupql"],
  ["recent", 1, "entity.name.function.call.buttercupql"],
  ["30", 0, "constant.numeric.buttercupql"],
  ["group", 0, "keyword.control.step.buttercupql"],
  ["by", 0, "keyword.control.step.buttercupql"],
  ["count", 0, "support.function.builtin.buttercupql"],
  ["where", 1, "variable.parameter.function-call.buttercupql"],
  ["1_000_000", 0, "constant.numeric.buttercupql"],
  ["sort", 0, "keyword.control.step.buttercupql"],
  ["-", 0, "keyword.operator.arithmetic.buttercupql"],
  ["take", 0, "keyword.control.step.buttercupql"],
  ["select", 0, "keyword.control.step.buttercupql"],
  ["f", 0, "storage.type.string.buttercupql"],
  ["key", 1, "meta.interpolation.buttercupql"],
  ["def", 0, "storage.type.function.buttercupql"],
  ["recent", 2, "entity.name.function.buttercupql"],
  ["The rows raised in the last days.", 0, "string.quoted.docstring.multi.buttercupql"],
  ["where", 2, "keyword.control.step.buttercupql"],
  ["from", 2, "variable.other.quoted.buttercupql"],
  ["None", 0, "constant.language.buttercupql"],
];

load()
  .then((grammar) => {
    const tokens = tokenize(grammar, TEXT);
    for (const [text, nth, scope] of EXPECTED) {
      const found = tokens.filter((token) => token.text === text);
      assert.ok(found.length > nth, `\`${text}\` appears ${found.length} times, not ${nth + 1}`);
      assert.ok(
        found[nth].scopes.includes(scope),
        `\`${text}\` (${nth + 1}) has ${found[nth].scopes.join(" ")}, not ${scope}`,
      );
    }
    console.log(`grammar: ${EXPECTED.length} words have their colours`);
  })
  .catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
