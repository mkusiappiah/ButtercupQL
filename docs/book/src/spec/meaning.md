# Meaning

What each form of [the grammar](grammar.md) does. A query works on rows: a list of them, each with the same named
columns, and an order where one has been asked for. Each step takes the rows of the steps before it and gives rows to
the step after it. The meaning is Python's wherever Python has one, and SQL's where Python has none, as for a
comparison with a missing value.

## Texts and statements

A text's statements run in the order they are written: its queries, its changes to rows, and its changes to the
database. Each `create table`, `alter table` and `drop table` changes the tables the statements after it read. The
rows a text gives are those of its query, the one statement that is a query and changes no rows; a text has one at
most, and a module has none. A `with transaction:` block's statements run together, all of them or none.

## Values

### Kinds

A value is a whole number (`int`), a number with a fraction (`float`), an exact decimal (`Decimal`), text (`str`), a
truth (`bool`), a date (`date`), a date and time (`datetime`), a length of time, or missing (`None`). A column's
kind is the one its table gives it, as a schema says, and a value's kind follows from its parts. A value whose kind
no schema says is of a kind not known, and is taken as it is.

A number written in a text is a whole number, or a fraction when it has a point or an exponent; a string is text;
`True` and `False` are truths; `None` is the missing value. Text written in a query is passed to the database as a
parameter, never written into its SQL; numbers, truths, dates and times are written into it. Where a database takes
no parameters, as in a table's default or a stored `def`, text is written into the SQL too, its quotes doubled.

### Numbers

`+`, `-` and `*` are Python's. `/` divides exactly, even two whole numbers: `7 / 2` is `3.5`. `//` divides and rounds
down: `-7 // 2` is `-4`. `%` gives the remainder with the sign of the number divided by: `-7 % 3` is `2`. `**` raises
to a power. Unary `-` and `+` are Python's, and so are `abs()`, `int()`, which drops the fraction, and `float()`.

`round(x)` rounds to a whole number and `round(x, n)` to `n` places, a negative `n` to tens, hundreds and on; as in
Python, a half goes to the even neighbour: `round(2.5)` is `2`, and `round(3.5)` is `4`. A fraction rounded to places
is scaled in the database's own arithmetic, so in the rare case where the scaled fraction is a half only in that
arithmetic, as `round(2.675, 2)` is, the answer is that of Python's `Decimal`, `2.68`, rather than Python's float
`2.67`.

### Text

`+` joins text, and so does an f-string, in which each value in braces is written as text. `len()` counts characters.
`lower()`, `upper()`, `strip()`, `replace()`, `startswith()` and `endswith()` are Python's methods; `x in y`, with both
text, says whether `x` is in `y`. Each minds capitals, and takes its text exactly: `%` and `_` are characters, not
patterns. `str()` writes a value as text.

### Dates and times

`date("2026-10-20")` and `datetime("2026-10-20 14:30")` are a date and a date and time; `today()` and `now()` are
today and now. `days(n)`, `hours(n)`, `minutes(n)`, `seconds(n)` and `weeks(n)` are lengths of time, which add to a
date or a time, or take from it. `.year`, `.month`, `.day`, `.hour`, `.minute` and `.second` are the parts of a
date or a time, as numbers, and `.date()` is the day of a date and time. Dates and times compare in time's order.

### Missing values

- **A comparison with a missing value is neither true nor false**, as in SQL, and so are `not` of it and an `and`
  or an `or` that it decides. A `where` keeps a row only when its condition is true.
- `x == None` and `x is None` hold when `x` is missing; `x != None` and `x is not None` when it is not.
- `x in [a, b]` holds when `x` is one of the values; with `None` among them, when `x` is missing too. `x not in [a,
  b]` holds when `x` is there and is none of them, `None` among them or not.
- Arithmetic, text and dates made from a missing value are missing.
- Missing values sort last, high to low and low to high.

### Truth

Where a condition is wanted, a value is taken by Python's truth: `0`, `0.0`, `""`, `False` and `None` are false,
and every other value is true. A missing value is false, so `not notes` holds where `notes` is missing. `and`, `or`
and `not` are on truths, and give truths.

## Names

A name is, in this order:

1. a parameter of the `def` it is in;
2. a value named before, `threshold = 80`, in the text or imported into it: the name means the value even where a
   column has the name too, so a new column can never change what a query does;
3. a column of the rows at that place, the newest first. When no column has the name exactly, one whose name is the
   same but for capitals is.

A name that is none of these is a mistake, with a suggestion where a name is close to it. A name that is the column of
two tables is a mistake: `table.column` says which. A dotted name is a table's column, `cases.risk`; a link's,
`account.operator`; a module's value or `def`, `shared.recent`; or a part of a date: `raised.year`.

## Steps

**`from`** gives the rows of a table, of a named query, of a call of a query `def`, of a query in brackets, or of
rows written in the text: a list of dicts, whose keys, the same in every row, are the columns. `as` names what it
reads.

**`where`** keeps the rows its condition is true for. After a `group by`, it keeps the groups; after a window
value, it is worked out after the value.

**A column step**, `name = value`, adds a column to every row, or gives one a new value. `name += value` and its kin
are `name = name + value`. The steps after it read the new value.

**`select`** makes the rows' columns those it names, in its order, each named by its `=` or by the column it reads.
Its values read the columns before it, not each other: `select risk = risk * 2, before = risk` gives the old risk as
`before`. Two columns of one name are a mistake.

**`del`** drops the columns it names.

**`sort`** puts the rows in the order of its keys, the first first, each low to high or, after `-`, high to low;
`reverse=True` turns each around. It is stable, as Python's is: rows that tie keep the order they had, so a sort after
another keeps the first for its ties. A key reads the columns at its place: a sort before a `select` that gives a
column a new value sorts by the old one.

**`take n`** keeps the first `n` rows, and **`skip n`** drops them; `n` is a whole number, or a `def`'s parameter.
Steps after them work on the rows they kept: a `take` after a `take` keeps the fewer, and a `where` after a `take`
keeps some of the rows taken.

**`distinct`** drops the rows that repeat one before them in every column.

**`group by`** gives one row for each group of rows with the same values of its columns; rows with a missing value
make a group of their own. The row's columns are the group's values, then the totals of the block, each worked out
over the group's rows. With no block, the rows are the groups' values alone.

**`summarize`** gives one row of totals, worked out over every row, even when there are none.

**`pivot col in [a, b] by key:`** is a `group by key` whose totals are each worked out for each value: a total of the
rows where `col == a`, then of those where `col == b`. The columns are named by the values, or, with several totals,
by the value and the total's name: `a_n`. With no `by`, the totals are of every row, in one row. The values are
written in the text, each a string, a number, a truth or `None`.

**`join source on condition`** pairs each row with each of the source's rows for which the condition is true, and
drops the rows that pair with none. **`left join`** keeps those, with the source's columns missing. The columns of
both can be read, and `table.column` reads one that both have.

**`pass`** does nothing. **A call** of a `def` whose body is steps puts them in its place, its parameters given the
call's arguments.

## Totals

A total is worked out over the rows of a group, in a block of `group by`, `summarize` or `pivot`. `count()` is the
number of rows, and `count(x)` of the values of `x` that are not missing; `count_distinct(x)` is the number of
different values. `sum(x)`, `mean(x)`, which `avg(x)` is too, `min(x)` and `max(x)` leave out missing values.
`any(condition)` says whether the condition is true on some row, and `all(condition)` whether on every row.

Of no rows, or no values, `count()` and `sum()` are `0`, `any()` is `False` and `all()` is `True`, as in Python;
`mean()`, `min()` and `max()` are missing. Each total takes `where=`, a condition of its own: `count(where=status ==
"open")` counts the open rows alone. A total in a block can read the totals before it.

## Window functions

A total with `over=`, `sort=` or `rows=` is a window value: worked out for each row, over a window of rows, and
keeping every row. `over=` names the columns whose values split the rows into groups; `sort=` orders each group; and
`rows=n` makes the window the row and the `n - 1` before it. With `sort=`, a total runs: each row's is of the rows up
to it. `row_number()`, `rank()` and `dense_rank()` number the rows in the order of `sort=`; `lag(x)` and `lead(x)`
are the value of the row before and after, missing at either end; `first(x)` and `last(x)` are the first and last of
the window.

A window value counts the rows the steps before it kept: after a `take 3`, it is worked out over three rows. A
condition on it is worked out after it, so it needs a name of its own first: `place = rank(sort=-risk)`, then
`where place <= 3`.

## Links

A column whose table says it holds another table's key, as SQL's `REFERENCES` does, is a link to that table. The
link is named for the column, without `_id`: `account_id` is the link `account`. `account.operator` reads the
operator of the account a row points to, and a link reads on through the next: `account.bank.name`. Each link is
followed once in a query, keeping every row: a row whose link leads nowhere has its linked values missing.

## Definitions

**A value**, `name = value`, is worked out once, with no rows: a number, a text or a date, or a value made from them.
**A named query**, `name = (from ...)`, is read by `from` and `join` where it is named, and in other queries.

**A `def`** whose body starts with `from` is a query, read by `from` and `join` where it is called: `from
cases_for("A1")`. Otherwise its body is steps, put in place where it is called on a line of a query. A parameter takes
the argument given in its place or by its name, and its default when none is. Compiled on its own, a query `def`'s
parameters are left as placeholders, given values when the query runs. A `def` whose steps change rows is called on
a line of its own, as a statement.

**An import** reads another file's definitions: `import shared` makes them `shared.name`, and `from shared import
recent` makes `recent` one of the text's own names. A module is found by its dotted name, from the text's own folder,
or from its own with `from . import`.

## Combining queries

`a + b` gives the rows of `a`, then those of `b`. `a | b` gives the rows of either, each once; `a & b` those in both,
each once; and `a - b` those of `a` that are not in `b`, each once. The second query's columns are matched to the
first's by their names; two queries with different columns are a mistake.

A named query that reads itself is recursive. Its first part gives the first rows, and names the columns; its second
reads the rows found so far, and gives the next, until it gives no more. With `+`, every row it finds is kept; with
`|`, a row found already is not added again, so the query ends when it finds no new row. A recursive query reads
itself only in its second part.

## Changes to rows

A change is the last step of its query. **`update`** gives the columns it names new values, on the rows the steps
before it choose, each value worked out from the row as it was. **`delete`** removes those rows; with no `where`, it
removes every row, and says so with a warning. **`insert into table`** adds the rows of the steps before it to the
table, each column into the table's column of the same name; the table's other columns take their defaults.

Only `where`, `pass` and calls of `def`s of `where`s can choose the rows an `update` or a `delete` changes: they work
on rows in no order. A condition that follows a link chooses the rows by their keys. A `select` after a change gives
the rows changed, as they are after it.

## Tables

**`create table name:`** makes a table, its columns in the block below it, each written as a Python annotation:
`name: type`. The types are `int`, `float`, `Decimal`, `str`, `bool`, `date`, `datetime` and `bytes`. A column is
never missing unless its type ends with `| None`, or is `Optional[...]`. `= value` gives its default, and
`field()` the rest: `primary_key=True`, `unique=True`, `references=table.column` and `default=`. A line
`primary_key(a, b)` or `unique(a, b)` makes a key of several columns. `exist_ok=True` makes the table only if there
is none of its name.

**`create index on table(keys)`** makes an index of the keys, each low to high or, after `-`, high to low, with
missing values last; **`create unique index`** one in which no two rows have the same values. A name may follow
`index`. `where=` keeps only the rows a condition chooses, and `exist_ok=True` makes it only if there is none of its
name.

**`alter table name:`** changes a table, a change on each line: a column, written as in `create table`, is added;
`rename a to b` renames a column; `del a` drops one. **`drop table`** and **`drop index`** drop a table or an index,
and `missing_ok=True` only if it is there.

**`grant`** lets users or roles do what it names on tables, and **`revoke`** takes it away: `read`; `insert`,
`update` and `delete`; `write`, which is those three; and `all`.

**`@stored`** on the line before a `def` keeps it in the database, to be run by its name: a query `def` as a
function, a macro or a view with parameters, and a `def` that changes rows as a procedure. A stored `def`'s
parameters say their types where its database needs them.

## Mistakes

A text that does not fit the grammar, a name that means nothing, a value of the wrong kind for its place, a step
where it can't be, and a form a database can't run are mistakes. An implementation reports every mistake it finds in a
text, each at its place, and compiles none of a text that has one. A warning is a text that compiles but likely does
not do what was meant, such as a `delete` with no `where`, or a sort that a grouping after it undoes.
