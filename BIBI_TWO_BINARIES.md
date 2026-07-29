# Two bibi binaries

## Status

This document records a product and workspace direction: the small,
project-local bibliography tool and the large, user-level bibliography manager
are separate binaries built on common domain crates.

This direction reopens parts of `REDESIGN.md` that reject a database backend and
a user-level library. Until those decisions are reconciled explicitly,
`REDESIGN.md` remains authoritative for the existing `bibi` binary.

**The large binary is deferred.** Active work is phase one of the implementation
sequence: finishing the small binary against its file-centered promise. The
large binary is recorded here so that the small binary's deletions are made for
a stated reason rather than by taste, and so the shared-crate consequences are
known before they are needed. Sections describing large bibi and the shared
crate layout are design intent, not pending work.

## The problem

There are two coherent products pulling bibi in opposite directions.

The first is a file-centered CLI for maintaining one bibliography. Its value is
that the complete project state is visible, reviewable, portable, and
reproducible from a single tracked manifest.

The second is a personal bibliography manager. Its value comes from global
storage, tags, collections, rich queries, cross-project deduplication, and
incremental organization of a large library.

A single middle-ground product weakens both promises:

- adding global state and database behavior makes the project tool harder to
  understand and reproduce;
- retaining a manifest-shaped storage model limits the library manager's
  relationships and query features;
- treating TOML and SQLite as interchangeable CRUD backends hides meaningful
  differences between whole-snapshot publication and transactional row
  mutation.

The split is therefore a product split, not merely a choice of persistence
adapter.

## Decision

Build two binaries:

1. **Small bibi** remains the project-local bibliography file manager.
2. **Large bibi** becomes the user-level bibliography manager backed by
   SQLite.

The final name of the large binary is not decided here. The binaries share
domain mechanics and provider integrations, but each owns its persistence
model and application workflows.

## Small bibi

### Product promise

> Clone the project and reproduce its bibliography without any user-level
> library or ambient database.

### Authority and scope

- `bibi.toml` is the only authoritative project state.
- Each invocation selects `./bibi.toml` or one manifest named explicitly with
  `-p/--path`.
- There is no `-g/--global` target and no upward directory search.
- The manifest is self-contained, Git-reviewable, and hand-mergeable.
- A `.bib` file is explicit derived output produced through stdout redirection.
- Rendering remains a pure function of the manifest and rendering options.
- The global library, if installed, is never required to list, inspect, sync,
  check, or render a project.

### Responsibilities

Small bibi:

- adds, removes, renames, lists, and shows records;
- refreshes provider-owned metadata and BibTeX;
- renders BibTeX to stdout and checks materialized `.bib` files;
- imports BibTeX into a project manifest;
- downloads a selected record's arXiv PDF or source artifact;
- validates and atomically publishes a complete candidate manifest.

Small bibi does not provide:

- an authoritative global record store;
- a global-manifest mode;
- library-wide tags or collections;
- cross-project queries;
- saved searches;
- a managed document cache;
- project records that are only references into a user-level database.

Project-local metadata can be added later when it has a clear project use, but
the feature set should not turn the manifest into an encoded global library.

### Command set

Small bibi has this command set:

| Command | Purpose |
| --- | --- |
| `init` | Create an empty `bibi.toml` without overwriting anything |
| `add <locator>...` | Resolve and add provider records |
| `add -f <bibfile>` | Import BibTeX entries |
| `remove <selector>...` | Remove records |
| `rename <selector> <key>` | Change a local citation key |
| `list` | Select records and project them to stdout |
| `show <selector>` | Emit exactly one record as BibTeX |
| `sync` | Refresh provider-owned records |
| `check <bibfile>` | Compare a materialized BibTeX file with current rendering |
| `fetch <selector>` | Retrieve or locate a PDF or source artifact |

There is no `export` command. The shell chooses whether and where rendered
BibTeX becomes a file:

```text
bibi list --format bibtex > refs.bib
bibi sync && bibi list --format bibtex > refs.bib
```

`sync` and rendering remain separate operations. `list` is always offline;
there is no flag on it that performs a preliminary refresh.

Shell redirection opens and truncates the destination before bibi runs. A user
who needs atomic replacement can use a temporary sibling:

```sh
bibi list --format bibtex > refs.bib.tmp &&
    mv refs.bib.tmp refs.bib
```

The loss of built-in atomic output is accepted because `.bib` files are derived
and reproducible. If safe destination writing later proves important, that is
the strongest reason to reconsider a dedicated file-writing command.

### Listing, filtering, and rendering

`list` first selects records, then projects them into a format:

```text
bibi list
bibi list --format keys
bibi list --format json
bibi list --format bibtex
```

It retains these lightweight, project-local filters:

| Filter | Match |
| --- | --- |
| `--author <text>` | Case-insensitive substring in an author or collaboration |
| `--title <text>` | Case-insensitive substring in the title |
| `--year <year>` | Exact stored year |
| `--provider <name>` | Exact stored provider name |
| `--local` | Records owned by an ingest-capable, unrefreshable provider |

Every supplied criterion is conjunctive, and filtering preserves local-key
order. These are bounded scans over one manifest, not a tag system or query
language.

The supported formats are:

- `table`, the default human-readable view;
- `keys`, one local citation key per line;
- `json`, the stable metadata-only automation view;
- `bibtex`, the deterministic rendering of the selected records.

`show` remains distinct because it resolves exactly one selector and emits
exactly one locally re-keyed BibTeX entry.

`check <bibfile>` compares a file byte-for-byte with the same deterministic
rendering. It accepts the same filters as `list` so it can verify a materialized
filtered view:

```text
bibi list --author Aad --format bibtex > aad.bib
bibi check aad.bib --author Aad
```

`list` and `check` share one filter set, `--provider` included. The current tree
splits them into two types, with a provider filter for `list` and none for the
rendering commands, because on `export` `--provider` named the provider to
*sync* first and a filter spelled the same way would have been ambiguous.
Removing `export` removes the ambiguity, so the two collapse into one.

### Simplified `fetch`

`fetch` acquires a file; it does not manage a document collection.

```text
bibi fetch <selector>
bibi fetch <selector> --source
bibi fetch <selector> --url
bibi fetch <selector> --open
bibi fetch <selector> -o <path>
```

The behavior is:

| Options | Result |
| --- | --- |
| none | Download the PDF to the current working directory |
| `--source` | Download the original source archive to the current working directory |
| `--url` | Print the selected artifact's public URL without downloading |
| `--open` | Open the downloaded or already-present destination |
| `--url --open` | Open the public URL without downloading |
| `-o/--output <path>` | Download to that exact path |
| `--source --url` | Print the source archive URL |

Relative output paths resolve against the current working directory. `--output`
names an exact file, not a directory. `--output` is incompatible with `--url`.

Downloads use a temporary sibling, validate the artifact sufficiently for its
kind, and rename it into place only after success. An explicit output path is
never overwritten. When the default destination already exists, `--open` may
open it; otherwise `fetch` reports the collision and asks the user to remove it
or select another path.

There is no `--force`, global document cache, cache root, or `cache clean`
command.

`fetch` retrieves arXiv artifacts and nothing else. A record carrying no arXiv
identifier fails cleanly and says so: bibi does not follow a DOI to a publisher,
and `--url` prints no resolver link. Keeping the command pointed at one
provider's artifact service is what stops it from becoming a general document
acquisition layer, which is a library concern.

Source retrieval downloads the original archive rather than extracting it.
This preserves the provider artifact and avoids making archive extraction and
directory lifecycle part of the small binary.

#### Default filenames

The default filename is the normalized arXiv identifier, with `.pdf` for a PDF
and `.tar.gz` for a source archive:

```text
2401.12345.pdf
1207.7214.tar.gz
hep-th-9901001.pdf
```

Adopting arXiv's own naming instead of inventing a descriptive scheme deletes an
algorithm and three decisions with it: a portable Unicode slugging rule, whether
a collaboration counts as an author, and which segments to omit when a value is
absent. The only transformation is replacing `/` in a legacy identifier with
`-`. arXiv cannot avoid that problem either — its URL carries the archive as a
path component, so a browser saving `/pdf/hep-th/9901001` keeps `9901001` alone
and loses which archive it came from.

Two consequences are accepted:

- The name carries no version, because `ArxivId` is versionless by design: a
  record names a work rather than one revision of it. A user holding v1 who
  fetches again gets the ordinary collision report and chooses a path.
- The name is not self-describing in a downloads directory. `--output` is the
  answer when a descriptive name matters.

The command prints the resulting path or URL to stdout. Progress, provenance,
and diagnostics go to stderr.

## Large bibi

### Product promise

> Organize, search, and reuse everything the user might cite across projects.

### Authority and scope

- A user-level SQLite database is authoritative for the personal library.
- Records, tags, collections, and project membership are first-class
  relationships.
- Mutations use database transactions and constraints.
- The database may maintain indexes suitable for rich interactive queries.
- Database backup and synchronization are library concerns, not project
  manifest concerns.

### Responsibilities

Large bibi may provide:

- global import and provider resolution;
- tags and collections;
- Boolean and metadata queries;
- full-text search;
- library-wide duplicate detection;
- saved searches;
- document collection management and caching;
- materialization of selected records into project manifests;
- explicit comparison between a library record and a copied project record.

SQLite is a natural fit because tags, collections, and project membership are
many-to-many relationships:

```text
records
tags
record_tags
collections
collection_records
projects
project_records
```

The exact schema is deliberately left to the large binary. It is not a storage
schema that small bibi must implement through traits.

## Interaction between the binaries

The binaries are independent at runtime:

- small bibi never opens the SQLite library and does not know whether large
  bibi is installed;
- large bibi never invokes the small binary;
- there is no IPC protocol, daemon, or shared mutable process state;
- both binaries use `bibi-manifest` to understand the project file format.

The compile-time dependency is intentionally asymmetric. Large bibi depends on
the project-format crate, while small bibi has no dependency on the large
binary, its application crates, or its SQLite store:

```text
bibi-bibtex --> bibi-core --> bibi-manifest --> small bibi
                                     |
                                     +--------> large bibi
```

`bibi-manifest` is therefore both small bibi's persistence module and the
shared implementation of the project interchange format. It owns schema
decoding, canonical encoding, candidate validation, duplicate detection,
generation checks, and atomic publication.

The binaries cross scopes through two explicit copying operations:

```text
                     import
project bibi.toml ----------------> SQLite library
       ^
       |
       +--------------------------- selected library records
                   materialize
```

### Importing a project into the library

Large bibi may import a project manifest:

```text
large-bibi import ./bibi.toml
```

It loads and validates the file through `bibi-manifest`, matches records by
preserved bibi UUID and then canonical identifiers, inserts genuinely new
records, and reports conflicts rather than silently choosing a version. Import
does not modify the project manifest or establish a live link.

Large bibi may record source path and import time privately in SQLite. Such
provenance is library state and must never become required project state.

### Materializing library records into a project

The reverse operation copies a selected library view into a project:

```text
SQLite library
      |
      | copy/materialize selected records
      v
project bibi.toml
      |
      | `bibi list --format bibtex` and shell redirection
      v
references.bib
```

Materialization copies complete records into `bibi.toml`; it never writes a
database reference that must be dereferenced during rendering. A project remains
self-contained after the copy.

Materialization copies the bibi UUID, provider provenance, structured metadata,
local key, and byte-preserved BibTeX payload, and **nothing that only the
library knows**. Tags, collections, saved searches, import provenance, and
document cache state never reach `bibi.toml`. Preserving identity leaves room
for later explicit comparison and refresh workflows; preserving nothing else
keeps the schema shared.

This is the one place the asymmetric dependency could be reversed in practice.
Admitting a single library-only field into the manifest would make
`bibi-manifest` grow a key small bibi ignores, and would turn a schema-1 to
schema-2 migration — a change to a crate both binaries depend on — into
something driven entirely by a large-bibi feature. It would also be the first
step toward project records that are references into a database, which the
project promise forbids. A project record carries what a project needs to
render and refresh itself, and stops there.

Import and materialization are copies, not bidirectional synchronization. After
either operation, the project and library records are independent snapshots.
Automatic synchronization would require ownership and conflict rules for:

- local citation keys;
- provider revision and metadata;
- BibTeX payloads;
- project-local versus library tags;
- deletion in either scope.

Those rules should be designed only after the explicit import and
materialization operations are useful in practice.

## Workspace structure

The binaries share domain, provider, and project-format crates, not a universal
persistence interface.

The shared stack, which knows nothing about either persistence model:

```text
                         bibi-bibtex
                              |
                              v
                          bibi-core
                              |
     +-------------+----------+-------------+
     |             |          |             |
     v             v          v             v
bibi-provider  bibi-sync  bibi-documents  bibi-manifest
     |             ^        (retrieval)   (project format)
     v             |
bibi-inspire ------+
```

`bibi-sync` depends on `bibi-provider` for the contract, never on a concrete
provider. The two applications sit on top of that stack and diverge only in
storage:

```text
   small application                    library application
  (manifest snapshot,                 (transactions, tags,
   candidate, generation)              collections, queries)
          |                                     |
          v                                     v
   small bibi binary                    large bibi binary
                                                |
                                                v
                                          bibi-library
                                            (SQLite)
```

Shared crates should contain:

- BibTeX scanning, validation, re-keying, and deterministic rendering;
- provider-neutral record and identifier types;
- provider contracts, registries, and concrete provider integrations;
- the conditional refresh algorithm, as a decision over owned values;
- artifact retrieval;
- pure domain operations that have the same semantics in both products.

Small-bibi crates should contain:

- schema-1 TOML serialization;
- candidate validation and manifest indexes;
- generation checks and atomic whole-file publication;
- project-oriented command workflows.

Large-bibi crates should contain:

- SQLite schema and migrations;
- transactions and relational constraints;
- tags, collections, library queries, and saved searches;
- document collection management and caching;
- library-oriented workflows.

`bibi-manifest` sits in the shared stack because it plays two roles: it is small
bibi's persistence module, and it is large bibi's decoder and encoder for the
project interchange format. It is shared as a *format*, not as a storage
abstraction, and it names no database.

### Where sync lives

This section records where sync *belongs*, not work to do now; the extraction
waits for a second caller. Conditional refresh is the densest logic in the tree
and none of it is project-specific. Revision comparison, the verified texkey join, the rule that a
batch with an ambiguous join fails whole, and above all the rule that **a record
is updated as a unit** — never an advanced revision beside an old payload — are
promises about providers, not about storage. Large bibi needs every one of them
and must not reimplement them.

The current `sync` interleaves storage with decision:

```text
store.load() -> group by provider -> refresh (mutating a candidate) -> store.commit()
```

Storage appears only on the first and last line. Everything between is a
function from a provider and a set of managed handles to a set of refreshed
records and a report. That middle belongs in a shared `bibi-sync` crate that
touches no store at all:

```text
bibi-sync
    refresh(provider, &[Managed], options) -> RefreshOutcome

small application:  load manifest -> refresh -> fold into a Candidate -> commit
library application: select rows  -> refresh -> apply in a transaction
```

This shares the algorithm without sharing a storage interface, the distinction
drawn under "Why there is no generic CRUD backend" below. `Managed` is already
the right shape for it: a projection of a record, not a manifest row.

### Artifact retrieval versus document management

The existing document code is two things wearing one name: arXiv artifact
retrieval — URL construction, download, sufficient validation, temporary sibling
and rename — and cache policy — a global root, deduplication, and eviction.

Small bibi's simplified `fetch` keeps the first and discards the second. Large
bibi wants both. `bibi-documents` therefore narrows to retrieval and becomes a
shared crate; cache policy moves into a large-bibi crate. Retrieval is not
rewritten from scratch for the small binary, because the download-and-rename
sequence is exactly the code that should not exist twice.

## Why there is no generic CRUD backend

`Record` remains an owned domain value rather than a persistence-aware trait.
The two storage models do not expose one generic table interface:

- a manifest loads a validated snapshot and publishes a complete candidate;
- SQLite performs incremental operations inside transactions;
- manifest duplicate detection validates the complete proposed state;
- database uniqueness and referential integrity are continuously enforced;
- manifest ordering is canonical serialized output;
- database ordering belongs to each query.

A trait containing `add`, `replace`, `remove`, and `list` would reproduce
surface operations while failing to express these different invariants. Sharing
that trait would reduce clarity without providing useful substitution.

The binaries should instead exchange shared domain values at an explicit
materialization seam.

The same reasoning is what makes `bibi-sync` shareable rather than
contradictory. Sharing an algorithm that takes owned values and returns a
decision costs nothing, because the decision means the same thing on both
sides. Sharing a storage trait would cost the truth: `commit` compares a
`Generation` and can fail as stale, and a transactional insert cannot. Extract
computations, not persistence.

## Implementation sequence

The large binary is deferred. Everything below in phase one is subtraction from
the existing tree, is worth doing whether or not the large binary is ever
written, and needs no workspace reorganization at all: the eight current crates
are already the right shape for small bibi. The split's first payment is
deletion, not new code.

### Phase one: the small binary

1. Remove global-manifest targeting. This reaches further than the flag:
   `PlatformPaths` loses both fields and disappears, taking `TargetSelection`,
   the `NoPlatformDirectory` error, and the `directories` dependency with it.
   `TargetResolver` collapses to `-p/--path` or `./bibi.toml`, and `Services`
   loses its paths argument.
2. Remove `export`, making `list --format bibtex` the only multi-record BibTeX
   output, and collapse the two filter types into one.
3. Replace the managed document cache with the direct `fetch` behavior above.
   Delete cache policy — the clean modes, fetch policy, cache root, and cached
   path layout — in place, along with archive extraction and its `tar` and
   `flate2` dependencies. What remains in `bibi-documents` is retrieval.
4. Keep a base-URL seam on the retrieval client. Two of the three redirection
   variables, `BIBI_GLOBAL_MANIFEST` and `BIBI_CACHE_ROOT`, are deleted by steps
   1 and 3, which is a genuine reduction in ambient state. The third must
   survive, or `fetch` becomes untestable offline. Because `fetch` now writes
   into the working directory, the CLI suite gives each test its own.

### Phase two: deferred until the large binary exists

The shared-crate work described under "Workspace structure" is *design intent,
not pending work*. Extracting `bibi-sync`, and splitting retrieval away from
document management into separate packages, both wait for a second caller.
Doing either now would mean guessing the shape large bibi wants, and a wrong
guess is worse than no extraction: the seam would have to be moved anyway, after
code had been written against it.

5. Prototype the large binary with only three end-to-end workflows:
   - import records into SQLite;
   - query the library;
   - materialize selected records into a self-contained `bibi.toml`.
6. Let those workflows force the shared seams, then extract `bibi-sync` and the
   retrieval crate against two real callers.
7. Add tags, collections, and document management only after that vertical
   slice proves useful.
8. Reconcile this direction with `REDESIGN.md` before treating the large binary
   as part of the accepted workspace contract.

## Open decisions

- The name and package layout of the large binary.
- Whether both binaries ship from one Cargo package or separate packages.
- The exact materialization command and collision policy.
- The exact import conflict policy when UUID and canonical identifiers disagree.
- Whether `bibi-sync` also owns pacing and retry, or leaves them to each
  provider crate as today.

None of these block phase one.
