# Lexical structure

A text is split into tokens: names, keywords, numbers, strings, operators and brackets. The end of a line, and where
a line starts, give the text its shape.

## Characters

A text is Unicode, written in UTF-8. A byte order mark at its start is left out. A line ends with `\n`, and `\r\n`
is the same. The characters that change the order text is shown in, U+202A to U+202E and U+2066 to U+2069, are
refused anywhere in a text, in its strings and comments too: they can make a query read differently from what it
does.

## Space and comments

Spaces, tabs and form feeds separate tokens and are otherwise left out. `#` starts a comment, which runs to the end
of its line. A `\` as the last character of a line joins the next line to it, as one line; a `\` anywhere else
outside a string is a mistake.

## Lines

A line's first token starts something: a step, a statement or a definition. Each ends at the end of its line, or at a
`;`, which lets several share a line: `from cases; where risk > 90; take 5`. Inside brackets, `(`, `[` and `{`, the
end of a line ends nothing: a step can run over as many lines as its brackets hold.

## Blocks

The first line of a text sets its column, and each of its lines starts there: a text indented inside a Python string
reads as it is. A line that ends with `:` opens a block. The lines of the block start further in than the line of the
`:`, all at the column of the first of them, and the block ends before the first line that starts less far in. Its
lines can be on the rest of the `:`'s line instead, where a step allows: `summarize: n = count()`.

A line that starts further in than its block, or less far in where no block ends, is a mistake. A tab moves to the
next multiple of 8; a line whose place, beside the lines it is compared with, depends on how wide a tab is, because
it mixes tabs and spaces in another way, is a mistake.

In the grammar, `NEWLINE` stands for the end of a line or a `;`, `INDENT` for the start of a block, and `DEDENT` for
its end.

## Names

A name starts with a letter or `_`, and goes on with letters, digits and `_`, the letters and digits of any script.
An accent written as a character of its own, U+0300 to U+036F, and the joiners U+200C and U+200D can follow a letter
inside a name.

A name in backticks, `` `order date` ``, is a name whatever it holds: any characters but a backtick and the end of a
line, with two backticks for one. It can't be empty. A keyword in backticks is a name: `` `from` ``.

## Keywords

These are Python's keywords that ButtercupQL uses. They are never names:

`and`, `as`, `def`, `del`, `else`, `False`, `from`, `if`, `import`, `in`, `is`, `None`, `not`, `or`, `pass`, `True`.

These are Python's other keywords. ButtercupQL refuses each where a name or a step could be, with a message that
says what to write instead: `assert`, `async`, `await`, `break`, `class`, `continue`, `elif`, `except`, `finally`,
`for`, `global`, `lambda`, `nonlocal`, `raise`, `return`, `try`, `while`, `yield`, and `with`, but for
`with transaction:`.

The words of the steps, `where`, `select`, `sort`, `take`, `skip`, `distinct`, `group`, `summarize`, `join`, `left`,
`update`, `delete`, `insert` and `pivot`, and of the statements, `create`, `alter`, `drop`, `grant` and `revoke`,
are names. Each has its meaning only where a step or a statement starts, at the start of a line or after a `;`, and
before anything that would give it a value: `select = 3` makes a column named `select`. Elsewhere it is a name like
any other. The words that follow them, such as `by`, `on`, `into`, `table`, `index`, `unique`, `rename`, `to` and
`transaction`, are names too, with their meaning only where the grammar puts them.

## Numbers

A whole number is decimal digits: `20`, `1_000_000`. It can be hexadecimal, octal or binary, after `0x`, `0o` or
`0b`: `0xff`. An `_` goes between two digits, or after the base. A decimal whole number other than `0` can't start
with `0`.

A number with a fraction or an exponent is a fraction: `3.5`, `.5`, `1e6`, `2.5e-3`. Letters straight after a
number, as in `3px`, are a mistake, and a complex number, `2j`, is refused.

## Strings

A string is written as Python writes one: in `"` or `'`, on one line, or in `"""` or `'''`, over as many lines as it
needs. Its escapes are Python's: `\\`, `\'`, `\"`, `\a`, `\b`, `\f`, `\n`, `\r`, `\t`, `\v`, `\0`, `\x` and two
hexadecimal digits, `\u` and four, `\U` and eight, and a `\` at the end of a line, which joins it to the next.
`\N{name}` is refused: the character itself, or `\u` and its code, is written instead. Strings written one after
the other are one: `"open" "ed"` is `"opened"`.

Before the quote, `r` makes a raw string, in which `\` is a `\`; `u` changes nothing; and `f` makes an f-string, as
`rf` makes a raw one. In an f-string, `{` and `}` hold a value, and `{{` and `}}` stand for `{` and `}`:
`f"{key}: {risk}"`. A format after the value, such as `{risk:.2f}` or `{key!r}`, is not supported yet. `b`, for
bytes, is refused.

## Operators and brackets

The operators are `+ - * / // % **`, `== != < <= > >=`, `=` and the operators that give a value where it is,
`+= -= *= /= //= %= **=`, then `->`, `.`, `,`, `:` and `;`. Between two queries, `|` and `&` combine them. `@` starts
a decorator at the start of a line.

The other languages' ways of writing these, `&&`, `||`, `!`, `<>`, `~` and `:=`, are each refused with ButtercupQL's
way: `and`, `or`, `not`, `!=`.
