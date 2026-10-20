# The specification

This part of the book defines ButtercupQL: what a text must be to be one, and what it means. It is written for those
who implement the language, and for anyone who needs to know exactly what a query does. The chapters before it teach
the language by example; where one of them and this part disagree, this part is the language.

It describes ButtercupQL 0.2, the language of the compiler on the repository's `main` branch.

- [Lexical structure](lexical.md): how a text is split into tokens, lines and blocks.
- [Grammar](grammar.md): the forms of files, statements, steps and expressions.
- [Meaning](meaning.md): what each form does: values and their types, missing values, each step, totals and
  windows, links, definitions, changes to rows, tables, and scripts.
- [The databases](databases.md): how each database is given the meaning, and what each can't do.

## Conformance

An implementation of ButtercupQL conforms when:

1. it takes every text this part allows, and refuses every text it does not, with an error at the place of each
   mistake it finds;
2. for a text it takes, the rows it gives on each database it supports are the rows the text's meaning gives;
3. it refuses a text that asks a database for something the database can't do, rather than doing something else.

The [conformance suite](https://github.com/mkusiappiah/ButtercupQL/tree/main/conformance) holds cases that check
these, each a text and the rows or the errors it gives. ButtercupQL's own tests run every case on SQLite,
PostgreSQL, DuckDB and ClickHouse. The words of the messages are an implementation's own; the suite's are those of
ButtercupQL's compiler.

## Notation

The grammar is written in EBNF, with these readings:

| Written | Means |
|---|---|
| `"where"` | The token written so. A word in quotes that is no keyword is a name with that text. |
| `a b` | `a`, then `b`. |
| `a \| b` | `a` or `b`. |
| `[ a ]` | `a`, or nothing. |
| `{ a }` | `a` any number of times, none too. |
| `( a )` | `a`, as one. |
| `NAME`, `NUMBER`, `STRING`, `FSTRING` | The tokens of [Lexical structure](lexical.md). |
| `NEWLINE`, `INDENT`, `DEDENT` | The end of a line, the start of a block, and its end. |

A rule's name is in lowercase: `step`, `expression`. Where the grammar alone would take a text that is still a
mistake, such as a column with two names, [Meaning](meaning.md) says so.
