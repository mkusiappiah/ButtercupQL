// The extension's own code, run with test stand-ins for VS Code and its language client: what it registers, how it
// starts the server, and how it writes the SQL a file compiles to.

"use strict";

const assert = require("assert");
const fs = require("fs");
const Module = require("module");
const os = require("os");
const path = require("path");

const commands = {};
const providers = {};
const opened = [];
const requests = [];
let client;
let answer;

class EventEmitter {
  constructor() {
    this.listeners = [];
    this.event = (listener) => this.listeners.push(listener);
  }

  fire(value) {
    for (const listener of this.listeners) {
      listener(value);
    }
  }
}

const settings = { dialect: "sqlite", schema: "schema.sql", "server.path": "" };
const disposable = { dispose() {} };

const vscode = {
  EventEmitter,
  ViewColumn: { Beside: -2 },
  Uri: {
    from: (parts) => ({ ...parts, toString: () => `${parts.scheme}:${parts.path}?${parts.query}` }),
    parse: (text) => ({ toString: () => text }),
  },
  workspace: {
    getConfiguration: () => ({ get: (key) => settings[key] }),
    createFileSystemWatcher: (glob) => ({ glob }),
    registerTextDocumentContentProvider: (scheme, provider) => {
      providers[scheme] = provider;
      return disposable;
    },
    onDidChangeTextDocument: () => disposable,
    onDidCloseTextDocument: () => disposable,
    onDidChangeConfiguration: () => disposable,
    openTextDocument: async (uri) => {
      opened.push(uri);
      return { uri };
    },
  },
  window: {
    activeTextEditor: undefined,
    createOutputChannel: () => disposable,
    showErrorMessage: async () => undefined,
    showInformationMessage: async () => undefined,
    showTextDocument: async () => undefined,
  },
  commands: {
    registerCommand: (name, run) => {
      commands[name] = run;
      return disposable;
    },
    executeCommand: async () => [],
  },
  languages: { setTextDocumentLanguage: async (document) => document },
  env: { openExternal: () => undefined },
};

class LanguageClient {
  constructor(id, name, serverOptions, clientOptions) {
    client = { id, name, serverOptions, clientOptions, stopped: false };
    this.record = client;
  }

  async start() {}

  async stop() {
    this.record.stopped = true;
  }

  async sendRequest(method, params) {
    requests.push({ method, params });
    return answer;
  }
}

const load = Module._load;
Module._load = function (request, parent, isMain) {
  if (request === "vscode") {
    return vscode;
  }
  if (request === "vscode-languageclient/node") {
    return { LanguageClient };
  }
  return load.call(this, request, parent, isMain);
};

const extension = require(path.join(__dirname, "..", "src", "extension.js"));

async function main() {
  const context = { extensionPath: path.join(__dirname, "no-such-folder"), subscriptions: [] };
  await extension.activate(context);
  assert.deepStrictEqual(Object.keys(commands).sort(), ["buttercupql.restartServer", "buttercupql.showSql"]);
  // No `bcql` comes with this copy, so the one on the PATH is used.
  assert.deepStrictEqual(client.serverOptions.run, { command: "bcql", args: ["lsp"] });
  assert.deepStrictEqual(client.clientOptions.initializationOptions, { dialect: "sqlite", schema: "schema.sql" });
  assert.deepStrictEqual(client.clientOptions.documentSelector, [{ language: "buttercupql" }]);
  assert.strictEqual(client.clientOptions.synchronize.configurationSection, "buttercupql");

  // The SQL of a query `def`, with its arguments, as `bcql compile` writes it.
  const view = providers["buttercupql-sql"];
  answer = {
    dialect: "sqlite",
    sql: "SELECT *\nFROM cases\nWHERE operator = ?\nLIMIT ?",
    params: [{ arg: "operator" }, "a,b", { arg: "rows" }],
    args: [
      { name: "operator", type: "text", default: null },
      { name: "rows", type: "a whole number", default: 10 },
    ],
    warnings: [],
  };
  const query = new URLSearchParams({ source: "file:///work/report.bcql", query: "for_operator" }).toString();
  const shown = await view.provideTextDocumentContent({ query });
  assert.deepStrictEqual(requests.pop(), {
    method: "buttercupql/compile",
    params: { textDocument: { uri: "file:///work/report.bcql" }, query: "for_operator" },
  });
  assert.strictEqual(
    shown,
    "SELECT *\nFROM cases\nWHERE operator = ?\nLIMIT ?\n" +
      '-- parameters: [{"arg": "operator"}, "a,b", {"arg": "rows"}]\n' +
      "-- arguments: operator, rows = 10\n" +
      "-- for sqlite\n",
  );

  // A text of several statements shows each, ended with `;` and with its own parameters.
  answer = {
    dialect: "sqlite",
    sql: "BEGIN;\nUPDATE cases SET status = ?\nWHERE risk > 90;\nCOMMIT;",
    params: ["urgent"],
    args: [],
    statements: [
      { sql: "BEGIN", params: [], kind: "begin" },
      { sql: "UPDATE cases SET status = ?\nWHERE risk > 90", params: ["urgent"], kind: "change" },
      { sql: "COMMIT", params: [], kind: "commit" },
    ],
    warnings: ["warning: this deletes every row\n"],
  };
  const script = await view.provideTextDocumentContent({ query: "source=file%3A%2F%2F%2Fwork%2Fscript.bcql" });
  assert.strictEqual(
    script,
    "BEGIN;\n\nUPDATE cases SET status = ?\nWHERE risk > 90;\n" +
      '-- parameters: ["urgent"]\n\n' +
      "COMMIT;\n--\n-- warning: this deletes every row\n-- for sqlite\n",
  );

  // A query that does not compile shows its problems, as comments.
  answer = { dialect: "postgres", problems: ["error: use `==` to compare\n  ┌─ open.bcql:2:14\n"] };
  const failed = await view.provideTextDocumentContent({ query: "source=file%3A%2F%2F%2Fwork%2Fopen.bcql" });
  assert.strictEqual(failed, "-- The query does not compile:\n--\n-- error: use `==` to compare\n--   ┌─ open.bcql:2:14\n");

  // Show SQL opens the view of the file beside it.
  vscode.window.activeTextEditor = {
    document: {
      languageId: "buttercupql",
      uri: { path: "/work/report.bcql", toString: () => "file:///work/report.bcql" },
    },
    selection: { active: {} },
  };
  await commands["buttercupql.showSql"]();
  const viewUri = opened.pop();
  assert.strictEqual(viewUri.scheme, "buttercupql-sql");
  assert.strictEqual(viewUri.path, "/report.sql");
  assert.strictEqual(viewUri.query, "source=file%3A%2F%2F%2Fwork%2Freport.bcql");

  // A copy of the extension with `bcql` in it uses its own.
  const folder = fs.mkdtempSync(path.join(os.tmpdir(), "buttercupql-extension-"));
  fs.mkdirSync(path.join(folder, "bin"));
  const bundled = path.join(folder, "bin", process.platform === "win32" ? "bcql.exe" : "bcql");
  fs.writeFileSync(bundled, "");
  const first = client;
  context.extensionPath = folder;
  await commands["buttercupql.restartServer"]();
  assert.strictEqual(first.stopped, true);
  assert.deepStrictEqual(client.serverOptions.run, { command: bundled, args: ["lsp"] });
  if (process.platform !== "win32") {
    assert.strictEqual(fs.statSync(bundled).mode & 0o777, 0o755, "the bundled bcql can run");
  }
  // And a path in the settings comes first.
  settings["server.path"] = "/opt/bcql/bin/bcql";
  await commands["buttercupql.restartServer"]();
  assert.strictEqual(client.serverOptions.run.command, "/opt/bcql/bin/bcql");
  fs.rmSync(folder, { recursive: true });

  await extension.deactivate();
  assert.strictEqual(client.stopped, true);
  console.log("extension: it starts the server, shows the SQL, and finds its bcql");
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
