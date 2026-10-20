// The ButtercupQL extension. It runs `bcql lsp`, the language server, which shows each problem as it is typed,
// completes and describes names and goes to where one is defined, and it shows the SQL a file compiles to beside it.

"use strict";

const fs = require("fs");
const path = require("path");
const vscode = require("vscode");
const { LanguageClient } = require("vscode-languageclient/node");

/** The scheme of the documents that show a file's SQL. */
const SQL_SCHEME = "buttercupql-sql";

let client;
let output;

/** The `bcql` command: the one set in the settings, the one that comes with the extension, or the one on the PATH. */
function serverCommand(context) {
  const configured = vscode.workspace.getConfiguration("buttercupql").get("server.path");
  if (configured) {
    return configured;
  }
  const bundled = path.join(context.extensionPath, "bin", process.platform === "win32" ? "bcql.exe" : "bcql");
  if (!fs.existsSync(bundled)) {
    return "bcql";
  }
  if (process.platform !== "win32") {
    // Unpacking the extension can lose the program's permission to run.
    try {
      fs.chmodSync(bundled, 0o755);
    } catch {
      // A read-only install: it runs if it already could.
    }
  }
  return bundled;
}

function settings() {
  const config = vscode.workspace.getConfiguration("buttercupql");
  return { dialect: config.get("dialect"), schema: config.get("schema") };
}

async function start(context) {
  const command = serverCommand(context);
  const server = { command, args: ["lsp"] };
  client = new LanguageClient(
    "buttercupql",
    "ButtercupQL",
    { run: server, debug: server },
    {
      documentSelector: [{ language: "buttercupql" }],
      initializationOptions: settings(),
      outputChannel: output,
      synchronize: {
        // Settings are sent as `{"buttercupql": {...}}` when they change.
        configurationSection: "buttercupql",
        // A schema or a module saved outside the editor changes what the open files find.
        fileEvents: vscode.workspace.createFileSystemWatcher("**/*.{bcql,sql}"),
      },
    },
  );
  try {
    await client.start();
  } catch (error) {
    client = undefined;
    const install = "How to install bcql";
    const chosen = await vscode.window.showErrorMessage(
      `ButtercupQL could not start its language server, \`${command} lsp\`: ${error.message}. ` +
        "Install the `bcql` command, or set `buttercupql.server.path` to it.",
      install,
    );
    if (chosen === install) {
      vscode.env.openExternal(vscode.Uri.parse("https://mkusiappiah.github.io/ButtercupQL/editors.html"));
    }
  }
}

async function stop() {
  if (client) {
    const stopping = client;
    client = undefined;
    await stopping.stop();
  }
}

/** The address of the document that shows the SQL of a file, or of one of its queries. */
function sqlUri(source, query) {
  const name = path.posix.basename(source.path).replace(/\.bcql$/, "") + (query ? `.${query}` : "") + ".sql";
  const params = new URLSearchParams({ source: source.toString() });
  if (query) {
    params.set("query", query);
  }
  return vscode.Uri.from({ scheme: SQL_SCHEME, path: `/${name}`, query: params.toString() });
}

/** A text as SQL comments. */
function commented(text) {
  return text
    .trimEnd()
    .split("\n")
    .map((line) => (line ? `-- ${line}` : "--"))
    .join("\n");
}

/** A placeholder's value as `bcql compile` writes it: the value, or the argument that gives it. */
function shownParam(param) {
  if (param !== null && typeof param === "object") {
    return `{"arg": ${JSON.stringify(param.arg)}}`;
  }
  return JSON.stringify(param);
}

/** The SQL a file compiles to, written as `bcql compile` writes it. */
class SqlView {
  constructor() {
    this.changed = new vscode.EventEmitter();
    this.onDidChange = this.changed.event;
  }

  async provideTextDocumentContent(uri) {
    const params = new URLSearchParams(uri.query);
    const source = params.get("source");
    const query = params.get("query");
    if (!client) {
      return "-- The language server is not running: the SQL can't be shown.";
    }
    const request = { textDocument: { uri: source } };
    if (query) {
      request.query = query;
    }
    const compiled = await client.sendRequest("buttercupql/compile", request);
    if (!compiled) {
      return "-- Open the file to see its SQL.";
    }
    if (compiled.problems) {
      return `-- The query does not compile:\n--\n${compiled.problems.map(commented).join("\n--\n")}\n`;
    }
    const lines = [];
    const statements = compiled.statements || [];
    if (statements.length > 1) {
      // Several statements, each ended with `;` and with its own parameters: they run one by one.
      statements.forEach((statement, n) => {
        if (n > 0) {
          lines.push("");
        }
        lines.push(`${statement.sql};`);
        if (statement.params.length > 0) {
          lines.push(`-- parameters: [${statement.params.map(shownParam).join(", ")}]`);
        }
      });
    } else {
      lines.push(compiled.sql);
      if (compiled.params.length > 0) {
        lines.push(`-- parameters: [${compiled.params.map(shownParam).join(", ")}]`);
      }
      if (compiled.args.length > 0) {
        const args = compiled.args.map((arg) => (arg.default === null ? arg.name : `${arg.name} = ${JSON.stringify(arg.default)}`));
        lines.push(`-- arguments: ${args.join(", ")}`);
      }
    }
    for (const warning of compiled.warnings) {
      lines.push("--", commented(warning));
    }
    lines.push(`-- for ${compiled.dialect}`);
    return lines.join("\n") + "\n";
  }
}

/** The query `def` or named query the cursor is in, by the server's list of the file's definitions. */
async function queryAt(editor) {
  const symbols = await vscode.commands.executeCommand("vscode.executeDocumentSymbolProvider", editor.document.uri);
  const cursor = editor.selection.active;
  const around = (symbols || []).find(
    (symbol) => symbol.range.contains(cursor) && (symbol.detail === "a query" || symbol.detail === "a named query"),
  );
  return around ? around.name : undefined;
}

function activate(context) {
  output = vscode.window.createOutputChannel("ButtercupQL");
  const view = new SqlView();
  const shown = new Map();
  let refresh;

  context.subscriptions.push(
    output,
    vscode.workspace.registerTextDocumentContentProvider(SQL_SCHEME, view),
    vscode.commands.registerCommand("buttercupql.showSql", async () => {
      const editor = vscode.window.activeTextEditor;
      if (!editor || editor.document.languageId !== "buttercupql") {
        vscode.window.showInformationMessage("Open a ButtercupQL file to see its SQL.");
        return;
      }
      const uri = sqlUri(editor.document.uri, await queryAt(editor));
      shown.set(uri.toString(), editor.document.uri.toString());
      const document = await vscode.workspace.openTextDocument(uri);
      await vscode.languages.setTextDocumentLanguage(document, "sql");
      await vscode.window.showTextDocument(document, {
        viewColumn: vscode.ViewColumn.Beside,
        preserveFocus: true,
        preview: true,
      });
    }),
    vscode.commands.registerCommand("buttercupql.restartServer", async () => {
      await stop();
      await start(context);
    }),
    // The SQL shown follows the files as they are edited. A file can change another's SQL by being imported, so
    // every view is refreshed.
    vscode.workspace.onDidChangeTextDocument((event) => {
      if (event.document.languageId !== "buttercupql") {
        return;
      }
      clearTimeout(refresh);
      refresh = setTimeout(() => {
        for (const uri of shown.keys()) {
          view.changed.fire(vscode.Uri.parse(uri));
        }
      }, 200);
    }),
    vscode.workspace.onDidCloseTextDocument((document) => {
      if (document.uri.scheme === SQL_SCHEME) {
        shown.delete(document.uri.toString());
      }
    }),
    vscode.workspace.onDidChangeConfiguration(async (event) => {
      if (event.affectsConfiguration("buttercupql.server.path")) {
        await stop();
        await start(context);
      }
      if (event.affectsConfiguration("buttercupql")) {
        for (const uri of shown.keys()) {
          view.changed.fire(vscode.Uri.parse(uri));
        }
      }
    }),
  );
  return start(context);
}

function deactivate() {
  return stop();
}

module.exports = { activate, deactivate };
