# Marrow Language Reference

This directory is the canonical reference for Marrow `.mw`. It describes the
language Marrow presents to application developers: a small programming
language with built-in saved data.

## The Main Idea

Marrow has one data model:

```text
A resource is a typed tree.
The same resource shape can be local or saved.
^ marks saved data.
Indentation in a resource declaration shows tree layers.
Parentheses introduce keyed layers.
Indexes are declared lookup trees.
History is modeled as keyed child layers.
```

Local data:

```mw
var book: Book
book.title = "Small Gods"
book.author = "Terry Pratchett"
```

Saved data:

```mw
^books(id).title = book.title
```

The difference is the `^`. Without it, data is local to the running program.
With it, data is saved in the project database.

## First Look

```mw
module shelf::books

resource Book at ^books(id: int)
    ;; Display title shown in shelf views and search results.
    @id("book.title")
    required title: string

    @id("book.author")
    required author: string

    @id("book.shelf")
    required shelf: string

    loanedTo: string
    tags(pos: int): string

    index byShelf(shelf, id)

pub fn add(title: string, author: string, shelf: string): Book::Id
    var book: Book
    book.title = title
    book.author = author
    book.shelf = shelf

    const id: Book::Id = nextId(^books)
    ^books(id) = book

    return id

pub fn listShelf(shelf: string)
    for id in keys(^books.byShelf(shelf))
        print($"book {id}: {^books(id).title}")
```

This shows the main shape:

- `resource Book` defines a typed tree shape.
- `at ^books(id: int)` declares the stored identity key and generated
  `Book::Id` type.
- Documentation comments and `@id(...)` metadata feed editor hover, docs,
  inspect output, and rename/migration tooling.
- `index byShelf(shelf, id)` declares an alternate lookup tree.
- `var book: Book` uses the same resource shape locally.
- `^books(id) = book` saves the local resource and creates index entries.
- Assignment to an indexed field updates the field and its index entries
  together.
- A single managed write does not need a user-written transaction.
- `keys(...)` iterates one layer of a tree.

## Reference Map

- [Syntax](syntax.md) defines source text, indentation, declarations,
  statements, expressions, operators, strings, spelling, and punctuation.
- [Types](types.md) defines primitive types, sparse fields, required fields,
  resources, sequences, keyed trees, local variables, identity types, and
  conversion rules.
- [Resources and Saved Data](resources-and-storage.md) defines resources,
  local trees, saved trees, identity keys, indexes, history, transactions,
  locks, delete, merge, and data access.
- [Modules and Functions](modules-functions.md) defines modules, imports,
  visibility, parameters, named arguments, resource arguments, returns, and
  name resolution.
- [Control Flow](control-flow-and-effects.md) defines conditionals, loops,
  tree iteration, short-circuiting, labeled loops, and structured errors.
- [Builtins](builtins.md) defines always-available helpers such as `exists`,
  `get`, `keys`, `values`, `entries`, conversions, `append`, `nextId`,
  output, and errors.
- [Standard Library](standard-library.md) defines the `std::` modules
  for clock/instant, IO, env/config, strings, bytes, math, testing, and logging.
- [Reference Sample](sample.md) gives one compact project that exercises
  resources, saved data, indexes, history, transactions, and traversal.
- [Formal Grammar](grammar.md) gives an EBNF-style grammar for the language.

## Small Complete Example

```mw
module reading::shelf

resource Book at ^books(id: int)
    required title: string
    required author: string
    required shelf: string
    subtitle: string
    loanedTo: string
    tags(pos: int): string

    index byShelf(shelf, id)

pub fn loan(id: Book::Id, borrower: string): bool
    if not exists(^books(id))
        return false

    if exists(^books(id).loanedTo)
        throw Error(
            code: "book.already_loaned",
            message: $"Book {id} is already loaned.",
        )

    ^books(id).loanedTo = borrower

    return true

pub fn printShelf(shelf: string)
    for id in keys(^books.byShelf(shelf))
        const title: string = ^books(id).title
        const author: string = ^books(id).author
        print($"{title} by {author}")
```

## Style

Examples use full statement spellings such as `if`, `else`, `for`,
`transaction`, and `delete`. Output uses `write(...)` and `print(...)`.
Type names use one source spelling.
