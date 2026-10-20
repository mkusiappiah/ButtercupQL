# Editors

`bcql lsp` is ButtercupQL's language server. An editor that speaks the Language Server Protocol starts it, and it:

- **shows each problem as you type,** at its place with its message, as `bcql check` finds them. Every query of a file
  is checked: the main query, each named query and each query `def`. A problem in an imported module is shown on the
  import that reads it, with a link to its place in the module.
- **completes names:** the steps and statements at the start of a line, and `stored` after `@`; the tables after
  `from`, `join`, `insert into`, `alter table`, `drop table` and `on`, those the file creates too; functions, the
  file's own definitions and, with a schema, the columns and links of the tables read; and after a dot, a table's or
  a link's columns, a module's names, or the methods.
- **describes a name** when the pointer rests on it: a step, a statement, a function, a column with its type, a
  link, or a `def` with its docstring.
- **goes to a definition,** in the file or in a module it imports.
- **lists a file's definitions,** for the editor's outline: its `def`s, named queries and values, the tables it
  creates, and each change to rows, such as `insert into archive`.
- **gives the SQL of a text** for the editor to show, with the `buttercupql/compile` request: its SQL and parameters,
  and each statement of a text of several.

It reads the files a file imports as the editor has them, saved or not.

## Visual Studio Code

The ButtercupQL extension adds colours to `.bcql` files, runs the language server, and adds **Show SQL**: the SQL of the
query beside the file, kept up to date as you type. With the cursor in a query `def` or a named query, it shows that
one, its arguments left as placeholders. A file of several statements shows each, ended with `;` and with its own
parameters.

Each [release](https://github.com/mkusiappiah/ButtercupQL/releases) has the extension: one file for each platform,
with `bcql` in it, and one for any platform, which uses the `bcql` on your `PATH`. Install it with
**Extensions: Install from VSIX...**, or:

```sh
code --install-extension buttercupql-darwin-arm64-0.2.0.vsix
```

### Settings

| Setting | Default | What it does |
|---|---|---|
| `buttercupql.dialect` | `postgres` | The database: `postgres`, `sqlite`, `duckdb` or `clickhouse`. |
| `buttercupql.schema` | none | A file of the database's `CREATE TABLE` statements. Names are checked against it and completed from it. A relative path is read from the workspace's folder. |
| `buttercupql.server.path` | none | The `bcql` command to run, when it is not the one that comes with the extension or the one on the `PATH`. |

## Other editors

Any editor with a language server client can use `bcql lsp`: give it the command `bcql lsp` for files ending in
`.bcql`. The server takes two settings, when it starts (as its initialization options) or when they change
(`workspace/didChangeConfiguration`), on their own or under `"buttercupql"`:

```json
{ "dialect": "sqlite", "schema": "schema.sql" }
```

For example, in Neovim:

```lua
vim.filetype.add({ extension = { bcql = "buttercupql" } })
vim.lsp.config("buttercupql", {
  cmd = { "bcql", "lsp" },
  filetypes = { "buttercupql" },
  root_markers = { ".git" },
  init_options = { dialect = "postgres", schema = "schema.sql" },
})
vim.lsp.enable("buttercupql")
```

and in Helix's `languages.toml`:

```toml
[language-server.bcql]
command = "bcql"
args = ["lsp"]
config = { dialect = "postgres", schema = "schema.sql" }

[[language]]
name = "buttercupql"
scope = "source.buttercupql"
file-types = ["bcql"]
comment-token = "#"
language-servers = ["bcql"]
```

The server counts places in a line in UTF-16, as the protocol says, or in UTF-8 when the editor offers it.
