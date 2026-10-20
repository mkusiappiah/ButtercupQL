# Functions and methods

Everything ButtercupQL has built in. An editor shows these descriptions as you type, from the same list.

## Steps

| Written | Does |
|---|---|
| `from cases` | Start from a table, a view or a named query. |
| `where condition` | Keep the rows where the condition is true. After `group by`, it keeps groups by their totals. |
| `select a, b, total = x + y` | Keep these columns, in this order, naming any computed ones. |
| `del a, b` | Drop these columns. |
| `sort -risk, raised` | Order the rows. `-` sorts a column high to low; missing values come last either way. |
| `take 20` | Keep the first 20 rows. |
| `skip 40` | Leave out the first 40 rows. |
| `distinct` | Drop repeated rows. |
| `group by operator:` | One row for each group, with the totals of the indented block below. |
| `summarize:` | One row of totals for all the rows, from the indented block below. |
| `join accounts on account_id == accounts.id` | Add another table's columns to the rows that match. |
| `left join accounts on account_id == accounts.id` | Add another table's columns, keeping the rows that have no match. |
| `update status = "closed", risk += 1` | Change the rows the steps before chose: give columns new values. A `select` after it gives the rows changed. |
| `delete` | Delete the rows the steps before chose. |
| `insert into archive` | Add the rows of the steps before to a table, its columns matched by name. |
| `pivot band in ["critical", "high"] by operator:` | One row for each group, with a column of each total of the block below for each value. |
| `pass` | Do nothing, as in Python: a block with no steps yet. |

A step that adds a column, `name = expression`, has no word of its own.

## Statements

Each of these is a statement of its own, on a line of its own: a change to the database itself. See
[Changing data](changing-data.md) and [Tables and permissions](tables.md).

| Written | Does |
|---|---|
| `create table cases:` | Create a table, its columns written as Python annotations below: `risk: int | None`. `create index on cases(-risk)` creates an index. |
| `alter table cases:` | Change a table: add a column written as in `create table`, `del` one, or `rename old to new`. |
| `drop table archive` | Drop a table, or with `drop index`, an index. |
| `grant read on cases to analyst` | Let users or roles read or write tables: `read`, `write`, `insert`, `update`, `delete` or `all`. |
| `revoke write on cases from analyst` | Take permissions away again. |
| `with transaction:` | Run the statements below together: all of them, or none if one fails. |

A `def` written with `@stored` on the line before it is kept in the database: see [Stored defs](tables.md#stored-defs).

## Totals

Totals work over the rows of a group, in a `group by` or `summarize` block. Given `over=`, `sort=` or `rows=`, they work
over a window of rows instead, and keep every row: see [Window functions](windows.md). Each can take `where=`, a
condition of its own.

| Written | Does |
|---|---|
| `count(), count(x, where=condition)` | A total: the number of rows, or of the rows where `x` is not missing. `where=` counts only the rows where the condition is true. |
| `count_distinct(x, where=condition)` | A total: the number of different values of `x`, leaving out missing ones. |
| `sum(x, where=condition)` | A total: the sum of `x`. The sum of no rows is 0, as in Python. |
| `mean(x, where=condition)` | A total: the mean of `x`. The mean of no rows is missing. |
| `avg(x, where=condition)` | The same as `mean(x)`. |
| `any(condition)` | A total: whether the condition is true on any row. For no rows, False. |
| `all(condition)` | A total: whether the condition is true on every row. For no rows, True. |

## Window functions

| Written | Does |
|---|---|
| `row_number(over=group, sort=order)` | A window function: numbers the rows 1, 2, 3 in the order of `sort=`, starting again for each group of `over=`. |
| `rank(over=group, sort=order)` | A window function: each row's place in the order of `sort=`. Rows that tie share a place, and leave a gap after it: 1, 1, 3. |
| `dense_rank(over=group, sort=order)` | A window function: as `rank()`, without gaps: 1, 1, 2. |
| `lag(x, n=1, default=None, over=group, sort=order)` | A window function: the value of `x` on the row `n` rows before, in the order of `sort=`, or `default=` when there is none. |
| `lead(x, n=1, default=None, over=group, sort=order)` | A window function: the value of `x` on the row `n` rows after, in the order of `sort=`, or `default=` when there is none. |
| `first(x, over=group, sort=order)` | A window function: the value of `x` on the first row of its group, in the order of `sort=`. |
| `last(x, over=group, sort=order)` | A window function: the value of `x` on the last row of its group, in the order of `sort=`. |

## Values

| Written | Does |
|---|---|
| `min(x), min(a, b, ...)` | Given one value, a total: the smallest `x`. Given several, the smallest of them on each row. |
| `max(x), max(a, b, ...)` | Given one value, a total: the largest `x`. Given several, the largest of them on each row. |
| `len(text)` | The number of characters in the text. |
| `abs(x)` | The number without its sign. |
| `round(x), round(x, places)` | The number rounded to a whole number, or to `places` decimal places. |
| `str(x)` | The value as text. |
| `int(x)` | The value as a whole number. A fraction is dropped, as Python's `int()` drops it. |
| `float(x)` | The value as a number with a fraction. |
| `literal(x)` | Writes a value into the SQL rather than passing it as a parameter, for a partial index that needs to see it. |

## Dates and times

| Written | Does |
|---|---|
| `now()` | The date and time now. |
| `today()` | Today's date. |
| `date("2026-10-20")` | A date, written as text: year, month and day. |
| `datetime("2026-10-20 14:30")` | A date and time, written as text. |
| `days(n)` | A length of time of `n` days, to add to a date or take from it: `now() - days(7)`. |
| `hours(n)` | A length of time of `n` hours. |
| `minutes(n)` | A length of time of `n` minutes. |
| `seconds(n)` | A length of time of `n` seconds. |
| `weeks(n)` | A length of time of `n` weeks. |

## Methods

| Written | Does |
|---|---|
| `text.lower()` | The text in small letters. |
| `text.upper()` | The text in capitals. |
| `text.strip(), text.strip(characters)` | The text without spaces at either end, or without the characters given. |
| `text.lstrip(), text.lstrip(characters)` | The text without spaces at its start, or without the characters given. |
| `text.rstrip(), text.rstrip(characters)` | The text without spaces at its end, or without the characters given. |
| `text.startswith(prefix)` | Whether the text starts with the prefix, matching capitals exactly. An index can serve it. |
| `text.endswith(suffix)` | Whether the text ends with the suffix, matching capitals exactly. |
| `text.replace(old, new)` | The text with every `old` in it replaced by `new`. |
| `moment.date()` | The day of a date and time. |

## Parts of a date

| Written | Does |
|---|---|
| `moment.year` | The year, as a number. |
| `moment.month` | The month, as a number from 1 to 12. |
| `moment.day` | The day of the month, as a number. |
| `moment.hour` | The hour, as a number from 0 to 23. |
| `moment.minute` | The minute, as a number. |
| `moment.second` | The second, as a number. |
