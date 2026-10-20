# The `bcql` command

```text
bcql: the ButtercupQL command

Usage:
  bcql compile [OPTIONS] FILE
  bcql check [OPTIONS] FILE...
  bcql lsp
  bcql --version
  bcql --help

Commands:
  compile    Write the SQL a query compiles to, and its parameters
  check      Read each file and report every mistake in it
  lsp        Run the language server, for editors, on standard input and output

Options:
  --dialect NAME    The database: sqlite, postgres, duckdb or clickhouse
                    [default: postgres]
  --schema FILE     The database's CREATE TABLE statements, to check names against
  --query NAME      Compile a named query or a query def of the file, with the
                    def's parameters left as placeholders
  --params STYLE    Placeholders: dollar ($1), question (?), format (%s),
                    colon (:p1) or braces ({p1:String}) [default: the database's own]
  --json            Write the SQL and its parameters as JSON
  --color WHEN      Colour the messages: auto, always or never [default: auto]

A FILE of `-` is read from standard input. `compile` and `check` exit with 0
when there is no error, with 1 when there is one, and with 2 when the command
is wrong.
```

## `bcql compile`

`bcql compile FILE` writes the SQL of the file's query to standard output, then a comment with the values of its
placeholders, in order, and for a query `def`, its arguments. Problems go to standard error, so the SQL can be piped
on. With `report.bcql` holding the `cases_for` query of [Queries with arguments](reuse.md#queries-with-arguments):

```text
$ bcql compile report.bcql --query cases_for --dialect sqlite
SELECT *
FROM cases
WHERE account_id = ? AND raised > datetime('now', CAST(-? AS TEXT) || ' days')
-- parameters: [{"arg": "account"}, {"arg": "days_back"}]
-- arguments: account, days_back = 30
```

`--json` writes the same as one JSON object, for programs:

```text
$ bcql compile open.bcql --json
{"sql": "SELECT *\nFROM cases\nWHERE status = $1", "params": ["open"], "args": []}
```

## `bcql check`

`bcql check` reads each file given and reports every mistake in them, then says how many it found. A file with a query
is compiled too, for the mistakes only compiling finds, and so are each named query and query `def` of a file of
definitions. It is the command for a CI job: it exits with 1 when any file has an error.

```text
$ bcql check --schema schema.sql queries/*.bcql
checked 12 files: no problems found
```

`--color always` colours the messages for a terminal that `bcql` can't tell is one. Colour is used only in a terminal,
and never when `NO_COLOR` is set.

## `bcql lsp`

`bcql lsp` is the language server: an editor starts it and speaks the Language Server Protocol to it on standard input
and output. See [Editors](editors.md).
