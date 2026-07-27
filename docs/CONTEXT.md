# Domain context

## Reference

A `Reference` is bibi's provider-neutral semantic projection: title, authors,
collaborations, display year, publication, URL, primary category, and normalized
identifiers. Commands consume references; they do not inspect provider payloads
or parse tracked output.

## Source snapshot

A source snapshot is the authoritative provider-specific evidence stored under a
local citation key in `cita.toml`. Every snapshot holds authoritative standalone
BibTeX and is tagged by the source that owns its refresh lifecycle.

- An INSPIRE entry (`source = "inspire"`) contains authoritative INSPIRE BibTeX,
  the stable record ID (its refresh key), an update timestamp, and canonical
  normalized arXiv/DOI values selected from and cross-checked against that
  BibTeX.
- An import (`source = "import"`) contains one exact standalone imported entry.

Snapshots project to `Reference` from their BibTeX; INSPIRE entries override the
projected arXiv/DOI with their stored identifiers and add the `inspire` provider
id. Projections are derived and are never stored as a second authority.

## Local citation key

The sorted key in `references` is bibi's local identity for citation and Git
review. It may differ from a provider texkey. Refreshing a source snapshot never
changes it; bibliography generation changes only the raw entry's key token.

## Provider identity

An identifier names the same work independently of its local key. DOI and arXiv
identifiers are normalized globally. Provider identities are namespaced, for
example `inspire:1124337`. Any identity shared by different local keys is a
conflict, including across source kinds.

## Generated bibliography

`references.bib` is a tracked generated artifact, analogous to a lockfile. Its
bytes are completely derived from the manifest: local-key order, preserved raw
entry fields, one blank line between entries, and a final newline. Drift is an
error that blocks reads until the next mutation rewrites the file.

## Derived export

The `bibi export` output is a derived artifact, not a generated one. It shares
the generated bibliography's layout and ordering but adds resolvable `url`
fields for downstream reference managers. Unlike `references.bib` it is not
tracked, not verified, never read back, and never authoritative — it is written
for other tools to consume and regenerated rather than edited.

## Managed and imported references

INSPIRE snapshots are managed: `bibi sync` refreshes them by stable record ID.
BibTeX snapshots are imported/unmanaged and remain byte-for-byte unchanged until
explicitly removed. bibi does not merge provenance or adopt one source as
another.
