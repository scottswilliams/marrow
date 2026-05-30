# Modules And Functions

Future counterpart of
[`../../language/modules-functions.md`](../../language/modules-functions.md). The
surfaces below are designed but not yet implemented.

## Visibility

`pub` is one uniform, enforced marker on every top-level declaration: `pub fn`,
`pub resource`, `pub enum`, `pub const`. A declaration is private to its module
unless marked `pub`, and referencing a non-`pub` declaration from another module
is a checked error. For a saved resource, `pub` governs which modules may read or
write its `^` root — visibility is data ownership, not only type naming.

## Parameter Documentation

A `;;` doc comment may precede an individual parameter. To document parameters,
write the parameter list one per line:

```mw
fn fileBook(
    ;; the book to file. every required field must be set before it is
    ;; saved, or the write faults with the missing-field error.
    book: Book,
    ;; shelf the book is filed under
    shelf: string,
): Book::Id
```

Each parameter's doc is the run of `;;` lines directly above it; the language
server and other tooling render it in signature help next to the parameter's
name and type, the same way a `;;` comment on a `const`, `resource`, function,
or field is surfaced. A doc longer than one line is written as consecutive `;;`
lines, the same multi-line form those comments already take elsewhere; rendering
follows the same convention, so a soft-wrapped doc reads as one paragraph. A
single-line parameter list carries no parameter docs.

A single-line parameter list is comma-separated. In a multi-line list a line
break separates one parameter from the next, so the comma is optional and a
parameter is documented by placing its `;;` comment on the line above it; the
formatter writes a comma after every parameter, including the last, so adding a
parameter never edits the line before it. A parameter occupies one logical line.
Its type may span several physical lines only inside brackets — a wrapped
`sequence[...]`, for example — where the brackets hold the type together; a bare
`name:` and `type` split across lines at the top level is not a valid layout.

Parameter docs and resource field docs document different things and compose. A
resource documents the meaning of each field once, at its declaration; a
parameter of that resource type documents the role of the whole value at the
call boundary. Passing a resource as a parameter therefore reads with
documentation at both levels — the parameter says why the function needs the
value, and the resource says what each field means — so a structured argument
stays self-describing through a named resource rather than an anonymous inline
shape.
