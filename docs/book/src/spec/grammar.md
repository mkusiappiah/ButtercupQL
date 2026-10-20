# Grammar

The grammar of a ButtercupQL text, from a file down to an atom. [The specification](index.md) says how to read it,
and [Lexical structure](lexical.md) what its tokens are. `NEWLINE` is the end of a line or a `;`.

## Files

A file is its docstring, its imports, and then its definitions and statements, in any order. A file with no
statements is a module: other files import its definitions.

```ebnf
file           = [ docstring ] { import } { definition | statement } ;
docstring      = STRING NEWLINE ;

import         = "import" module [ "as" NAME ] { "," module [ "as" NAME ] } NEWLINE
               | "from" { "." } [ module ] "import" imported NEWLINE ;
imported       = import_names | "(" import_names [ "," ] ")" ;
import_names   = NAME [ "as" NAME ] { "," NAME [ "as" NAME ] } ;
module         = dotted ;
dotted         = NAME { "." NAME } ;
```

## Definitions

```ebnf
definition     = value | def ;
value          = NAME "=" expression NEWLINE ;
def            = { decorator } "def" NAME "(" [ parameters ] ")" [ "->" type ]
                 ":" def_body ;
decorator      = "@" NAME NEWLINE ;
parameters     = parameter { "," parameter } [ "," ] ;
parameter      = NAME [ ":" type ] [ "=" expression ] ;
type           = ( dotted | "None" ) [ "[" type { "," type } "]" ] ;
def_body       = step { NEWLINE step } NEWLINE
               | NEWLINE INDENT [ docstring ] step NEWLINE { step NEWLINE } DEDENT ;
```

A `value` whose expression is a query in brackets is a named query.

## Statements

```ebnf
statement      = query
               | call NEWLINE
               | create_table | create_index | alter_table | drop
               | grant | revoke | transaction ;

query          = from_step NEWLINE { step NEWLINE } ;

create_table   = "create" "table" dotted { option } ":" NEWLINE
                 INDENT table_line NEWLINE { table_line NEWLINE } DEDENT ;
table_line     = column | ( "primary_key" | "unique" ) "(" NAME { "," NAME } ")" ;
column         = NAME ":" type [ "|" "None" ] [ "=" expression ] ;
option         = "," NAME "=" expression ;

create_index   = "create" [ "unique" ] "index" [ NAME ] "on" dotted
                 "(" sort_key { "," sort_key } ")" { option } NEWLINE ;

alter_table    = "alter" "table" dotted ":" NEWLINE
                 INDENT table_change NEWLINE { table_change NEWLINE } DEDENT ;
table_change   = column | "del" NAME | "rename" NAME "to" NAME ;

drop           = "drop" ( "table" | "index" ) dotted { option } NEWLINE ;

grant          = "grant" permissions "on" tables "to" roles NEWLINE ;
revoke         = "revoke" permissions "on" tables "from" roles NEWLINE ;
permissions    = NAME { "," NAME } ;
tables         = dotted { "," dotted } ;
roles          = NAME { "," NAME } ;

transaction    = "with" "transaction" ":" NEWLINE
                 INDENT statement { statement } DEDENT ;
```

## Steps

A step is on a line of its own, or after a `;`. A word that names a step is a step only where a step starts, and
not when an `=` follows it, which makes it a column.

```ebnf
step           = from_step | where_step | column_step | select_step
               | del_step | sort_step | take_step | skip_step
               | distinct_step | group_step | summarize_step
               | pivot_step | join_step | call_step | "pass" | change ;
change         = update_step | delete_step | insert_step ;

from_step      = "from" source ;
source         = ( dotted | call | "(" query ")" | list ) [ "as" NAME ] ;
where_step     = "where" expression ;
column_step    = NAME ( "=" | update_operator ) expression ;
select_step    = "select" fields ;
del_step       = "del" dotted { "," dotted } ;
sort_step      = "sort" sort_key { "," sort_key } [ "," "reverse" "=" expression ] ;
sort_key       = [ "-" ] expression ;
take_step      = "take" expression ;
skip_step      = "skip" expression ;
distinct_step  = "distinct" ;
group_step     = "group" "by" fields [ ":" totals ] ;
summarize_step = "summarize" ":" totals ;
pivot_step     = "pivot" sum "in" ( list | tuple ) [ "by" fields ] ":" totals ;
join_step      = [ "left" ] "join" source "on" expression ;
call_step      = call ;

update_step    = "update" assignment { "," assignment } ;
assignment     = NAME ( "=" | update_operator ) expression ;
update_operator = "+=" | "-=" | "*=" | "/=" | "//=" | "%=" | "**=" ;
delete_step    = "delete" ;
insert_step    = "insert" "into" dotted ;

fields         = field { "," field } [ "," ] | "(" field { "," field } [ "," ] ")" ;
field          = [ NAME "=" ] expression ;
totals         = total { NEWLINE total } NEWLINE
               | NEWLINE INDENT total NEWLINE { total NEWLINE } DEDENT ;
total          = NAME "=" expression ;
call           = dotted "(" [ arguments ] ")" ;
```

The first step of a query is its `from`, and a query has no other. A change ends its query: only a `select` can
follow it, which gives the rows changed, and a new statement starts after it. Before an `update` or a `delete`, only
`where`, `pass` and calls can follow the `from`. A `def`'s body is a query when its first step is a `from`, and steps
to put in place otherwise.

## Expressions

The forms bind as Python's do, from the loosest to the tightest.

```ebnf
expression     = disjunction [ "if" disjunction "else" expression ] ;
disjunction    = conjunction { ( "or" | "|" ) conjunction } ;
conjunction    = inversion { ( "and" | "&" ) inversion } ;
inversion      = "not" inversion | comparison ;
comparison     = sum { comparator sum } ;
comparator     = "==" | "!=" | "<" | "<=" | ">" | ">="
               | "in" | "not" "in" | "is" | "is" "not" ;
sum            = term { ( "+" | "-" ) term } ;
term           = factor { ( "*" | "/" | "//" | "%" ) factor } ;
factor         = ( "-" | "+" ) factor | power ;
power          = primary [ "**" factor ] ;
primary        = atom { "." NAME | "(" [ arguments ] ")" } ;
arguments      = argument { "," argument } [ "," ] ;
argument       = [ NAME "=" ] expression ;

atom           = NAME | NUMBER | strings | "True" | "False" | "None"
               | "(" expression ")" | tuple | list | set | dict | "(" query ")" ;
strings        = ( STRING | FSTRING ) { STRING | FSTRING } ;
tuple          = "(" [ expression "," [ expression { "," expression } [ "," ] ] ] ")" ;
list           = "[" [ expression { "," expression } [ "," ] ] "]" ;
set            = "{" expression { "," expression } [ "," ] "}" ;
dict           = "{" expression ":" expression
                 { "," expression ":" expression } [ "," ] "}" ;
```

`|` and `&` stand between two queries alone: queries in brackets, or the names of named queries. `+` and `-` between
two queries combine them too. A comparison chains, as in Python: `a < b <= c` is `a < b and b <= c`, with `b` read
once.

Python's other forms are refused, each with a message that says what to write: comprehensions, subscripts such as
`name[0]`, `lambda`, `:=`, `*` and `**` in arguments and parameters, `/` and `*` among parameters, and a parameter
without a default after one with a default.
