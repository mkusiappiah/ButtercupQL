# Research: making ButtercupQL faster and better than writing SQL

Gathered on 20 October 2026. Each finding links to its source at the end.

## What others have learned

| Project or study | What it shows | What ButtercupQL takes |
|---|---|---|
| Google, *SQL Has Problems. We Can Fix Them: Pipe Syntax In SQL* (VLDB 2024) [1] | SQL's clause order (`SELECT` first, `FROM` later) does not match the order data flows. That one problem forces subqueries, three filter clauses (`WHERE`, `HAVING`, `QUALIFY`) and "inside-out" reading. SQL's meaning, the relational model, is sound. | Steps in data-flow order; one `where`; SQL's meaning kept as it is |
| PRQL [2] | A pipeline language that compiles to many SQL dialects. Its compiler is written in Rust. Putting the source first lets an editor complete column names. | The pipeline; compiling to SQL; Rust |
| DuckDB's "Friendly SQL" [3] | `FROM` first, `GROUP BY ALL` and `SELECT * EXCLUDE` remove the most repeated typing. | `from` first; grouping columns written once; `del` |
| Malloy [4] | Joins declared once in a model, and reusable measures, make queries short. | Links from the schema (v0.2) |
| EdgeQL [5] | Following links with a dot replaces written-out joins and makes queries much shorter. | `account.operator` (v0.2) |
| Preql [6] | A relational language with syntax like Python and JavaScript, compiled to SQL. | Python's expressions; compiling at run time |
| Pony ORM [7] and Ibis [8] | Python expressions become SQL in three steps: syntax tree, a database-neutral plan, then each dialect's SQL. Ibis reaches 20+ backends this way. | The same three steps |
| sqlglot [9] | Each SQL dialect gets its own generator over one shared tree. | A generator per database |
| Jamie Brandon, *Against SQL* [10] | SQL does not compose, and is not a "library language": there is no ecosystem of reusable code. | `def`, named queries and imports |

## Faster to write

1. **The table comes first.** Once `from cases` is written, an editor knows every column before you type one [2][3].
   In SQL, people type `SELECT * FROM t` first and go back, so that completion can work.
2. **Nothing is written twice.** Grouping columns appear once, not in both `SELECT` and `GROUP BY` [3].
3. **One `where` instead of three filter clauses,** and no subqueries for multi-step logic [1].
4. **Dots instead of joins** once links are declared (v0.2) [4][5].
5. **Python's shorthand:** `1 <= risk <= 100`, `x if c else y`, `days(7)`, `del`, `-risk` to sort high to low.
6. **Reuse:** `def` steps and imports, so common logic is written once [10].

Measured on the design's own examples (characters typed, spaces excluded): a grouped query with a condition on its
totals is about a quarter shorter (194 for SQL, 149 for ButtercupQL); a simple filter is only slightly shorter (130 and
118). The larger gains come from completion, no repetition and no subqueries, rather than from fewer characters.

## As fast to run as hand-written SQL

1. **Flat queries and no optimisation fences.** PostgreSQL before version 12 treated every CTE as an optimisation fence.
   Since version 12 it inlines CTEs that are used once and have no side effects, but some conditions still prevent
   inlining [11]. ButtercupQL writes one flat `SELECT` where it can, and inlines named queries itself.
2. **Each database's own forms.** ClickHouse moves selective filters into `PREWHERE` by itself, reading 90% less data in
   some cases. Its `countIf` and `sumIf` keep one counter per group [12]. ButtercupQL writes plain `WHERE` and lets
   ClickHouse decide, and uses the `-If` totals for conditional aggregates.
3. **Parameters, with care.** PostgreSQL reuses plans for parameterised statements. For the first five runs it plans with
   the values, then may switch to a generic plan, and a generic plan cannot use a partial index whose condition names a
   value [13]. ButtercupQL makes values parameters by default, and `literal(x)` writes a value into the SQL where a
   partial index needs it.
4. **A fast compiler.** Written in Rust, compiling takes microseconds, so it can run on every request.

## Better than SQL

1. **Missing values.** `col = NULL` is never true in SQL, and `NOT IN` against a list containing `NULL` returns no rows,
   both silently [14]. ButtercupQL compiles `== None` and `is None` to `IS NULL`, and guards `not in`.
2. **Errors with places and suggestions,** every error at once, with the parser recovering after each [15].
3. **Composition:** named queries, `def` and imports [10].
4. **Safety:** values are always parameters, never pasted into the SQL.

## How it is built

| Part | Choice | Why |
|---|---|---|
| Language | Rust | Memory safety when reading untrusted text, at the speed of C, with Cargo's tooling [16] |
| Parser | `logos` tokens and `chumsky` parsing | `chumsky` keeps going after an error and returns every error with a partial tree [15] |
| Error messages | `ariadne` | Clear messages pointing at the source [15] |
| Python package | PyO3 and maturin | Wheels for Python 3.8+ on Windows, Linux and macOS, published to PyPI [17][18] |
| Browser playground | `wasm-bindgen` | Rust compiled to WebAssembly with JavaScript bindings [19] |
| Editor support | `tower-lsp` | A Language Server Protocol server for VS Code and other editors [20] |
| Name | ButtercupQL, `.bcql` | `buttercup` is a password manager's name on npm [21]; `bql` is another project's crate |

## Sources

1. [SQL Has Problems. We Can Fix Them: Pipe Syntax In SQL (VLDB 2024)](https://vldb.org/pvldb/vol17/p4051-shute.pdf) ·
   [Google Research](https://research.google/pubs/sql-has-problems-we-can-fix-them-pipe-syntax-in-sql/)
2. [PRQL](https://prql-lang.org/) · [PRQL on GitHub](https://github.com/PRQL/prql) ·
   [Autocompletion and type checking in PRQL](https://github.com/PRQL/prql/discussions/4156)
3. [Friendlier SQL with DuckDB](https://duckdb.org/2022/05/04/friendlier-sql) ·
   [DuckDB's Friendly SQL](https://duckdb.org/docs/current/sql/dialect/friendly_sql)
4. [Malloy documentation](https://docs.malloydata.dev/documentation/)
5. [EdgeQL paths](https://docs.geldata.com/reference/reference/edgeql/paths)
6. [Preql](https://github.com/erezsh/Preql)
7. [Pony ORM queries](https://docs.ponyorm.org/queries.html)
8. [Ibis](https://ibis-project.org/)
9. [sqlglot](https://github.com/tobymao/sqlglot)
10. [Against SQL, Jamie Brandon](https://www.scattered-thoughts.net/writing/against-sql/)
11. [PostgreSQL: WITH queries](https://www.postgresql.org/docs/current/queries-with.html) ·
    [PostgreSQL's CTEs are optimisation fences](https://www.enterprisedb.com/blog/postgresqls-ctes-are-optimisation-fences)
12. [ClickHouse: how PREWHERE works](https://clickhouse.com/docs/optimize/prewhere) ·
    [The definitive guide to ClickHouse query optimisation](https://clickhouse.com/resources/engineering/clickhouse-query-optimisation-definitive-guide)
13. [PostgreSQL query planner parameters and prepared statements](https://dev.to/franckpachot/postgresql-query-planner-parameters-and-prepared-statements-3o5a) ·
    [Prepared statements and plan cache pitfalls](https://monpg.app/blog/postgresql-prepared-statements-plan-cache)
14. [Modern SQL: NULL](https://modern-sql.com/concept/null) · [Null (SQL)](https://en.wikipedia.org/wiki/Null_(SQL))
15. [chumsky](https://github.com/zesterer/chumsky)
16. [Rust vs. C++: a modern take on performance and safety](https://thenewstack.io/rust-vs-c-a-modern-take-on-performance-and-safety/)
17. [PyO3 user guide](https://pyo3.rs/)
18. [maturin](https://github.com/PyO3/maturin)
19. [Rust to WebAssembly with wasm-bindgen](https://matthewmacfarquhar.medium.com/programming-webassembly-with-rust-part-3-targeting-js-with-wasm-bindgen-d0ee3c28d18a)
20. [tower-lsp](https://github.com/ebkalderon/tower-lsp)
21. [Buttercup password manager](https://github.com/buttercup)
