CREATE TABLE bibliography_references (
    id          INTEGER PRIMARY KEY,
    source_kind TEXT NOT NULL CHECK (source_kind IN ('import', 'inspire')),
    bibtex      TEXT NOT NULL,
    title       TEXT NOT NULL CHECK (length(trim(title)) > 0),
    year        INTEGER
);
CREATE TABLE contributors (
    reference_id INTEGER NOT NULL REFERENCES bibliography_references(id) ON DELETE CASCADE,
    kind         TEXT NOT NULL CHECK (kind IN ('author', 'collaboration')),
    position     INTEGER NOT NULL CHECK (position >= 0),
    name         TEXT NOT NULL,
    PRIMARY KEY (reference_id, kind, position)
);
CREATE TABLE identities (
    reference_id INTEGER NOT NULL REFERENCES bibliography_references(id) ON DELETE CASCADE,
    kind         TEXT NOT NULL CHECK (kind IN ('doi', 'arxiv')),
    value        TEXT NOT NULL,
    canonical    INTEGER NOT NULL CHECK (canonical IN (0, 1)),
    PRIMARY KEY (reference_id, kind, value),
    UNIQUE (kind, value)
);
CREATE UNIQUE INDEX one_canonical_identity
    ON identities(reference_id, kind) WHERE canonical = 1;
CREATE TABLE inspire_records (
    reference_id INTEGER PRIMARY KEY REFERENCES bibliography_references(id) ON DELETE CASCADE,
    record_id    INTEGER NOT NULL UNIQUE CHECK (record_id > 0),
    updated      TEXT NOT NULL
);
CREATE TABLE shelves (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE COLLATE NOCASE
);
CREATE TABLE shelf_references (
    shelf_id      INTEGER NOT NULL REFERENCES shelves(id) ON DELETE CASCADE,
    reference_id  INTEGER NOT NULL REFERENCES bibliography_references(id) ON DELETE CASCADE,
    citation_key  TEXT NOT NULL,
    PRIMARY KEY (shelf_id, reference_id),
    UNIQUE (shelf_id, citation_key)
);
PRAGMA user_version = 1;
