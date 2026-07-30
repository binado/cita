# AGENTS.md

## What this is

bibi is a command-line tool that maintains one project's bibliography as a
manifest and renders BibTeX from it on demand. Schema-1 `bibi.toml` is the only
authoritative project state; a `.bib` file is derived output the shell chooses
to keep, and bibi never reads one back. Rust edition 2024, MSRV 1.88.

`BIBI_TWO_BINARIES.md` records the product split this tool is being finished
against: the project-local file manager here, and a deferred user-level library
manager. It supersedes `REDESIGN.md` and `IMPLEMENTATION.md` wherever they
describe a global manifest, an `export` command, or a managed document cache;
elsewhere `REDESIGN.md` is the design authority and `IMPLEMENTATION.md` the
crate-level contract, and where those two disagree the design wins.

## Commands

```bash
cargo build --workspace
cargo test --workspace
cargo test -p bibi-bibtex
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo doc --workspace --no-deps          # CI runs this with -D warnings
cargo run -p bibi -- add 1207.7214
cargo run -p bibi -- add -f local.bib
cargo run -p bibi -- list --format json
cargo run -p bibi -- list --fields key,year,arxiv-url
cargo run -p bibi -- sync
cargo run -p bibi -- list --format bibtex > references.bib
cargo run -p bibi -- check references.bib
cargo run -p bibi -- fetch <selector> --url
cargo test -p bibi-inspire --test e2e -- --ignored  # live INSPIRE, network required
```

## Testing

Everything except the ignored e2e test is hermetic. HTTP suites bind a local
`TcpListener`; INSPIRE's pacing and retry run on an injected clock, so the suite
asserts *schedules* rather than sleeping.

Three seams exist for tests and are documented as such:

- `bibi_core::remote::testing` ships the provider-neutral fake and
  `verify_contract`, while `bibi_provider::testing` supplies the narrow
  scripted closed facade used by application tests. Provider crates prove the
  core contract once, and command tests replace only the compiled INSPIRE slot.
- `bibi_inspire::testing::TestClock` records the waits it was asked for.
- Two environment variables redirect what the binary would otherwise reach:
  `BIBI_INSPIRE_BASE_URL` and `BIBI_ARXIV_BASE_URL`. They are the only ambient
  state left — bibi targets the working directory and writes nowhere else — so
  pointing both at a local `TcpListener` is enough to keep a command-level test
  off the real network entirely.

CI runs format, clippy-as-errors, test, build, and doc on 1.88 and stable.
Tests are hermetic; INSPIRE and arXiv suites use local `TcpListener`s. The
exception is `crates/bibi-inspire/tests/e2e.rs`, a single `#[ignore]`d test
against the real INSPIRE API, run as a standalone liveness job.

## Commit messages

Use Conventional Commits: `<type>: <description>`.

## Workspace architecture

```text
bibi-bibtex
     │
     ▼
 bibi-core ───────────────┬────────────────┐
     │                    │                │
     ▼                    ▼                ▼
bibi-inspire        bibi-manifest    bibi-documents
     │                    │                │
     ▼                    └───────┬────────┘
bibi-provider ────────────────────┤
                                 ▼
                          bibi-application
                                 │
                                 ▼
                                bibi
```

- `bibi-bibtex`: the whole boundary around BibTeX syntax — scanning entry and
  key spans, structural validation, re-keying, identifier candidates, local
  metadata projection, deterministic rendering.
- `bibi-core`: provider-neutral domain values and the statically dispatched
  `RemoteProvider` contract — `BibiId`, the closed `Provider` enum, `ProviderId`,
  `Revision`, `Doi`, `ArxivId`, `Record`, locators, selection, filtering,
  remote outcomes, and provider errors.
- `bibi-provider`: exhaustive dispatch over `bibi_core::Provider`, default
  selection, direct local ingestion, provider construction, and complete
  conditional-refresh outcomes. Re-exports `Provider` rather than defining its
  own.
- `bibi-inspire`: INSPIRE transport, pure mapping, batching, the verified texkey
  join, pacing, and retry.
- `bibi-manifest`: schema-1 TOML, candidate validation, indexes, optimistic
  concurrency, atomic writes.
- `bibi-documents`: arXiv PDF/source retrieval. It stores nothing.
- `bibi-application`: command use cases over injected stores and providers.
- `bibi`: Clap definitions, bootstrap, output routing, exit codes.

Two couplings are forbidden by the graph: `bibi-manifest` never names a
provider crate, and `bibi-application` never depends on a concrete network
provider. `bibi-core` naming the closed provider roster is what makes the first
possible: `bibi-manifest` can reject an unknown provider name at decode time by
depending only on `bibi-core`, never on a network provider crate.

## Key decisions

### Provider-owned metadata

Every record is owned by a provider (I1). Identifiers and description are
derived from the provider's **structured record**, not from BibTeX — author
strings, collaborations, and the chosen year are all things a BibTeX rendering
loses. bibi never interprets provider BibTeX as metadata (I3); the only
semantic parse in the tree is `local_metadata`, used by the local provider,
where the user's BibTeX *is* the original.

Local provenance is structurally identified by a missing provider id. The local
path ingests user-supplied BibTeX directly and supports no refresh.

Provider selection is closed everywhere, not only operationally: one `Provider`
enum in `bibi-core`, `Provider::{Inspire, Local}`, is the sole provenance type
— a manifest field, a `--provider` flag, and a `<provider>:` locator qualifier
all name a value of it, and a name outside the set is rejected the moment it is
parsed. A manifest is decoded through this enum, so a hand-edited or
later-build `provider = "ads"` fails to load rather than being tolerated as an
unrefreshable-but-filterable record. One qualifier or `--provider` selects
exactly one implementation; repeated identical qualifiers are allowed, mixed
qualifiers fail before I/O, and no selection defaults to INSPIRE. There is no
fallback after absence or failure. Local ingestion is explicit through
`add -f FILE --provider local` and local never implements `RemoteProvider`.

### Byte-preserved payloads

Stored BibTeX is byte-identical to what it was given (I2). The only permitted
transformation is rewriting the citation-key token, through the scanner's key
span. Do not add a handwritten BibTeX writer or round-trip an entry through a
formatter.

`parse_one` and `parse_file` are purely syntactic and require no title;
`identifier_candidates` and `local_metadata` are separate operations, because
import must be able to resolve an entry that carries a DOI and nothing else.

### Keys and identity

A record's bibi id is a UUIDv4, minted once and never changed (I5). The local
citation key adopts the source texkey at ingestion and changes only through
`--key` or `rename` (I4). Collisions are never silently suffixed: `add` fails
the item, `add -f` skips it, and under `--overwrite` a colliding key identifies
the overwrite target.

### Conditional sync

Refresh fetches narrowed structured fields for every managed id, compares the
provider's opaque revision token, and fetches BibTeX only for changed records.
Bulk BibTeX is paired back to records through a **verified** texkey join: an
unplaceable entry or a texkey claimed twice fails that batch whole, while a
record that received no entry is a warning and a no-op.

**A record is updated as a unit.** If metadata mapped but the payload did not
arrive, nothing is written — above all not the revision, since an advanced
revision beside an old payload desyncs the two permanently and suppresses the
repair.

INSPIRE traffic is paced proactively under the documented 15-per-5-seconds
budget; 429 retries are the fallback and clamp `Retry-After` to [5s, 60s].

### Rendering and output

Rendering is a pure function of the manifest and its options (I6): no network,
no ambient state. `check` and `list --format bibtex` share one `render_manifest`
entry point; every other BibTeX-producing path calls `bibi_bibtex::render`
directly, so separator and trailing-newline rules live in exactly one place.

**bibi writes no bibliography.** `list --format bibtex` renders to stdout and
the shell decides whether and where that becomes a file, which is what keeps a
`.bib` output rather than maintained state. `sync` and rendering stay separate
operations; nothing renders and refreshes in one command. `list` and `check`
share one filter set, `--provider` included, so a filtered view can be rendered
and then verified under the same options.

`--format` says how to encode a listing; `--fields` says what to put in it, as
tab-separated columns. They are mutually exclusive. `--fields arxiv-url` is how
a shell downloads a selection in bulk, which is what keeps `fetch` a command
that acquires one file rather than a downloader.

**`output::table` is pure over its width and colour**, both resolved at the call
site. Ambient reads inside it would be untestable: `terminal_size` falls back to
*stdin*, so a layout test would silently take the width of whatever terminal ran
`cargo test`. Column padding is measured with `unicode-width`, never
`chars().count()` and never `{:<n$}` — a combining mark is a `char` occupying no
column, so counting characters shifts every column to its right. All styling,
not only colour, is suppressed when `NO_COLOR` is set or the stream is not a
terminal; one policy in `output::color_enabled` serves the table and warnings
alike.

**`--provider` accepts only a provider this build carries.** Because a manifest
cannot hold a foreign one either, `list`/`check` filtering and `add`/`sync`
calling share the identical acceptance check, rather than filtering tolerating
a wider set than calling does. A rejected name is answered with the names that
would work, never with the parser's grammar.

stdout carries the command's result in its most pipeable form; stderr carries
everything meant for a human. Skips exit zero, failures exit one, Clap usage
errors exit two.

### Atomic mutations

A command validates a complete candidate before writing anything. Commit
compares the exact bytes read (`Generation`) against the bytes on disk, then
writes through a same-directory temporary with fsync and rename. This is
best-effort staleness detection, not mutual exclusion; bibi takes no lock and
writes no file into a project directory other than the manifest.

Bulk operations are partial; writes are not. A batch resolves everything it was
asked to, reports each failure, and commits the successes in one write.

### Known limitation

`fetch_payloads` returns `Result<Vec<PayloadItem>, ProviderError>`, so a
provider can report per-record absence but not per-batch failure. An ambiguous
texkey join therefore fails every changed record under *that provider* rather
than only the batch it occurred in. Other providers still commit, and nothing
wrong is ever written; the cost is that a repair retries more records than it
strictly needs to. Narrowing it means widening the contract.

### Scope

A manifest is `bibi.toml`: the one named by `-p/--path`, otherwise the one in
the working directory. **There is no upward search and no user-level manifest.**
The target is always evident from where the command was run. Inputs (`add -f`,
`check`) resolve against the working directory, as does a `fetch` download. Only
`init`, `add`, and `add -f` may create a missing manifest, and bibi writes no
file into a project directory other than that manifest.

There is no global store, no document cache, no shelves, no library registry, no
grouping, and no Git integration. Those belong to the deferred library manager
in `BIBI_TWO_BINARIES.md`, which is why they are absent here rather than
unimplemented.

### Documents

`fetch` acquires one file into the working directory and manages nothing. It
retrieves arXiv artifacts only: a record carrying no arXiv identifier fails
cleanly, and bibi does not follow a DOI to a publisher.

The default filename is the arXiv identifier and its kind — `1207.7214.pdf`,
`hep-th-9901001.tar.gz` — which is arXiv's own naming rather than a scheme of
bibi's. `-o/--output` names an exact file. Neither destination is ever
overwritten. A source fetch saves the original archive rather than unpacking
it, so no archive traversal happens anywhere in the tree.

## Error handling

Library crates use typed `thiserror` enums. The binary uses `anyhow` for
context. Add typed library variants for new failure modes.
