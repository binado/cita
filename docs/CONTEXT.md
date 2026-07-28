# Domain context

## Global library and shelves

Cita owns one user-global library rooted at `$CITA_HOME`, or `$HOME/.cita` by
default. `$CITA_HOME/library.sqlite3` is the sole authority and the fixed
default shelf is `main`. Commands never discover local project files.

Every data command lazily initializes the database and `main`. An explicit shelf
must already exist; selection never creates it. SQLite WAL transactions
coordinate processes, and every write revalidates global identities inside a
`BEGIN IMMEDIATE` transaction.

## Global reference and membership

A reference is stored once globally. Shelves contain memberships pairing that
reference with a shelf-local citation key, so the same source can use different
keys in different shelves. Removing the final membership deletes the orphaned
global reference.

DOI, versionless arXiv, and INSPIRE record identities are globally unique.
Title and author similarity are never identity. Incoming identifiers that point
to different global references are a conflict and never cause an implicit merge.

## Source snapshot and semantic projection

Every reference stores an exact standalone UTF-8 BibTeX string. This raw snapshot
is authoritative for bibliographic content. Cita also persists its structured
projection—title, year, ordered authors/collaborations, and normalized
identities—for SQL queries. The projection is recomputed from BibTeX and written
in the same transaction; it is never independently edited.

- INSPIRE sources additionally store the stable record ID, update timestamp,
  and canonical arXiv/DOI identities.
- Imported sources retain exact BibTeX. Import and sync attempt to canonicalize
  imports through INSPIRE using arXiv and then DOI identities.

The INSPIRE texkey remains inside raw BibTeX. A shelf-local citation key may
differ, is selected by `--key` or import, and is substituted only while
rendering.

## Sync

Sync selects unique global references from one shelf or the whole library.
Managed records refresh by stable INSPIRE record ID; imported records retry
canonicalization. Network work happens before the database write, and the
complete selected result set is applied atomically after checking that sources
did not change concurrently. Shared shelves immediately observe the update.

## Export and interchange

BibTeX export is a deterministic citation projection: entries are sorted by
local key, re-keyed without rewriting their fields, separated by one blank line,
and terminated by one newline. An arXiv PDF URL is added only when no authored
URL exists.

JSON and TOML exports are versioned, deterministic, lossless logical-library
documents. They preserve global sharing, shelves, local keys, raw source
snapshots, identities, and INSPIRE metadata, and are accepted by
`cita init --from-file`. SQLite row IDs and derived projection fields are
private and are not serialized.

Exports are caller-relative and cannot target the global store.

## Documents

PDFs and safely extracted source packages live under `$CITA_HOME/files` and are
shared by normalized, versionless arXiv ID. They are not part of lossless
library exports. Transient `fetch` resolves INSPIRE JSON unless `--save` stores
the complete INSPIRE snapshot.
