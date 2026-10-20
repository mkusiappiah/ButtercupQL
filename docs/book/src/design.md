# Design and research

ButtercupQL's design is written down in two documents in the repository:

- **[The language design](https://github.com/mkusiappiah/ButtercupQL/blob/main/docs/design.md)** says what each form
  means and why, and records each decision made while building it.
- **[The research](https://github.com/mkusiappiah/ButtercupQL/blob/main/docs/research.md)** looks at the languages
  that came before it, and at how to make a query faster to write than SQL and as fast to run.

## Goals

1. **Read like Python.** Indentation, `#` comments, and Python's own expressions and values.
2. **One step per line, in the order the data flows.** Each step takes rows and gives rows.
3. **Faster to write than SQL, and never slower to run.** The compiler writes the SQL a careful person would write.
4. **Compile to SQL; never replace the database.**
5. **Safe by default.** Every value becomes a parameter, so a query can never inject SQL, and names are checked
   against the schema when one is given.

## Not yet

- Writing data (`insert`, `update`, `delete`) and creating tables.
- `match`, recursive queries, pivots, and subtracting one date from another.

The [changelog](https://github.com/mkusiappiah/ButtercupQL/blob/main/CHANGELOG.md) lists what each version added.
