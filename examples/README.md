# Examples

Queries from the [language design](../docs/design.md). Beside each is the SQL it compiles to, for SQLite and for
PostgreSQL: `open_critical.sqlite.sql` and `open_critical.postgres.sql`, with the parameters at the end. Every change is
checked against them, and every example is run on both databases.

When the compiler changes what it writes on purpose, `BCQL_BLESS=1 cargo test -p buttercupql --test snapshots` writes
the new SQL. Read the changes before committing them.
