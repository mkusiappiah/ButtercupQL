# The playground

**[Open the playground](https://mkusiappiah.github.io/ButtercupQL/playground/)** to write ButtercupQL in your browser and
see the SQL it compiles to as you type.

- **Choose the database** at the top: PostgreSQL, SQLite, DuckDB or ClickHouse.
- **Each mistake is underlined,** and its message shown in place of the SQL, as `bcql check` shows it.
- **The schema** below the query checks names, and lets links be followed. It starts with the tables of
  [Schemas](schemas.md); change them, or clear them to compile without a schema.
- **A query `def` or a named query** can be compiled on its own: choose it beside the SQL, and see its arguments.
- **A text of several statements,** such as the example that changes data, shows each statement's SQL, and the
  parameters of each.
- **Share** copies a link with the query, the database and the schema in it.

The compiler runs in the browser: it is ButtercupQL's own compiler, built for WebAssembly. Nothing you write is sent
anywhere; a shared link holds the query in the part of the address after `#`, which browsers do not send.
