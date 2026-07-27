# bibi-bibfile

The `.bib` file as the single source of truth for
[bibi](https://github.com/binado/bibi).

Entries are authoritative BibTeX; tool-owned bookkeeping (the INSPIRE record id
and its refresh timestamp, curated arXiv/DOI identifiers, and a freeze marker)
lives in `x-bibi-*` fields on the entries themselves, where LaTeX toolchains
ignore it. There is no separate manifest and no generated artifact, so there is
nothing to drift.

Because the file is hand-edited, writes are byte-preserving: a mutation replaces
only the entries it touches and copies every other byte through unchanged,
including comments, `@string` directives, and whatever spacing the author chose.

This crate is published for reuse by the bibi workspace.
