# AGENTS.md

## Product

bibi maintains one project's bibliography as schema-1 `bibi.toml` and renders
BibTeX to stdout. The TOML is authoritative; `.bib` files are derived output.
There is no upward search or user-level store. Rust edition 2024, MSRV 1.88.

## Commands

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace
cargo doc --workspace --no-deps

cargo run -p bibi -- add 1207.7214
cargo run -p bibi -- import local.bib
printf '@misc{Piped,title={Piped}}' | cargo run -p bibi -- import
cargo run -p bibi -- list --format json
cargo run -p bibi -- list --fields key,year,arxiv-url
cargo run -p bibi -- sync
cargo run -p bibi -- list --format bibtex > references.bib
cargo run -p bibi -- check references.bib
cargo test -p bibi-inspire --test e2e -- --ignored
```

Use Conventional Commits: `<type>: <description>`.

## Architecture

```text
bibi-bibtex -> bibi-core -> bibi-manifest
                    |             |
                    v             v
              bibi-inspire -> bibi-provider -> bibi-application -> bibi

bibi-documents  (dormant workspace crate; no application or CLI dependency)
```

- `bibi-bibtex`: strict entry splitting/parsing, exact bytes, derived texkey,
  identifier candidates, local metadata, raw deterministic rendering.
- `bibi-core`: `RecordId`, `RecordState`, `Record`, `Bibliography`, `Locator`,
  `Source`, identifiers, filters, `ProviderName`, and the `Provider` trait.
- `bibi-manifest`: strict schema-1 conversion, exact-generation stale-write
  detection, and atomic publication of complete bibliographies.
- `bibi-inspire`: private INSPIRE transport, mapping, batching, pacing/retry,
  BibTeX retrieval, and verified texkey join.
- `bibi-provider`: closed provider selection and exhaustive dispatch.
- `bibi-application`: strict command use cases over injected stores/providers.
- `bibi`: Clap, stdin resolution, presentation, and exit codes.
- `bibi-documents`: retained and tested independently, but dormant.

Forbidden couplings: `bibi-manifest` never depends on a provider crate;
`bibi-application` never depends on a concrete network provider; the CLI and
application never depend on `bibi-documents`.

## Domain contracts

`RecordState` is complete replaceable state: `Source`, `Identifiers`,
`Description`, exact `BibtexEntry`. `Record` is `{ RecordId, RecordState }`.
Fields are private and exposed through read-only accessors.

`RecordId` is a canonical UUIDv4 minted on insertion and retained through
overwrite and sync. The texkey is derived from `RecordState::bibtex`; there is
no citation-key type, stored key, override, rekey, or rename.

`Source` is either `Local` or `Managed { provider: ProviderName, id:
ProviderId }`. `ProviderName` is the closed serialized remote roster; local is
not a provider.

`Bibliography` owns all indexes and mutations. Every mutation consumes a valid
aggregate and returns a fully valid successor plus ordered outcomes. Never add
a candidate type or expose partially mutated state.

Stable identity claims are provider identity, DOI, and arXiv id. Admission:

- reject policy: any stable match fails;
- overwrite policy: zero matches inserts, one UUID replaces while retaining
  that UUID, multiple UUIDs fail;
- texkey is uniqueness-only and never selects the overwrite target.

Duplicate incoming stable claims, divergent matches, duplicate texkeys,
repeated UUID replacements, and repeated removal targets fail the whole batch.

## BibTeX boundary

Stored BibTeX is byte-identical to provider or user input. Never reformat or
rewrite it. Rendering concatenates exact entry sources in derived-texkey order,
with one blank line between entries and one trailing newline.

`parse_file` is strict. One malformed entry or duplicate texkey fails the
whole stream. Key and field grammar come from `biblatex`; bibi does not impose
a narrower rekey-safe subset because keys are never rewritten.

Provider structured records own managed metadata. Do not infer managed
metadata from provider BibTeX. Local BibTeX is the original input, so
`local_metadata` is the sole semantic BibTeX projection.

## Provider contract

`Provider` has only `name()` and async `resolve(&[Locator])`. A successful call
returns exactly one complete positional `RecordState` per locator. Unsupported
locators, absence, count mismatch, mapping failures, and transport failures are
whole-call errors.

Add and sync use this same path. Sync sends managed provider identities,
verifies each returned identity is identical, retains UUIDs, and replaces
complete state unconditionally. There are no revisions or conditional refresh
outcomes.

INSPIRE bulk BibTeX pairing is verified from provider-declared texkeys. An
unclaimed entry, duplicate claim, duplicate payload, or missing payload fails
the complete call. Keep pacing and retry schedules deterministic under the
injected clock.

## Persistence

Schema remains `1` and has no compatibility layer. Wire records contain `id`,
`source`, managed-only `provider_id`, identifiers, description, and exact
`bibtex`; never add `key` or `revision` back. Reject unknown fields and unknown
provider names. Local `provider_id` is forbidden; managed `provider_id` is
required.

Loading converts wire records to `(RecordId, RecordState)` and calls
`Bibliography::restore`. Commit accepts a complete `Bibliography`, compares the
exact generation bytes, and atomically replaces `bibi.toml`.

## CLI contracts

- `add [LOCATOR]… --provider --overwrite --dry-run` is remote only.
- `import [FILE] --overwrite --dry-run` is local only. With no file it reads
  complete redirected stdin; interactive omission is usage error; `-` is a
  normal filename.
- add/remove retain newline-delimited stdin; show requires exactly one.
- `rename`, `fetch`, add `--file`/`--key`, and sync `--force` do not exist.
- `list --fields arxiv-url` is the download piping surface.
- All mutations parse/resolve/validate the entire input, commit once or not at
  all, and emit results only after success. Dry-run does all planning but no
  commit.
- Operational failures exit 1; Clap usage errors exit 2.

## Tests

The ignored INSPIRE e2e test is the only live-network test. Socket-based
transport tests bind loopback listeners; sandboxed runners may need network
permission even though the suites are hermetic. `bibi-documents` tests continue
to run independently to prove the dormant crate remains healthy.

Provider fakes script complete `resolve` batches. Application tests replace the
closed INSPIRE slot, and command tests exercise file/stdin import and removed
CLI surfaces.
