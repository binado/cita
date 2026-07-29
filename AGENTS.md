# AGENTS.md

## What this is

bibi is a command-line tool that maintains a bibliography as a manifest and
renders a BibTeX file from it on demand. Schema-1 `bibi.toml` is the only
authoritative project state; `references.bib` is explicit output, written by
`bibi export` and never read back. Rust edition 2024, MSRV 1.88.

`REDESIGN.md` is the design authority and `IMPLEMENTATION.md` the crate-level
contract. Where they disagree, the design wins.

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
cargo run -p bibi -- sync
cargo run -p bibi -- export
cargo run -p bibi -- check references.bib
cargo test -p bibi-inspire --test e2e -- --ignored  # live INSPIRE, network required
```

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
bibi-provider       bibi-manifest    bibi-documents
     │                    │                │
     ▼                    └───────┬────────┘
 bibi-inspire                     ▼
                          bibi-application
                                  │
                                  ▼
                                 bibi
```

- `bibi-bibtex`: the whole boundary around BibTeX syntax — scanning entry and
  key spans, structural validation, re-keying, identifier candidates, local
  metadata projection, deterministic rendering.
- `bibi-core`: provider-neutral domain values — `BibiId`, `ProviderName`,
  `ProviderId`, `Revision`, `Doi`, `ArxivId`, `Record`, locators, selection,
  filtering.
- `bibi-provider`: the object-safe provider contract, provider-neutral result
  types, the ordered registry, and the local provider.
- `bibi-inspire`: INSPIRE transport, pure mapping, batching, the verified texkey
  join, pacing, and retry.
- `bibi-manifest`: schema-1 TOML, candidate validation, indexes, optimistic
  concurrency, atomic writes.
- `bibi-documents`: the global arXiv PDF/source cache.
- `bibi-application`: command use cases over injected stores and providers.
- `bibi`: Clap definitions, bootstrap, output routing, exit codes.

Two couplings are forbidden by the graph: `bibi-manifest` never names a
provider crate, and `bibi-application` never depends on a concrete network
provider.

## Key decisions

### Provider-owned metadata

Every record is owned by a provider (I1). Identifiers and description are
derived from the provider's **structured record**, not from BibTeX — author
strings, collaborations, and the chosen year are all things a BibTeX rendering
loses. bibi never interprets provider BibTeX as metadata (I3); the only
semantic parse in the tree is `local_metadata`, used by the local provider,
where the user's BibTeX *is* the original.

Providers differ in capability, not in kind. The local provider ingests
user-supplied BibTeX and supports no refresh. **Nothing outside `bibi-provider`
may test `provider.name() == "local"`** — commands branch on capability.

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
no cache probing, no ambient state. `export`, `check`, and `list --format
bibtex` share one `render_manifest` entry point; every other BibTeX-producing
path calls `bibi_bibtex::render` directly, so separator and trailing-newline
rules live in exactly one place.

Plain `export` and `check` are offline. `export --provider` syncs that provider
first, reloads the *committed* manifest, and writes nothing if sync reported a
failure.

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

### Scope

A manifest is `bibi.toml`, resolved by the first rule that applies: `-p/--path`,
then `-g/--global`, then `./bibi.toml`. **There is no upward search.** Outputs
resolve against the manifest directory; inputs (`add -f`, `check`) resolve
against the working directory. Only `init`, `add`, and `add -f` may create a
missing manifest.

There are no shelves, no library registry, no grouping, and no Git integration.

## Error handling

Library crates use typed `thiserror` enums. The binary uses `anyhow` for
context. Add typed library variants for new failure modes.
