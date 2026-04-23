# Comments

## Language and tone

English, declarative, compact. State the non-obvious _why_ — the constraint, the invariant, the workaround — never the obvious _what_. One dense sentence beats a paragraph of narration; every word earns its line, but never at the cost of a fact a future reader would need.

## Length

Aim for one or two lines. A comment block over three lines usually means the function is under-named or doing too much — fix the code, not the comment. If a comment carries a URL, the link stays when you compress the prose around it.

## No separator lines

Never use `// ---`, `// ===`, `// ***`, `/* === */`, or any visual divider made of repeated characters.

A separator signals that a file carries too many responsibilities and should be split, or that a function's name does not communicate its intent. If names are honest, the structure is self-evident — separators add noise without information.

If a section genuinely needs a comment, write the comment directly. The function signature, or the module boundary _is_ the separator.
