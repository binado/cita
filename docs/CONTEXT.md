# Domain context

## Reference

A `Reference` is bibi's provider-neutral semantic projection: title, authors,
collaborations, display year, and normalized identifiers (DOI, arXiv, provider
ids). Commands consume references; they do not inspect provider payloads.

## Bibliography

The `references.bib` file is the store, not an output. It is a sequence of
authoritative BibTeX entries interleaved with whatever else the author put
there — comments, `@string` directives, blank lines. bibi models the entries and
copies everything else through untouched, which is why a mutation can rewrite
one entry without disturbing the rest of the file.

## Entry

An entry is one complete `@type{key, ...}` block. It is the authoritative
evidence for a reference: the projection is derived from it, never stored beside
it. Tool-owned bookkeeping lives inside the entry under the `x-bibi-` prefix, so
an entry is self-contained — everything needed to refresh it travels with it.

## Managed and unmanaged

An entry is *managed* when it carries a present and parseable non-zero
`x-bibi-inspire-id`. That is the whole definition: there is no separate tag that
could disagree with the data. A malformed or zero id reads as unmanaged so a
hand-edit cannot make the file unreadable. Managed entries refresh by stable
record ID. Unmanaged entries with a DOI or arXiv id are candidates for
*adoption* — `bibi sync` resolves them and attaches bookkeeping while leaving
their content alone, since resolving answers "what is this thing I have", not
"replace it".

`x-bibi-frozen` removes an entry from both halves. It covers the entry you
corrected by hand and the textbook INSPIRE will never have, because both mean
the same thing: do not touch this.

## Local citation key

The citation key is bibi's local identity and the token that appears in
`\cite{}`. It may differ from a provider texkey, and refreshing never changes
it. `bibi rekey` changes only that token within the entry.

## Provider identity

An identifier names the same work independently of its local key. DOI and arXiv
identifiers are normalized globally. Provider identities are namespaced, for
example `inspire:1124337`. Any identity shared by different local keys is a
conflict. A record ID claimed by two entries is the same conflict discovered
later, when INSPIRE resolves a pair that DOI and arXiv comparison could not tell
apart.

## Derived export

The `bibi export` output is a derived artifact. It is the bibliography with
bibi's own namespace removed and resolvable `url` fields added for downstream
reference managers. Unlike the bibliography it is not tracked, never read back,
and never authoritative — it is written for other tools to consume and
regenerated rather than edited. Its layout is normalized, because nobody edits
it and there are no authored bytes to preserve.
