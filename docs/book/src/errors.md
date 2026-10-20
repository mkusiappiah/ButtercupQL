# Mistakes and their messages

ButtercupQL reports every mistake in a file at once, each at its place, with what to write instead. `bcql check`
shows them as below, an editor shows them as you type, and the Python package raises them together as one
`CompileError`.

```python mistake
from cases
where status = "open"
order by risk desc
limit 20
```

```text
error: use `==` to compare
  ┌─ query.bcql:2:14
  │
2 │ where status = "open"
  │              ^ `=` gives a column a value; `==` compares two values

error: there is no step `order`
  ┌─ query.bcql:3:1
  │
3 │ order by risk desc
  │ ^^^^^ ButtercupQL calls this `sort`
  = help: for example: `sort -risk, raised`

error: there is no step `limit`
  ┌─ query.bcql:4:1
  │
4 │ limit 20
  │ ^^^^^ ButtercupQL calls this `take`
  = help: for example: `take 20`

checked 1 file: 3 errors
```

## Habits from other languages

Words from SQL and from data-frame libraries get their ButtercupQL form: `order by` and `sort_values` are `sort`,
`limit` and `head` are `take`, `having` and `filter` are `where`, `groupby` is `group by`, `agg` is `summarize`, and
`mutate` and `assign` are a column step. SQL's operators are caught too:

```python mistake
from cases
where risk > 90 && band <> "low"
sort risk desc
```

```text
error: use `and`
  ┌─ query.bcql:2:17
  │
2 │ where risk > 90 && band <> "low"
  │                 ^^ `&&` is not how ButtercupQL writes “and”

error: use `!=` for “not equal to”
  ┌─ query.bcql:2:25
  │
2 │ where risk > 90 && band <> "low"
  │                         ^^

error: there is no `desc` in ButtercupQL
  ┌─ query.bcql:3:11
  │
3 │ sort risk desc
  │           ^^^^ write `-risk` to sort high to low

checked 1 file: 3 errors
```

## Misspelt names

A name that is not known gets the one most likely meant, when one is close: a step, a function, a column of the schema,
or a definition of the file.

```python mistake
from cases
wehre risk > 90
```

```text
error: there is no step `wehre`
  ┌─ query.bcql:2:1
  │
2 │ wehre risk > 90
  │ ^^^^^ did you mean `where`?

checked 1 file: 1 error
```

```python mistake
from cases
level = rond(risk)
```

```text
error: there is no function `rond`
  ┌─ query.bcql:2:9
  │
2 │ level = rond(risk)
  │         ^^^^ did you mean `round`?

checked 1 file: 1 error
```

A mistake in how a line is written stops the file from compiling, so the mistakes that only compiling finds, such as a
misspelt function, are shown once the lines read.

## Mistakes that would give wrong rows

Some SQL runs, but does not do what it says. ButtercupQL refuses to write it:

```python mistake
from transactions
total = sum(amount)
```

```text
error: `sum()` is a total
  ┌─ query.bcql:2:9
  │
2 │ total = sum(amount)
  │         ^^^^^^^^^^^ a total is worked out from many rows
  = help: it goes in the block of a `group by` or a `summarize`:

              summarize:
                  n = count()
  = help: to keep every row with its total beside it, give the total a window: `sum(..., over=operator)`

checked 1 file: 1 error
```

A step whose work a later step undoes is a warning: the query runs, but likely not as meant.

```python mistake
from cases
sort -risk
group by operator:
    n = count()
where n > 100
summarize:
    operators = count()
```

```text
warning: this sort has no effect
  ┌─ query.bcql:2:1
  │
2 │ sort -risk
  │ ^^^^^^^^^^ grouping does not keep the rows' order
  = help: to order the groups, sort after the grouping

checked 1 file: 1 warning
```
