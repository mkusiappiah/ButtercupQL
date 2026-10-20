// The ButtercupQL playground. The compiler, built for WebAssembly, compiles the query at every key press, in the
// browser: nothing written here is sent anywhere.

"use strict";

const DIALECTS = { postgres: "PostgreSQL", sqlite: "SQLite", duckdb: "DuckDB", clickhouse: "ClickHouse" };

// The words ButtercupQL colours, as the editor extension colours them.
const STEPS = new Set([
  "from", "where", "select", "del", "sort", "take", "skip", "distinct", "group", "summarize", "join", "left", "pass",
  "update", "delete", "insert", "pivot",
]);
// Statements, and the words that follow them on their line.
const STATEMENTS = new Set(["create", "alter", "drop", "grant", "revoke", "with", "rename"]);
const STATEMENT_WORDS = {
  create: ["table", "unique", "index"],
  alter: ["table"],
  drop: ["table", "index"],
  grant: ["to"],
  revoke: ["from"],
  with: ["transaction"],
  rename: ["to"],
};
const PRIVILEGES = new Set(["read", "write", "insert", "update", "delete", "all"]);
// The types of a column or a parameter, written after `:`.
const TYPES = new Set(["int", "float", "str", "bool", "date", "datetime", "Decimal", "bytes", "Optional"]);
const WORDS = new Set(["and", "or", "not", "in", "is", "if", "else", "def", "import", "as", "by", "on"]);
const CONSTANTS = new Set(["True", "False", "None"]);
const SQL_WORDS = new Set(
  ("SELECT DISTINCT FROM WHERE GROUP BY HAVING ORDER LIMIT OFFSET LEFT INNER JOIN ON AS AND OR NOT IN IS NULL CASE " +
    "WHEN THEN ELSE END OVER PARTITION ROWS RANGE BETWEEN PRECEDING FOLLOWING UNBOUNDED CURRENT ROW FILTER QUALIFY " +
    "INTERVAL CAST NULLS LAST FIRST DESC ASC SETTINGS LIKE GLOB ESCAPE TRUE FALSE WITH EXISTS TIMESTAMP DATE INTEGER " +
    "BIGINT TEXT REAL NUMERIC DOUBLE PRECISION CURRENT_DATE DAY DAYS HOUR MINUTE SECOND RECURSIVE UNION INTERSECT " +
    "EXCEPT ALL INSERT INTO VALUES UPDATE SET DELETE RETURNING CREATE TABLE INDEX UNIQUE PRIMARY KEY REFERENCES " +
    "DEFAULT ALTER ADD COLUMN DROP RENAME TO IF GRANT REVOKE BEGIN COMMIT REPLACE FUNCTION PROCEDURE MACRO VIEW " +
    "RETURNS SETOF LANGUAGE STABLE ATOMIC ENGINE BOOLEAN VARCHAR BLOB BYTEA DECIMAL").split(" "),
);

const $ = (id) => document.getElementById(id);

function escape(text) {
  return text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

/** ButtercupQL as HTML, each token in its colour. */
function highlightQuery(text) {
  const token =
    /(#[^\n]*)|([fF]?(?:"""[\s\S]*?(?:"""|$)|'''[\s\S]*?(?:'''|$)|"(?:[^"\\\n]|\\.)*"?|'(?:[^'\\\n]|\\.)*'?))|(`[^`\n]*`?)|(\d[\d_]*(?:\.\d[\d_]*)?(?:[eE][+-]?\d+)?)|([\p{L}_][\p{L}\p{N}_]*)|(\n)|([^\S\n]+)|([\s\S])/uy;
  let html = "";
  let lineStart = true;
  let previous = "";
  // The word the line starts with, and whether the token before was an `@`, or a `:`.
  let first = "";
  let decorator = false;
  let colon = false;
  let match;
  token.lastIndex = 0;
  while ((match = token.exec(text)) !== null) {
    const [whole, comment, string, quoted, number, word, newline, space] = match;
    if (comment !== undefined) {
      html += `<span class="t-comment">${escape(comment)}</span>`;
    } else if (string !== undefined) {
      html += `<span class="t-string">${escape(string)}</span>`;
      lineStart = false;
    } else if (quoted !== undefined) {
      html += escape(quoted);
      lineStart = false;
    } else if (number !== undefined) {
      html += `<span class="t-number">${number}</span>`;
      lineStart = false;
    } else if (word !== undefined) {
      const after = text.slice(token.lastIndex).match(/^[^\S\n]*(.)/);
      if (lineStart) {
        first = word;
      }
      const statement =
        (lineStart && STATEMENTS.has(word)) || (!lineStart && (STATEMENT_WORDS[first] || []).includes(word));
      const step =
        (lineStart && STEPS.has(word)) ||
        (word === "by" && (previous === "group" || first === "pivot")) ||
        (word === "join" && previous === "left") ||
        (word === "into" && previous === "insert");
      let kind = "";
      if (decorator) {
        kind = "t-function";
      } else if (colon && TYPES.has(word) && !(after && after[1] === "(")) {
        kind = "t-constant";
      } else if (step || statement) {
        kind = "t-step";
      } else if ((first === "grant" || first === "revoke") && PRIVILEGES.has(word)) {
        kind = "t-keyword";
      } else if (CONSTANTS.has(word)) {
        kind = "t-constant";
      } else if (WORDS.has(word)) {
        kind = "t-keyword";
      } else if (after && after[1] === "(") {
        kind = "t-function";
      }
      html += kind ? `<span class="${kind}">${word}</span>` : word;
      previous = step ? word : "";
      lineStart = false;
      decorator = false;
      colon = false;
    } else if (newline !== undefined) {
      html += "\n";
      lineStart = true;
      first = "";
    } else if (space !== undefined) {
      html += space;
    } else {
      decorator = whole === "@" && lineStart;
      colon = whole === ":";
      html += decorator ? `<span class="t-function">@</span>` : escape(whole);
      lineStart = whole === ";";
    }
  }
  return html;
}

/** SQL as HTML, each token in its colour. */
function highlightSql(text) {
  const token =
    /(--[^\n]*)|('(?:[^']|'')*'?)|(\$\d+|\?|\{[\p{L}_][\p{L}\p{N}_]*:[\w()]+\}|%s|:[\p{L}_][\p{L}\p{N}_]*)|(\d+(?:\.\d+)?)|([\p{L}_][\p{L}\p{N}_]*)|([\s\S])/uy;
  let html = "";
  let match;
  token.lastIndex = 0;
  while ((match = token.exec(text)) !== null) {
    const [whole, comment, string, param, number, word] = match;
    if (comment !== undefined) {
      html += `<span class="t-comment">${escape(comment)}</span>`;
    } else if (string !== undefined) {
      html += `<span class="t-string">${escape(string)}</span>`;
    } else if (param !== undefined) {
      html += `<span class="t-param">${escape(param)}</span>`;
    } else if (number !== undefined) {
      html += `<span class="t-number">${number}</span>`;
    } else if (word !== undefined) {
      if (SQL_WORDS.has(word)) {
        html += `<span class="t-keyword">${word}</span>`;
      } else if (text[token.lastIndex] === "(") {
        html += `<span class="t-function">${word}</span>`;
      } else {
        html += word;
      }
    } else {
      html += escape(whole);
    }
  }
  return html;
}

/** Where each line of a text starts, to find a problem's place from its line and column. */
function lineStarts(text) {
  const starts = [0];
  for (let at = text.indexOf("\n"); at !== -1; at = text.indexOf("\n", at + 1)) {
    starts.push(at + 1);
  }
  return starts;
}

/** The place in a JavaScript string of a line and a column, both counted from 1, the column in characters. */
function offsetOf(text, starts, line, column) {
  if (line > starts.length) {
    return text.length;
  }
  let at = starts[line - 1];
  for (let counted = 1; counted < column && at < text.length && text[at] !== "\n"; counted++) {
    at += text.codePointAt(at) > 0xffff ? 2 : 1;
  }
  return at;
}

/** The text with each problem's place underlined, and nothing else shown. */
function markProblems(text, problems) {
  const starts = lineStarts(text);
  const ranges = problems
    .map((problem) => {
      const start = offsetOf(text, starts, problem.line, problem.column);
      let end = offsetOf(text, starts, problem.end_line, problem.end_column);
      if (end <= start) {
        end = Math.min(start + 1, text.length);
      }
      return { start, end, kind: problem.severity };
    })
    .sort((a, b) => a.start - b.start);
  let html = "";
  let at = 0;
  for (const range of ranges) {
    if (range.start < at) {
      continue;
    }
    html += escape(text.slice(at, range.start));
    const marked = text.slice(range.start, range.end) || " ";
    html += `<span class="${range.kind}">${escape(marked)}</span>`;
    at = Math.max(range.end, range.start);
  }
  return html + escape(text.slice(at));
}

/** The compiler, built for WebAssembly. */
async function loadCompiler() {
  const response = fetch("buttercupql.wasm");
  let instance;
  try {
    ({ instance } = await WebAssembly.instantiateStreaming(response, {}));
  } catch {
    // A server that does not say the file is WebAssembly.
    const bytes = await (await fetch("buttercupql.wasm")).arrayBuffer();
    ({ instance } = await WebAssembly.instantiate(bytes, {}));
  }
  const exports = instance.exports;
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();
  return function compile(text, dialect, schema, query) {
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
  };
}

/** The state of the page as a link: the query, the database and the schema. */
function encodeState(state) {
  const bytes = new TextEncoder().encode(JSON.stringify(state));
  let binary = "";
  for (const byte of bytes) {
    binary += String.fromCharCode(byte);
  }
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function decodeState(text) {
  try {
    const binary = atob(text.replace(/-/g, "+").replace(/_/g, "/"));
    const bytes = Uint8Array.from(binary, (c) => c.charCodeAt(0));
    return JSON.parse(new TextDecoder().decode(bytes));
  } catch {
    return null;
  }
}

function remembered() {
  try {
    return JSON.parse(localStorage.getItem("buttercupql-playground") || "null");
  } catch {
    return null;
  }
}

function remember(state) {
  try {
    localStorage.setItem("buttercupql-playground", JSON.stringify(state));
  } catch {
    // A private window or blocked storage: the page works without it.
  }
}

/** A value for a placeholder, as the SQL's parameters show it. */
function shownValue(value) {
  if (value !== null && typeof value === "object") {
    return `<em>the argument ${escape(value.arg)}</em>`;
  }
  return escape(JSON.stringify(value));
}

function placeholder(dialect, index, name) {
  if (dialect === "sqlite") {
    return `?${index + 1}`;
  }
  if (dialect === "clickhouse") {
    return `{${name}}`;
  }
  return `$${index + 1}`;
}

async function main() {
  const query = $("query");
  const schema = $("schema");
  const dialect = $("dialect");
  const example = $("example");
  const queryChoice = $("query-choice");
  const queryName = $("query-name");
  const editor = $("query-editor");
  const gutter = editor.querySelector(".gutter");
  const highlight = editor.querySelector(".highlight");
  const marks = editor.querySelector(".marks");
  const status = $("status");

  let compile;
  let examples = [];
  let defaultSchema = "";
  try {
    [compile, examples, defaultSchema] = await Promise.all([
      loadCompiler(),
      fetch("examples.json").then((response) => response.json()),
      fetch("schema.sql").then((response) => response.text()),
    ]);
  } catch (error) {
    status.textContent = `The compiler could not be loaded: ${error.message}`;
    return;
  }

  const none = document.createElement("option");
  none.value = "";
  none.textContent = "Choose an example";
  example.append(none);
  for (const [index, item] of examples.entries()) {
    const option = document.createElement("option");
    option.value = String(index);
    option.textContent = item.title;
    example.append(option);
  }

  const fromLink = location.hash.length > 1 ? decodeState(location.hash.slice(1)) : null;
  const start = fromLink || remembered() || {};
  query.value = typeof start.q === "string" ? start.q : examples[0] ? examples[0].text : "";
  schema.value = typeof start.s === "string" ? start.s : defaultSchema;
  dialect.value = DIALECTS[start.d] ? start.d : "postgres";
  const startExample = examples.findIndex((item) => item.text === query.value);
  example.value = startExample >= 0 ? String(startExample) : "";

  let lastAnswer = null;

  function showSchemaSummary() {
    const tables = (schema.value.match(/CREATE\s+TABLE/gi) || []).length;
    $("schema-summary").textContent = tables ? `${tables} table${tables === 1 ? "" : "s"}` : "none: names are not checked";
  }

  function paint() {
    const text = query.value;
    // A last empty line still takes its height.
    highlight.innerHTML = highlightQuery(text) + "\n";
    const lines = text.split("\n").length;
    gutter.textContent = Array.from({ length: lines }, (_, i) => i + 1).join("\n") + "\n";
    sync();
  }

  function sync() {
    highlight.scrollTop = marks.scrollTop = gutter.scrollTop = query.scrollTop;
    highlight.scrollLeft = marks.scrollLeft = query.scrollLeft;
  }

  function run() {
    const text = query.value;
    const chosen = queryName.value;
    const answer = compile(text, dialect.value, schema.value, chosen);
    lastAnswer = answer;
    // Browsers round their clocks to a tenth of a millisecond, so the time shown is the mean of twenty compiles.
    const started = performance.now();
    for (let i = 0; i < 20; i++) {
      compile(text, dialect.value, schema.value, chosen);
    }
    const took = (performance.now() - started) / 20;

    // The queries that can be compiled on their own.
    const names = answer.queries || [];
    queryChoice.hidden = names.length === 0;
    const options = ["", ...names];
    if (queryName.options.length !== options.length || options.some((name, i) => queryName.options[i].value !== name)) {
      queryName.replaceChildren(
        ...options.map((name) => {
          const option = document.createElement("option");
          option.value = name;
          option.textContent = name || "the main query";
          return option;
        }),
      );
      queryName.value = names.includes(chosen) ? chosen : "";
    }

    const problems = answer.problems || answer.schema_problems || [];
    const warnings = answer.warnings || [];
    marks.innerHTML = markProblems(text, answer.schema_problems ? [] : [...problems, ...warnings]) + "\n";
    sync();

    $("sql").innerHTML = answer.ok ? highlightSql(answer.sql) : "";
    const params = $("params");
    if (answer.ok && (answer.params.length > 0 || answer.args.length > 0)) {
      let html = "";
      // A text of several statements: each runs on its own, with its own placeholders.
      const statements = answer.statements && answer.statements.length > 1 ? answer.statements : [answer];
      statements.forEach((statement, n) => {
        if (statement.params.length === 0) {
          return;
        }
        const label = statements.length > 1 ? `Parameters of statement ${n + 1}` : "Parameters";
        html += `<table><tr><td class="label" colspan="2">${label}</td></tr>`;
        statement.params.forEach((value, i) => {
          html += `<tr><td class="t-param">${escape(placeholder(dialect.value, i, statement.names[i]))}</td><td>${shownValue(value)}</td></tr>`;
        });
        html += "</table>";
      });
      if (answer.args.length > 0) {
        html += '<table><tr><td class="label" colspan="2">Arguments, given when the query runs</td></tr>';
        for (const arg of answer.args) {
          const fallback = arg.default === null ? "" : ` = ${escape(JSON.stringify(arg.default))}`;
          html += `<tr><td>${escape(arg.name)}${fallback}</td><td class="muted">${escape(arg.type)}</td></tr>`;
        }
        html += "</table>";
      }
      params.innerHTML = html;
    } else {
      params.innerHTML = "";
    }

    const shown = answer.error
      ? [{ severity: "error", rendered: `error: ${answer.error}` }]
      : [...problems, ...warnings];
    $("problems").replaceChildren(
      ...shown.map((problem) => {
        const box = document.createElement("pre");
        box.className = `problem ${problem.severity}`;
        box.textContent = problem.rendered.trimEnd();
        return box;
      }),
    );

    const name = DIALECTS[dialect.value];
    const version = answer.version ? ` · ButtercupQL ${answer.version}` : "";
    if (answer.ok) {
      const warned = warnings.length ? ` · ${warnings.length} warning${warnings.length === 1 ? "" : "s"}` : "";
      const time = took < 0.1 ? `${Math.max(1, Math.round(took * 1000))} µs` : `${took.toFixed(2)} ms`;
      status.textContent = `Compiled for ${name} in ${time}${warned}${version}`;
    } else if (answer.schema_problems) {
      status.textContent = "The schema can't be read: see its problems above.";
    } else {
      const errors = problems.filter((problem) => problem.severity === "error").length;
      status.textContent = `${errors || problems.length} problem${(errors || problems.length) === 1 ? "" : "s"} to fix${version}`;
    }
    remember({ q: text, s: schema.value, d: dialect.value });
  }

  function update() {
    paint();
    run();
  }

  /** Types text in place of the selection, keeping the editor's undo. */
  function type(text) {
    if (!document.execCommand("insertText", false, text)) {
      query.setRangeText(text, query.selectionStart, query.selectionEnd, "end");
      update();
    }
  }

  query.addEventListener("input", () => {
    example.value = "";
    update();
  });
  query.addEventListener("scroll", sync);
  query.addEventListener("keydown", (event) => {
    if (event.key === "Tab" && !event.shiftKey && !event.metaKey && !event.ctrlKey && !event.altKey) {
      event.preventDefault();
      type("    ");
    } else if (event.key === "Enter" && !event.shiftKey && !event.metaKey && !event.ctrlKey && !event.altKey) {
      // A new line keeps the indentation, and goes in one step more after a `:`.
      const before = query.value.slice(0, query.selectionStart);
      const line = before.slice(before.lastIndexOf("\n") + 1);
      const indent = line.match(/^[ \t]*/)[0];
      const more = /:\s*(#.*)?$/.test(line) ? "    " : "";
      event.preventDefault();
      type(`\n${indent}${more}`);
    }
  });
  schema.addEventListener("input", () => {
    showSchemaSummary();
    run();
  });
  dialect.addEventListener("change", run);
  queryName.addEventListener("change", run);
  example.addEventListener("change", () => {
    const chosen = examples[Number(example.value)];
    if (chosen) {
      query.value = chosen.text;
      queryName.value = "";
      query.scrollTop = 0;
      update();
    }
  });

  $("share").addEventListener("click", async () => {
    const link = `${location.origin}${location.pathname}#${encodeState({ q: query.value, d: dialect.value, s: schema.value })}`;
    history.replaceState(null, "", link);
    const button = $("share");
    try {
      await navigator.clipboard.writeText(link);
      button.textContent = "Link copied";
    } catch {
      button.textContent = "Link in the address bar";
    }
    setTimeout(() => {
      button.textContent = "Share";
    }, 2000);
  });

  $("copy").addEventListener("click", async () => {
    if (!lastAnswer || !lastAnswer.ok) {
      return;
    }
    const button = $("copy");
    try {
      await navigator.clipboard.writeText(lastAnswer.sql);
      button.textContent = "Copied";
    } catch {
      button.textContent = "Select the SQL to copy it";
    }
    setTimeout(() => {
      button.textContent = "Copy";
    }, 1500);
  });

  showSchemaSummary();
  update();
  query.focus();
}

main();
