# Examples

Queries from the [language design](../docs/design.md). Beside each is the SQL it compiles to for each database, with
the parameters at the end: `open_critical.sqlite.sql`, `open_critical.postgres.sql`, `open_critical.duckdb.sql` and
`open_critical.clickhouse.sql`. Every change to the compiler is checked against them, and the playground offers each
example, compiled with [its schema](../playground/schema.sql).

When the compiler changes what it writes on purpose, `BCQL_BLESS=1 cargo test -p buttercupql --test snapshots` writes
the new SQL. Read the changes before committing them.
