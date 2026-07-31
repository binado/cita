# bibi — implementation plan

Status: proposed implementation companion to [REDESIGN.md](REDESIGN.md).

This document translates the product and data-model decisions in `REDESIGN.md`
into Rust crate boundaries, public contracts, persistence rules, command flows,
and a delivery sequence. Where this document and `REDESIGN.md` disagree, the
design document wins.

This is a clean implementation break from cita. Packages, crate names, command
names, and manifest schemas are renamed directly to bibi. No compatibility
reader, migration command, or deprecated command aliases are built.

> **Partly superseded.** `BIBI_TWO_BINARIES.md` splits bibi into a project-local
> file manager and a deferred user-level library manager, and phase one of that
> document has landed. Wherever this document describes a global manifest and
> `-g/--global`, an `export` command, or a managed document cache with
> `cache clean`, it describes what bibi *was*: those are gone, `list --format
> bibtex` plus shell redirection is how a bibliography is written, and `fetch`
> downloads one or more files into the working directory or an existing output
> directory. Everything else here still
> holds. Full reconciliation waits until the second binary exists.
>
> **Provider architecture amendment.** `bibi-core::provider` owns the
> provider-neutral `RemoteProvider` contract, errors, outcomes, fake, and
> conformance suite. The contract uses `Send` RPITIT futures and is not
> object-safe. `bibi-inspire` depends only on core and BibTeX. `bibi-provider`
> depends on INSPIRE and exposes the closed `Provider::{Inspire, Local}` /
> `Providers` facade, exhaustive dispatch, direct local ingestion, construction
> overrides, and complete conditional-refresh decisions. Resolution selects
> one provider for the whole invocation, defaults to INSPIRE, and never falls
> back. `add -f FILE --provider local` replaces `--force-local`. This amendment
> supersedes the older crate graph, registry, capabilities, fallback, separate
> `bibi-sync` extraction, and import steps below.

---

## 1. Implementation principles

The following rules govern all crate-level decisions:

1. **The manifest is the only authoritative project state.** Provider responses,
   generated bibliographies, JSON list output, and downloaded documents are not
   alternative authorities.
2. **Network code does not enter the manifest crate.** Provider implementations
   return provider-neutral values; the application layer decides how those
   values mutate a candidate manifest.
3. **Provider response types do not cross provider crate boundaries.** INSPIRE
   JSON types remain private to `bibi-inspire`.
4. **BibTeX is manipulated structurally, never reformatted.** The bibliography
   crate owns every operation that scans, validates, splits, or re-keys BibTeX.
5. **Commands are application use cases, not Clap handlers.** The binary parses
   arguments and renders reports. Reusable workflows live in
   `bibi-application`.
6. **Every write validates a complete candidate first.** Atomic filesystem
   replacement publishes only validated bytes.
7. **Partial batches are explicit.** Successful items may commit, failures are
   returned in a typed report, and any failure produces a nonzero process exit.
8. **Offline paths stay offline.** Plain export, check, list, show, rename,
   remove, and manifest validation never issue a network request or consult the
   document cache.
9. **All library failures are typed.** Library crates use `thiserror`; only the
   binary uses `anyhow` to add command-level context.
10. **Tests are hermetic by default.** HTTP tests use local listeners. The only
    real-network test remains an explicitly ignored end-to-end test.

---

## 2. Target workspace

The initial target workspace contains seven libraries and one binary package:

```text
crates/
  bibi-bibtex/
  bibi-core/
  bibi-provider/
  bibi-inspire/
  bibi-manifest/
  bibi-documents/
  bibi-application/
  bibi/
```

The dependency direction is:

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
                          bibi-application ─────┐
                                  │            │
            bibi-provider ────────┤            │
            bibi-inspire ─────────┤            │
            bibi-documents ───────┘            │
                                  ▼            │
                                 bibi ◄────────┘
```

More precisely:

- `bibi-bibtex` has no dependency on another bibi crate.
- `bibi-core` depends on `bibi-bibtex`.
- `bibi-provider` and `bibi-manifest` depend on `bibi-core` and
  `bibi-bibtex`; `bibi-documents` depends only on `bibi-core`.
- `bibi-inspire` depends on `bibi-core` among workspace crates.
- `bibi-provider` depends on `bibi-core`, `bibi-bibtex`, and the compiled
  `bibi-inspire` implementation.
- `bibi-application` depends on the provider contract, manifest, documents,
  core, and BibTeX crates, but not on a concrete network provider.
- `bibi` depends directly on `bibi-application`, `bibi-provider`, and
  `bibi-documents`; the facade constructs concrete providers, while the binary parses
  CLI arguments, and writes stdout/stderr.

This graph prevents two unwanted couplings present in cita:

- the manifest crate cannot name INSPIRE or any future provider;
- the binary does not need to implement manifest mutation algorithms.

### Workspace-wide dependencies

Retain the existing MSRV of Rust 1.88 and edition 2024. Add workspace
dependencies for:

- `uuid`, with `v4`, for bibi record ids;
- `directories` for the platform-native config and cache roots.

Avoid adding:

- DuckDB or a SQL engine in v1;
- an async-trait macro when stable RPITIT supplies statically dispatched
  `Send` futures;
- a database, dataframe, or persistent JSON cache;
- a generic plugin framework before a second external provider requires it;
- a file-locking or cross-process coordination crate; §8 commits are unlocked;
- a configuration file format before a provider needs credentials (§11).

### Direct dependency budget

Keep direct dependencies narrow so crate ownership remains visible:

| Crate | Direct non-bibi dependencies |
| --- | --- |
| `bibi-bibtex` | `biblatex`, `thiserror` |
| `bibi-core` | `uuid`, `percent-encoding`, `url`, `thiserror` |
| `bibi-provider` | `thiserror` |
| `bibi-inspire` | `reqwest`, `serde`, `serde_json`, `httpdate`, `tokio`, `url`, `thiserror` |
| `bibi-manifest` | `serde`, `toml`, `tempfile`, `thiserror` |
| `bibi-documents` | `reqwest`, `flate2`, `tar`, `tempfile`, `url`, `thiserror` |
| `bibi-application` | `directories`, `serde`, `serde_json`, `thiserror` |
| `bibi` | `anyhow`, `clap`, `clap_complete`, `tokio`, `anstyle`, `terminal_size` |

Do not add a hashing dependency for stale-manifest detection. `Generation`
retains the exact bytes read and compares them with the bytes on disk
immediately before the atomic replace (§8).

`bibi` owns the async runtime: `#[tokio::main]` in the binary, plain `async fn`
in `bibi-application`, and no runtime dependency in any library but the two that
perform I/O. Application tests therefore run under whatever executor the test
harness supplies.

---

## 3. Shared domain model

The six record groups from `REDESIGN.md` become explicit domain types rather
than an unstructured record with many optional strings.

```rust
pub struct Record {
    pub id: BibiId,
    pub key: CitationKey,
    pub provenance: Provenance,
    pub identifiers: Identifiers,
    pub payload: BibtexEntry,
    pub description: Description,
}

pub struct Provenance {
    pub provider: Provider,
    pub provider_id: Option<ProviderId>,
    pub revision: Option<Revision>,
}

pub struct Identifiers {
    pub doi: Option<Doi>,
    pub arxiv: Option<ArxivId>,
}

pub struct Description {
    pub title: String,
    pub authors: Vec<String>,
    pub collaborations: Vec<String>,
    pub year: Option<i32>,
}
```

These types are defined in `bibi-core`, except `CitationKey` and
`BibtexEntry`, which are defined by `bibi-bibtex`.

All string-like domain values are private-field newtypes. Construction performs
normalization or validation once:

- `BibiId` wraps `uuid::Uuid` and serializes in canonical lowercase hyphenated
  form.
- `Provider` is a closed enum, `Provider::{Inspire, Local}`, not a validated
  string: the set this build carries is fixed at compile time, so a name
  outside it is rejected wherever it appears — a manifest field, a
  `--provider` flag, or a `<provider>:` locator qualifier.
- `ProviderId` and `Revision` are non-empty opaque strings. They are never
  parsed as numbers by generic code.
- `Doi` is trimmed and ASCII-lowercased.
- `ArxivId` is validated, lowercased where appropriate, and stripped of a
  trailing version.
- `CitationKey` is non-empty and matches `[A-Za-z0-9._:+-]+`, the exact safe
  grammar owned by `bibi-bibtex`.

`Record` construction is private to validated constructors. No crate should be
able to create a record whose payload is malformed, whose title is empty, or
whose local key cannot be written back into the payload.

---

## 4. `bibi-bibtex`

### Responsibility

`bibi-bibtex` is the complete boundary around BibTeX syntax. It:

- validates one standalone entry;
- parses a file containing only entries and whitespace;
- extracts the source texkey;
- structurally replaces only the texkey token;
- extracts identifier candidates from any entry, without requiring a title;
- projects full metadata from user-supplied/local-provider BibTeX;
- renders a deterministic sequence of locally re-keyed entries.

It does not:

- know about providers, manifests, UUIDs, or commands;
- serialize a manifest;
- generate BibTeX from structured metadata;
- semantically project provider-owned BibTeX.

### Modules

```text
src/
  lib.rs
  entry.rs
  scanner.rs
  file.rs
  local_metadata.rs
  render.rs
  error.rs
```

### Core types and API

```rust
pub struct CitationKey(String);

pub struct BibtexEntry {
    source: String,
    source_key: CitationKey,
    key_span: Range<usize>,
}

pub struct ImportedEntry {
    pub key: CitationKey,
    pub payload: BibtexEntry,
}

pub struct IdentifierCandidates {
    pub doi: Option<String>,
    pub arxiv: Option<String>,
}

pub struct LocalMetadata {
    pub title: String,
    pub authors: Vec<String>,
    pub collaborations: Vec<String>,
    pub year: Option<i32>,
    pub doi: Option<String>,
    pub arxiv: Option<String>,
}
```

`BibtexEntry` has a private constructor and exposes:

```rust
impl BibtexEntry {
    pub fn parse_one(source: String) -> Result<Self, Error>;
    pub fn source(&self) -> &str;
    pub fn source_key(&self) -> &CitationKey;
    pub fn rekey(&self, key: &CitationKey) -> Result<String, Error>;
    pub fn identifier_candidates(&self) -> IdentifierCandidates;
    pub fn local_metadata(&self) -> Result<LocalMetadata, Error>;
}

pub fn parse_file(source: &str) -> Result<Vec<ImportedEntry>, Error>;
pub fn render<'a>(
    entries: impl IntoIterator<Item = (&'a CitationKey, &'a BibtexEntry)>,
) -> Result<String, Error>;
```

`BibtexEntry::source` is the exact entry span from `@` through its matching
closing delimiter. Whitespace outside that span belongs to the containing
single-entry response or multi-entry file, not to the stored entry. All bytes
inside the span, including internal leading/trailing whitespace and comments,
are preserved. This lets import discard separators while satisfying the
byte-preservation invariant for each entry.

The implementation should retain and simplify the existing raw-span scanner.
The scanner, not `biblatex`, owns entry boundaries and key spans. `biblatex` is
used only by `local_metadata`, because local BibTeX is the original structured
source for that provider.

**Identifier extraction and metadata projection are separate operations, and
import needs only the first.** `identifier_candidates` reads the `doi` and
`eprint`/`archiveprefix` fields and returns whatever it finds; it has no required
field and cannot fail. `local_metadata` is the full projection and does require
a title. Fusing them would make import reject an entry that carries a DOI but no
title — an entry a provider would resolve, supplying the title itself — which
contradicts `REDESIGN.md` §6: import resolves, and discards the parse result.

### Structural validation

`parse_one` accepts exactly one entry and surrounding whitespace only. It
rejects:

- directives such as `@string`, `@preamble`, and `@comment`;
- non-entry text and comments outside an entry;
- malformed delimiters;
- multiple entries;
- a missing source key;
- keys outside the accepted key grammar;
- an entry that cannot be re-keyed without changing bytes outside the key span.

`parse_file` accepts entries separated by whitespace. It rejects duplicate
source keys and returns entries in source order so application reports can
preserve input order. It projects no metadata, so an untitled entry parses
successfully and is resolved against a provider like any other.

### Rendering

The renderer receives records already filtered and sorted by the caller. It:

1. re-keys each raw entry to its stored local key;
2. joins entries with exactly one blank line;
3. appends exactly one final newline;
4. emits an empty string for an empty selection.

It never reads description or identifiers. Golden tests must prove that changes
to description cannot change rendered bytes.

### Error model

Use one public `Error` enum with variants for:

- invalid key;
- malformed entry;
- unexpected content;
- unsupported directive;
- duplicate key;
- missing title for local metadata;
- semantic parse failure in local metadata;
- unsafe or ambiguous re-key span.

Errors include byte offsets where the scanner has them, but do not expose
internal parser types.

### Tests

Port existing scanner and re-keying tests first. Add:

- byte-for-byte fixtures containing braces, parentheses, TeX accents, inline
  comments, CRLF, and trailing whitespace;
- property tests asserting that re-keying changes only the key span;
- multi-entry import fixtures with duplicate keys and directives;
- local projection tests for author parsing, missing titles, and
  collaboration-like author fields;
- identifier-candidate tests, including an untitled entry with a DOI, which must
  parse and yield its identifier while `local_metadata` on the same entry fails;
- renderer golden files for ordering, separation, and terminal newline.

---

## 5. `bibi-core`

### Responsibility

`bibi-core` owns provider-neutral domain values and pure record operations. It
does not perform I/O or depend on serde wire shapes from external APIs.

### Modules

```text
src/
  lib.rs
  id.rs
  identifiers.rs
  locator.rs
  provider_name.rs
  record.rs
  selector.rs
  filter.rs
  error.rs
```

### Identity and identifiers

`BibiId::new()` mints UUIDv4. Parsing accepts only canonical UUID text.
The manifest validator remains responsible for detecting duplicates in one
candidate manifest.

`Doi` and `ArxivId` retain the current normalization behavior:

- DOI comparison is case-insensitive after trimming;
- arXiv versions are stripped;
- legacy archive identifiers such as `hep-th/9901001` remain valid;
- cache paths may retain the legacy archive slash only after validation.

Identifiers are single optional canonical values. A successful refresh treats
the selected provider as authoritative and replaces complete provider-owned
metadata, including identifier additions, replacements, and removals. The
manifest-owned bibi id and local citation key remain stable.

### Locators

Parse command-line input into:

```rust
pub struct QualifiedLocator {
    pub provider: Option<Provider>,
    pub locator: Locator,
}

pub enum Locator {
    Arxiv(ArxivId),
    Doi(Doi),
    ProviderId(String),
}
```

Recognize:

- `arxiv:<id>`, `doi:<doi>`, and canonical URLs;
- syntactically valid `<provider>:<id>` candidates, leaving installed-provider
  validation to the registry;
- unqualified arXiv identifiers;
- unqualified provider ids only when the provider registry proves their syntax
  belongs to exactly one provider.

Parsing generic syntax belongs in core. Deciding whether a provider supports a
locator belongs in `bibi-provider`.

### Selection

Selection is pure over a manifest index. Resolution order is:

1. exact local key;
2. qualified provider id;
3. normalized DOI;
4. normalized arXiv id.

Ambiguity is always an error. Bibi UUIDs are intentionally absent from the
ordinary selector parser.

### Filtering

Define `RecordFilter` with:

- provider equality;
- local-provider capability marker supplied by the application;
- case-insensitive author substring;
- case-insensitive title substring;
- exact year.

Filtering preserves manifest sort order. It is advisory and never participates
in selector resolution.

### Tests

Use table tests for every accepted and rejected locator, normalization edge
case, selector precedence, identifier transition, and filter combination.

---

## 6. `bibi-provider`

### Responsibility

`bibi-provider` defines the object-safe provider contract, common result types,
the ordered provider registry, and the local provider. It contains no external
HTTP API schema.

### Modules

```text
src/
  lib.rs
  contract.rs
  registry.rs
  local.rs
  outcome.rs
  error.rs
```

### Provider-neutral values

`ProviderOwned` (defined in `bibi-core`) is the single shape for a provider's
complete contribution — provenance, identifiers, description, and payload.
Providers return it from `resolve`/`ingest`; the application writes it into the
manifest. There is no parallel provider-crate twin of that type.

```rust
pub struct RefreshRequest {
    pub bibi_id: BibiId,
    pub provider_id: ProviderId,
    pub stored_revision: Option<Revision>,
}

pub struct ProviderMetadata {
    pub provider_id: ProviderId,
    pub revision: Option<Revision>,
    pub identifiers: Identifiers,
    pub description: Description,
    /// Ephemeral join hints for a subsequent fetch_payloads. Never persisted.
    pub join_tokens: Vec<String>,
}

pub struct PayloadRequest {
    pub provider_id: ProviderId,
    pub join_tokens: Vec<String>,
}

pub enum Resolution {
    Found(ProviderOwned),
    NotFound,
    UnsupportedLocator,
}

pub enum RefreshState {
    Metadata(ProviderMetadata),
    Missing,
    Unrefreshable,
}

pub struct RefreshItem {
    pub bibi_id: BibiId,
    pub result: Result<RefreshState, ProviderError>,
}

pub struct PayloadItem {
    pub provider_id: ProviderId,
    pub payload: Option<BibtexEntry>,
}
```

**Every provider entry point is plural.** `resolve` takes a slice of locators
and `fetch_payloads` a slice of [`PayloadRequest`], because both are driven by
operations that routinely carry hundreds of items — `add -f` over a colleague's
bibliography, and `sync --force` over a whole project — and because a provider is
the only party that knows how its API batches. A provider that cannot batch loops
internally; a provider that can, like INSPIRE, issues one search per hundred
records. Making the contract singular and batching above it would push
provider-specific batch limits into the application layer, which is precisely
what §1's second principle forbids.

`resolve` returns one `Resolution` per input locator, positionally. `PayloadItem`
carries `None` when a batch came back short a record — absence, reported and
survivable — while ambiguity inside a batch is a `ProviderError` over the whole
call (§7).

Provider results never contain a local key or bibi id for a new record. The
application mints identity and adopts or overrides the payload texkey.

### Object-safe contract

Use boxed futures so heterogeneous providers can be stored behind `Arc<dyn
Provider>` without exposing provider-specific response types:

```rust
pub type ProviderFuture<'a, T> =
    Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait Provider: Send + Sync {
    fn name(&self) -> &ProviderName;
    fn capabilities(&self) -> ProviderCapabilities;
    fn recognizes_unqualified_id(&self, value: &str) -> bool;

    fn resolve<'a>(
        &'a self,
        locators: &'a [Locator],
    ) -> ProviderFuture<'a, Result<Vec<Resolution>, ProviderError>>;

    fn refresh_metadata<'a>(
        &'a self,
        requests: &'a [RefreshRequest],
    ) -> ProviderFuture<'a, Vec<RefreshItem>>;

    fn fetch_payloads<'a>(
        &'a self,
        requests: &'a [PayloadRequest],
    ) -> ProviderFuture<'a, Result<Vec<PayloadItem>, ProviderError>>;

    fn ingest(
        &self,
        entry: BibtexEntry,
    ) -> Result<Resolution, ProviderError>;
}
```

Network providers normally return `UnsupportedLocator` from `ingest`. The local
provider supports ingest, returns `Unrefreshable` from refresh, and performs no
network I/O.

`ProviderCapabilities` describes supported locator kinds, refresh support, and
ingest support. Commands branch on capability, never on
`provider.name() == "local"`.

The registry validates every successful result before returning it:

- returned provenance names the provider that produced it;
- a refresh-capable provider supplies a provider id;
- a revision appears only with a provider id;
- the payload is already a structurally valid `BibtexEntry`;
- refresh returns exactly one result for every requested bibi id, with no
  unknown or duplicate ids;
- `resolve` returns exactly one `Resolution` per input locator, in input order;
- `fetch_payloads` returns exactly one `PayloadItem` per requested provider id,
  with no unknown or duplicated ids.

These arity checks are the generic half of the join guarantee. A provider owns
the texkey pairing inside its own response (§7); the registry owns the promise
that what comes back lines up positionally with what was asked for, so no caller
can mistake one record's result for another's.

### Registry

`ProviderRegistry` stores providers in deterministic roster order and indexes
them by `ProviderName`.

Resolution rules, now stated over a *set* of locators:

1. A qualified locator selects exactly one provider.
2. `--provider` constrains all inputs to that provider.
3. A qualifier and `--provider` that disagree fail before I/O.
4. For unconstrained resolution, the registry offers the whole unresolved set to
   the first provider in roster order, then offers the subset that came back
   `NotFound` or `UnsupportedLocator` to the next, and so on. Absence still
   advances; it simply advances in groups.
5. Any retrieval or mapping error stops the affected locators immediately. The
   registry does not fall through after an error, and a provider-level error
   fails every locator in that call rather than being silently retried
   elsewhere.
6. If all applicable providers report absence for a locator, the application may
   ask the ingest-capable provider to create a local record.

Rule 4 is why `resolve` is positional: the registry has to know which inputs came
back absent in order to build the next provider's batch.

The registry returns enough context for the CLI diagnostic to name the provider
that failed and suggest `--provider <other>`.

### Local provider

`LocalProvider` calls `BibtexEntry::local_metadata`, normalizes identifiers
through `bibi-core`, and returns the same `ProviderOwned` shape as a network
provider:

```text
provider       = "local"
provider_id    = absent
revision       = absent
payload        = original entry
description    = projected from original entry
identifiers    = normalized from original entry
```

Its `resolve` returns `UnsupportedLocator` for every input locator; its refresh
results are `Unrefreshable`; its payload is already present and is never
separately fetched, so `fetch_payloads` is unreachable for it.

### Error model

Keep retrieval and mapping failures separate:

```rust
pub enum ProviderError {
    Retrieval(RetrievalError),
    Mapping(MappingError),
}
```

`RetrievalError` distinguishes transport, HTTP status, authentication,
rate-limit exhaustion, and invalid configuration. `MappingError` covers missing
required fields, malformed identifiers, malformed BibTeX, and provider-contract
violations.

Not-found and unsupported-locator are outcomes, not errors.

### Tests

Use fake providers to verify:

- roster ordering;
- qualifier/flag compatibility;
- fallback after absence only, carrying the absent *subset* to the next provider
  while resolved locators stay resolved;
- no fallback after retrieval or mapping errors;
- ambiguous bare provider ids;
- local ingest and unrefreshable reporting;
- one refresh outcome for every request;
- positional arity for `resolve` and `fetch_payloads`, including a fake provider
  that returns too few, too many, or unknown ids.

---

## 7. `bibi-inspire`

### Responsibility

`bibi-inspire` implements the provider contract for INSPIRE. It owns HTTP,
INSPIRE wire structs, field mapping, batching, the texkey join, revision
handling, pacing, and retry policy.

### Modules

```text
src/
  lib.rs
  provider.rs
  transport.rs
  mapping.rs
  wire.rs
  batching.rs
  join.rs
  rate_limit.rs
  retry.rs
  error.rs
```

The retrieval/mapping seam is literal:

- the provider-specific `transport` API is public and returns raw JSON or raw
  BibTeX wrappers, so diagnostics and fixture capture can call retrieval alone;
  it also owns pacing and retry, so every path that reaches INSPIRE — including
  a diagnostic one — is rate-limited by construction;
- the provider-specific `mapping` API is public and is a pure function from a
  raw response to provider-neutral core values;
- serde `wire` structs remain private implementation details of mapping;
- `provider` composes the two halves and implements
  `bibi_provider::Provider`.

The public raw-response wrappers do not cross the generic provider contract.
Callers must name `bibi-inspire` explicitly to use them, so provider coupling is
visible and contained.

### Resolve flow

For a set of supported arXiv, DOI, or INSPIRE provider-id locators:

1. partition the input, answering `Resolution::UnsupportedLocator` positionally
   for kinds INSPIRE does not handle;
2. batch the remainder into search queries — `arxiv:X or doi:Y or
   control_number:Z` — under the same count and encoded-length bounds as
   refresh;
3. request structured JSON for each batch and map provider id, revision,
   identifiers, and description;
4. match each returned record back to its input locator by **normalized
   identifier**, which needs no join protocol: a JSON record carries its own
   arXiv id, DOI, and control number, so the association is in the payload
   rather than inferred;
5. request BibTeX for the resolved control numbers, batched, through the same
   join as refresh (below);
6. return one `Resolution` per input locator, in input order.

A locator no batch resolved is `Resolution::NotFound`; so is an HTTP 404 on a
single-record request. Other HTTP or decode failures are typed provider errors
and stop fallback.

Two locators in one call may resolve to the same INSPIRE record — a DOI and an
arXiv id for one paper, which is ordinary in an imported file. Both return the
same `ProviderOwned`, and the application's duplicate policy collapses them; the
provider does not deduplicate on the caller's behalf.

### Structured mapping

Private serde structs omit `deny_unknown_fields`; INSPIRE may add fields without
breaking bibi. Request and deserialize only:

- top-level record/control id and `updated`;
- `texkeys`, for the join;
- titles;
- authors' `full_name`;
- collaborations;
- publication years and preprint date;
- arXiv eprints;
- DOIs.

Mapping policy is deterministic:

- provider id is the numeric record/control id rendered as decimal;
- revision is the top-level `updated` string;
- title is the first non-empty title in provider order;
- authors retain provider order and use trimmed `full_name`;
- collaborations retain provider order and discard empty values;
- year is the first publication-info year, falling back to the year prefix of
  the preprint date;
- arXiv id is the first valid normalized eprint;
- DOI is the first valid normalized DOI.

Unknown and unused fields are discarded. Mapping does not parse or compare the
provider BibTeX.

### Refresh flow

`refresh_metadata`:

1. batches stable INSPIRE ids at no more than 100 records per batch and a 6 KiB
   encoded query;
2. requests only the structured fields listed above;
3. performs batches sequentially, through the shared rate limiter;
4. returns exactly one `RefreshItem` per requested bibi id;
5. maps missing records to `RefreshState::Missing`;
6. maps request/decode failures to item errors for every affected request;
7. returns each record's texkeys as `ProviderMetadata.join_tokens` for the
   subsequent payload fetch.

The 100-record and 6 KiB bounds are the current cita values and are kept: they
are properties of INSPIRE's query endpoint, not of the old refresh strategy.

The application compares returned revisions and calls `fetch_payloads` with one
`PayloadRequest` per changed or forced id, echoing that metadata's `join_tokens`.

### The texkey join

`fetch_payloads` issues one `format=bibtex` search per batch and pairs the
returned entries to the requested records. This is the mechanism `REDESIGN.md` §5
requires be *verified* rather than assumed, and it is the only place in bibi
where a provider response has no envelope.

`join.rs` implements it:

1. obtain declared texkeys from each `PayloadRequest.join_tokens`, or — when
   those tokens are empty — request `format=json` for the batch with
   `fields=texkeys` alongside the control number;
2. build a `texkey -> control_number` index, failing if any texkey is claimed by
   more than one record in the batch;
3. split the BibTeX response into entries with `bibi-bibtex`, which yields each
   entry's source key;
4. look up each entry's key in the index, failing if it matches nothing;
5. emit one `PayloadItem` per requested id, with `None` for ids no entry claimed.

Steps 2 and 4 are the ambiguity conditions and produce a `MappingError` over the
whole batch: if the response contains an entry bibi cannot place, or two records
claim one key, no pairing in that response is trustworthy and none is kept.
Step 5 is absence, which is unambiguous and survivable — the record is left
untouched and reported as a payload-absent sync absence.

A record's texkeys travel on `ProviderMetadata.join_tokens` from the metadata
pass into the matching `PayloadRequest`, so the refresh path needs no provider
session state and no extra lookup; only a cold `fetch_payloads` with empty tokens
(or `resolve`, which already holds the mapped texkeys in-process) pays for a
separate texkey request.

Note what makes this sound: INSPIRE declares the texkeys, bibi does not guess
them. The entry key is read syntactically by the scanner that already owns key
spans (I2), never parsed as metadata (I3). The join is an exact match on a
provider-declared unique token, with both non-injective cases rejected — not a
similarity judgment.

### Rate limiting and retry

INSPIRE documents a limit of **15 requests per IP address in any 5-second
window**, and states that requests rejected for exceeding it still count against
the quota, so a client must wait at least five seconds after a 429 before trying
again.

Two consequences shape the transport layer.

**Pacing is proactive, not reactive.** `rate_limit` holds a token bucket sized
conservatively below the documented limit — twelve permits per rolling
five-second window — shared by every request the client issues: resolve
searches, metadata batches, and payload fetches alike. Sustained throughput is
about 2.4 requests per second.

Batching does most of the work here, and pacing covers what batching cannot.
With batches of 100, a three-hundred-record project costs three requests to
refresh metadata and three more to fetch changed payloads, so the bucket never
binds on an ordinary operation. It binds when a project is large enough to need
tens of batches, when several commands run back to back, or when a future
provider batches less aggressively — and in those cases it is the difference
between waiting and being rejected, since rejected requests consume quota and
make the next attempt likelier to fail too.

Keeping the bucket even though batching made it rarely-reached is deliberate:
a rate limit that is only respected by accident of request volume is not
respected.

**Retry is the fallback, and `Retry-After` has a floor.** When a 429 arrives
anyway:

- retry at most three times after the initial request;
- wait `Retry-After` when parseable, clamped to at least 5 and at most 60
  seconds;
- wait 5 seconds when it is absent or unparseable;
- notify an injected observer immediately before each retry.

The clamp is a correction, not a refinement. The current policy honors
`Retry-After` verbatim, and any value below five seconds guarantees a second
rejection that also costs quota.

**Both waits go through an injected delay handle.** Pacing puts sleeps on the
*success* path, so a suite that slept for real would be unusable. `transport`
takes a clock/sleep handle; tests supply one that records requested durations and
returns immediately, and assert on the resulting schedule rather than on elapsed
time.

Other statuses are not retried in v1. The binary's observer writes retry notices
to stderr; tests inject a no-op or recorder.

### Tests

Keep all normal tests hermetic with local `TcpListener` servers:

- locator endpoint selection;
- 404 versus provider error;
- unknown JSON fields;
- author/collaboration/year mapping fixtures;
- narrow refresh query construction;
- count and encoded-size batching boundaries, for both resolve and refresh;
- batched resolve: mixed `arxiv:`/`doi:`/`control_number:` queries, matching
  results back to input locators positionally, partial batch results, and two
  locators resolving to one record;
- unchanged versus changed revision data;
- the texkey join: a clean bijection, an entry matching no requested record, one
  texkey claimed by two records, a record receiving no entry, and an entry
  carrying an alternate texkey rather than the primary one;
- malformed, empty, and multiple-entry BibTeX responses;
- 429 delays, the five-second floor, the sixty-second cap, and retry exhaustion;
- pacing: a burst of requests through the injected clock produces a schedule
  inside the documented window, and a 429 mid-run still observes the floor.

One ignored e2e test resolves a stable public INSPIRE record, confirms a valid
payload, performs a forced refresh, and does not assert volatile display text.

### Intended follow-on provider crates

The first implementation proves the contract with INSPIRE and local records.
The intended roster then adds provider-specific crates without changing
`bibi-core`, `bibi-manifest`, or command workflows:

- `bibi-ads` follows the same transport/mapping/provider split, reads its API
  token from the `BIBI_ADS_TOKEN` environment variable, and treats changed ADS
  record identifiers as an explicit migration rather than an automatic sync
  rebind. It owns its own pacing policy: a rate limit is a per-provider fact and
  does not belong in a shared abstraction.
- A DOI-registry crate is created only after selecting the concrete registry
  whose software/dataset BibTeX quality meets the design requirement. It exposes
  DOI resolution through the same provider contract.

Providers beyond INSPIRE, ADS, the selected DOI registry, and local are deferred.
There is no runtime plugin ABI in v1; adding a built-in provider is a new crate
plus one bootstrap registration.

---

## 8. `bibi-manifest`

### Responsibility

`bibi-manifest` owns schema-1 TOML, candidate validation, in-memory indexes,
optimistic concurrency, and atomic manifest writes. It does not know concrete
providers, HTTP, documents, CLI formatting, or generated bibliography paths.

### Modules

```text
src/
  lib.rs
  schema.rs
  store.rs
  candidate.rs
  indexes.rs
  mutation.rs
  atomic.rs
  error.rs
```

### Schema

The serialized shape is:

```toml
schema = 1

[[records]]
id = "d760f219-9098-4b49-9f62-10cbbcc22b11"
key = "Aad:2012tfa"
provider = "inspire"
provider_id = "1124337"
revision = "2026-07-27T12:34:56+00:00"
doi = "10.1016/j.physletb.2012.08.020"
arxiv = "1207.7214"
title = "Observation of a new particle ..."
authors = ["Aad, G.", "Abajyan, T."]
collaborations = ["ATLAS"]
year = 2012
bibtex = "@article{Aad:2012tfa,\n  ...\n}\n"
```

Optional TOML fields are omitted, never serialized as sentinel strings. Records
are serialized in local-key order. Struct field order above is canonical.

The wire structs use `#[serde(deny_unknown_fields)]` recursively. The loader:

1. parses the top-level schema number first;
2. reports a specific newer-schema error before decoding records;
3. rejects schema 0, missing schema, and all unsupported versions;
4. converts wire values through validated domain constructors;
5. validates the complete candidate and builds indexes.

The TOML serializer may escape payload newlines; the decoded `BibtexEntry`
string must remain exactly equal to the original input. Tests cover LF, CRLF,
quotes, backslashes, and Unicode. The implementation uses serde/TOML
serialization rather than a handwritten manifest writer.

### Candidate and indexes

`ManifestCandidate` owns a sorted `Vec<Record>` plus rebuilt indexes:

```text
by_id                 BibiId -> position
by_key                CitationKey -> position
by_provider_identity  (ProviderName, ProviderId) -> position
by_doi                Doi -> position
by_arxiv              ArxivId -> position
```

Validation rejects duplicates in every index. It also verifies:

- non-empty titles;
- every payload remains structurally valid;
- every local key can re-key its payload;
- every UUID is unique;
- provider id/revision fields satisfy domain constructors, and a revision never
  exists without a provider id;
- records are canonically sorted before serialization.

Manifest validation does not require the stored provider to be installed.
Offline rendering and inspection remain possible with only the manifest, and a
provider name the registry cannot supply is handled by the workflow that needed
it — skipped and counted by a plain `sync`, a usage error when the user named it
explicitly, and irrelevant to a `list --provider` query (§10).

Indexes are never serialized. Every load and candidate mutation rebuilds them,
which keeps the manifest authoritative and makes merge-introduced duplicates
detectable.

### Store API

```rust
pub struct ManifestStore {
    path: PathBuf,
}

pub struct LoadedManifest {
    pub manifest: Manifest,
    generation: Generation,
}

impl ManifestStore {
    pub fn load(&self) -> Result<LoadedManifest, Error>;
    pub fn load_or_empty(&self) -> Result<LoadedManifest, Error>;
    pub fn create_empty(&self) -> Result<(), Error>;
    pub fn commit(
        &self,
        expected: &Generation,
        candidate: ManifestCandidate,
    ) -> Result<Generation, Error>;
}
```

`Generation` is derived from the exact bytes loaded. It is opaque outside the
crate. It has a distinct missing-file state: `load_or_empty` returns an empty
candidate plus `Generation::Missing`, so a first write can distinguish "no
manifest, create one" from "manifest exists".

### Atomicity, and the deliberate absence of locking

**bibi takes no lock and creates no sidecar file.** The manifest remains the only
file bibi writes into a project directory. Atomic replacement satisfies I8 on its
own: a reader sees either the whole previous manifest or the whole next one,
never a partial write.

Commit is:

1. validate and serialize the candidate completely;
2. reread the manifest bytes and compare them with `Generation`;
3. fail with `StaleManifest` if they differ;
4. write a temporary file in the manifest directory;
5. flush and `sync_all` the temporary file;
6. atomically replace `bibi.toml`;
7. sync the containing directory where supported.

Step 2 is **best-effort detection, not mutual exclusion.** A window remains
between the comparison and the rename, so two processes committing in the same
instant can still lose one update. It is kept because it costs one comparison and
no dependency, and because it catches the case that actually occurs in practice:
a long `sync` or `export --provider` holding a manifest in memory across many
seconds of network work while the user edits the file in another terminal. The
diagnostic says the manifest changed since it was read and asks the user to
retry; it does not claim the write was serialized.

Concurrent bibi invocations against one manifest are outside v1's guarantees.
Adding real exclusion later is one advisory lock file and no change to this
sequence.

### Mutation API

The manifest crate provides pure candidate operations:

- insert a new record;
- replace an existing record while preserving id/key;
- remove by resolved position;
- rename a key;
- return sorted/filtered record references;
- resolve selectors through indexes.

It does not implement provider fallback, refresh, import, stdout reports, or
partial-batch policy. Those belong to `bibi-application`.

### Tests

Add golden and fault-injection tests for:

- canonical empty and populated manifests;
- rejection of every unknown field level;
- newer-schema diagnostics;
- all duplicate indexes;
- payload byte round trips;
- deterministic record and field ordering;
- interrupted temporary writes;
- commit rejection when the manifest changed after it was loaded;
- failed candidate validation leaving original bytes untouched;
- first-write creation and read-only missing-manifest behavior.

---

## 9. `bibi-documents`

### Responsibility

`bibi-documents` downloads, validates, atomically caches, and globally cleans
arXiv PDFs and source packages. It has no manifest dependency beyond receiving
a validated `ArxivId`. Opening a returned path or URL belongs to the binary.

### Modules

```text
src/
  lib.rs
  store.rs
  path.rs
  pdf.rs
  source.rs
  clean.rs
  error.rs
```

### Cache layout

The application supplies the platform cache root. The crate stores:

```text
<cache-root>/documents/arxiv/1207.7214/paper.pdf
<cache-root>/documents/arxiv/1207.7214/source/
<cache-root>/documents/arxiv/hep-th/9901001/paper.pdf
<cache-root>/documents/arxiv/hep-th/9901001/source/
```

The legacy slash is allowed only because `ArxivId` has already validated both
path components. No caller-supplied path is joined directly.

### API

Retain the useful current shape:

```rust
pub enum ArtifactKind { Pdf, Source }
pub enum FetchPolicy { UseCache, Force }
pub enum FetchOutcome { Cached(PathBuf), Downloaded(PathBuf) }

impl DocumentStore {
    pub async fn fetch(
        &self,
        id: &ArxivId,
        kind: ArtifactKind,
        policy: FetchPolicy,
    ) -> Result<FetchOutcome, Error>;

    pub fn clean(&self, mode: CleanMode) -> Result<CleanReport, Error>;
}

pub enum CleanMode { DryRun, All }
```

Returned paths are absolute.

### Download safety

Reuse the current protections:

- validate `%PDF-` before publishing PDFs;
- download to a same-directory temporary file;
- sync and atomically replace on `Force`;
- cap PDFs at 256 MiB while receiving the response;
- cap compressed source at 64 MiB;
- cap decompressed source at 256 MiB;
- cap extracted regular files at 10,000;
- reject absolute paths, parent traversal, symlinks, hard links, devices, and
  unsafe tar entry kinds;
- extract into a temporary directory and publish only a complete tree.

`fetch --force` replaces only the selected artifact. Sync never calls this
crate.

### Cleaning

`DryRun` walks only the known bibi document root and reports files/directories
and total bytes without deleting. `All` removes that exact root and recreates
nothing. It must reject a cache root that is empty, `/`, a home directory, or
otherwise fails the store's construction-time safety checks.

Cleaning is global and never consults manifests or reference counts.

### Tests

Port the current document tests, then add:

- exact modern and legacy cache paths;
- force replacement of an existing PDF and source tree;
- clean dry-run accounting;
- clean-all scope protection;
- concurrent fetch publication;
- source archive boundary limits and malicious-entry fixtures.

---

## 10. `bibi-application`

### Responsibility

`bibi-application` implements command use cases over injected stores and
providers. It is the only crate that coordinates network results with manifest
candidate mutations.

### Modules

```text
src/
  lib.rs
  services.rs
  target.rs
  reports.rs
  add.rs
  remove.rs
  rename.rs
  list.rs
  show.rs
  sync.rs
  render.rs
  export.rs
  check.rs
  fetch.rs
  cache.rs
  init.rs
  error.rs
```

### Injected services

```rust
pub struct Services {
    pub providers: Arc<ProviderRegistry>,
    pub documents: Arc<DocumentStore>,
    pub paths: PlatformPaths,
}
```

Each use case receives a `ManifestStore` for its resolved target. Tests inject
fake providers, temporary stores, and a local document server.

### Target resolution

`TargetResolver` implements exactly:

1. explicit `--path`;
2. `--global`;
3. `./bibi.toml`.

There is no parent search. A relative `--path` is made absolute against the
working directory, and its parent becomes the manifest directory.

Path arguments then split by role:

- **Outputs are manifest-relative.** The default `references.bib` and a relative
  `export --output` resolve against the manifest directory, so
  `-p other/bibi.toml` writes beside that manifest (`REDESIGN.md` §8).
- **Inputs are working-directory-relative.** `add -f <file>` and
  `check <bibfile>` resolve against the process working directory, because they
  name files the user's shell just completed. Resolving a tab-completed input
  path against some other directory is surprising in a way the output rule is
  not.

Platform paths use `directories`:

- global manifest: platform **config** directory plus `bibi/bibi.toml`,
  overridable by `BIBI_GLOBAL_MANIFEST`;
- document cache: platform cache directory plus `bibi`.

The global manifest is authored, reviewable, hand-editable content that users
will want to symlink into dotfiles — not derived state — so it belongs beside
configuration rather than in a platform data directory. v1 reads no configuration
file (§11).

Only `init`, `add`, and `add -f` may create a missing manifest. Read commands,
sync, removal, rename, check, fetch, and export report a missing target.
`TargetResolver` also returns whether parent creation is permitted: first-write
commands may create the fixed platform directory for the global manifest, while
an explicit or working-directory target requires its parent to exist.

### Init

`init` resolves the selected target, constructs the canonical empty schema-1
candidate, and creates it through the manifest store's atomic path. It refuses
to overwrite any existing file, including an invalid or newer-schema manifest.

For the fixed global target, `init` follows the returned parent-creation policy.
`init` does not construct providers or the document store.

### Optional positional input

The binary resolves omitted inputs for `add`, `fetch`, `remove`, and `show`
before constructing services or a manifest store. A testable resolver receives
the terminal status and a buffered reader, trims each UTF-8 line, and ignores
blank lines. Explicit positionals always win without reading stdin; `add -f`
remains its own mode, including the established `add -f -` BibTeX input.

An omitted positional on terminal stdin is a Clap-style usage error. Redirected
empty input is a successful no-op for the three batch commands and therefore
does not load or create a manifest. `show` reads the complete redirected stream
and requires exactly one selector. Stdin I/O failures are operational errors.
After resolution, `add --key` likewise requires exactly one locator.

### Add one or more locators

For `add <locator>...`:

1. parse every locator and preflight `--provider` compatibility;
2. reject `--key` with more than one locator;
3. load the manifest with `load_or_empty` and capture its generation;
4. resolve all locators through `ProviderRegistry` in one batched call;
5. for each found provider record:
   - use `--key` or adopt `payload.source_key()`;
   - mint a `BibiId`;
   - apply duplicate/overwrite policy to the candidate;
6. collect item failures without discarding prior successes;
7. validate the complete candidate;
8. on `--dry-run`, return the report without committing;
9. otherwise commit once against the captured generation.

Input order controls report order, not manifest order. The serialized manifest
is always sorted by local key.

Duplicate detection uses DOI, arXiv id, and provider identity. Without
`--overwrite`, a duplicate is skipped and its locally keyed BibTeX is still
returned. With overwrite, exactly one target must be identified by those
identities or by an existing `--key`; replacement preserves bibi id and local
key.

In overwrite mode, `--key` can identify an existing target but cannot rename it.
If deduplication identifies a target whose local key differs from a supplied
`--key`, the request is rejected instead of silently ignoring the flag. The user
can run `rename` as a separate explicit mutation.

### Citation-key collisions

`REDESIGN.md` §3 forbids silent suffixing, so every collision ends in an error or
an explicit overwrite. The outcome differs by command, because the two commands
have different escape hatches available.

**`add <locator>` fails the item.** If the adopted texkey or an explicit `--key`
already belongs to a *different* record — one duplicate detection did not
identify as the same work — the item fails and the diagnostic names both records
and suggests `--key`. That is exactly the escape hatch the design specifies, and
a single locator can use it.

**Within one invocation, the first claim wins.** Two locators whose provider
texkeys collide are unlikely but possible; the second is reported as a
duplicate-key failure. Collision detection therefore runs against the growing
candidate, not only against the manifest as loaded.

**`add -f` skips the entry unless `--overwrite` is given.** A file of imported
entries has no per-entry `--key`, so failing the item would leave the user no way
forward short of editing a colleague's bibliography by hand. The entry is
skipped, its BibTeX is still written to stdout, and the collision is explained on
stderr.

With `--overwrite`, a colliding key **identifies the overwrite target**: the
imported entry replaces the record holding that key, preserving its bibi id and
local key. This is the same mechanism as `add --overwrite --key <existing-key>`
(`REDESIGN.md` §10), reached from a file instead of a flag.

If a key collision and an identifier duplicate point at **different** existing
records, the item fails rather than picking one. That is the design's "zero or
multiple possible targets are errors", arrived at from two directions at once.

### Add a BibTeX file

For `add -f <file>`:

1. read stdin for `-`, otherwise the named file;
2. parse all entries before network I/O;
3. load the manifest with `load_or_empty` and capture its generation;
4. preserve each imported texkey as its local key;
5. extract DOI/arXiv candidates with `identifier_candidates`, which needs no
   title;
6. unless `--force-local`, resolve every extracted identifier through the
   constrained or ordered provider registry in one batched call;
7. after only `NotFound`/`UnsupportedLocator` outcomes, ingest through the local
   provider;
8. on retrieval/mapping error, fail that item and do not create a local record;
9. compare the retained or provider payload with the imported entry and record
   whether its bytes changed;
10. apply duplicate, overwrite, and key-collision policy per entry against the
    growing candidate;
11. commit once against the captured generation.

`--force-local` applies to every imported entry and performs no provider
resolution. Provider-resolved records store provider BibTeX but retain the
imported local key.

Duplicate policy matches `add <locator>`: an entry matching an existing record by
DOI, arXiv id, or provider identity is skipped unless `--overwrite` is given, and
its BibTeX is still emitted. Step 10 runs per entry against the candidate as it
grows, so two imported entries that duplicate *each other* fail one item rather
than being caught late by whole-candidate validation, which would discard the
whole batch and violate principle 7.

### Remove and rename

Remove resolves every selector against the original loaded manifest, rejects
ambiguous selectors, removes unique targets once, and emits the removed entries
re-keyed to their local keys. With `--dry-run` it returns the same report without
committing. It does not touch the document cache.

Rename accepts one selector and one new `CitationKey`. It rejects a collision,
changes only `Record.key`, preserves id and payload, validates, and commits once.

### List and show

List loads and validates the manifest, applies `RecordFilter`, and returns
records in local-key order. `--provider`, `--author`, `--title`, `--year`, and
`--local` populate the filter.

The `--local` filter asks the provider registry for the ingest-without-refresh
capability; it does not compare the stored provider name with the string
`"local"`. `--provider <name>`, by contrast, is a pure manifest query and never
requires that provider to be installed: it compares stored provider names, so a
provider this build does not carry still filters to the records it owns. The
binary checks the name against the providers it carries *plus* those the
manifest already names, so a name nothing knows is reported rather than silently
matching nothing. Only workflows that must *call* a provider — `add`, `sync` —
require it to be installed.

Output adapters:

- `table`: `KEY AUTHOR YEAR ARXIV TITLE`, sized and wrapped by the binary. The
  title takes the remaining width capped at half the terminal and wraps onto
  continuation lines indented to its own column; every other column is padded to
  its widest cell, measured by *display* width rather than `char` count. The
  author cell is the first collaboration, else the first author's family name
  with ` et al.` when there are more. `provider` is not a column — it is the same
  value on nearly every row, and `--fields provider` still asks for it;
- `bibtex`: shared deterministic renderer;
- `json`: a schema-1 `ListJsonRecord` array;
- `--fields <field>…`: tab-separated columns, one line per record, absent values
  rendered as empty columns. Mutually exclusive with `--format`, since a format
  says how to encode and a field says what to include.

`ListJsonRecord` contains exactly the fields specified in `REDESIGN.md` and
never contains payload. Optional values serialize as JSON `null`. The
application returns serialized UTF-8 ending in one newline; it never writes or
caches JSON.

Show resolves one selector and returns one standalone, locally re-keyed BibTeX
entry ending in one newline.

The three BibTeX-producing views share one application-level rendering entry
point:

```rust
pub struct RenderOptions {
    pub filter: RecordFilter,
}

pub fn render_manifest(
    manifest: &Manifest,
    options: &RenderOptions,
) -> Result<String, ApplicationError>;
```

`list --format bibtex`, export, and check call this function. It filters
records, preserves manifest order, and delegates byte production to
`bibi-bibtex`; it contains no command-specific formatting.

Every other BibTeX-producing path — `add`, `remove`, `rename`, `show` — calls
`bibi_bibtex::render` directly with the records it is reporting, including the
single-entry cases. No command module assembles BibTeX itself, so the separator
and trailing-newline rules exist in exactly one place and cannot drift between
stdout and a written bibliography.

### Sync

Sync is the most important batch workflow:

1. load manifest and generation;
2. group records by provider in registry order;
3. set aside every group whose provider name the registry cannot supply;
4. apply `--provider` by selecting one group;
5. ask each remaining provider for one metadata outcome per record;
6. count `Unrefreshable` records;
7. leave `Missing` records unchanged and add warnings;
8. compare revisions for metadata results;
9. unless forced, skip equal non-empty revisions;
10. treat absent revisions as changed;
11. fetch payloads for all changed records in one batched call per provider;
12. preserve bibi id and local key;
13. require the returned provider id to equal the stored provider id; a changed
    provider id requires explicit overwrite/provider migration;
14. accept added DOI/arXiv ids, but fail an item that replaces a non-empty id;
15. reject any new deduplication collision;
16. validate all successful replacements in one candidate;
17. commit once if at least one record changed.

Provider errors are item failures. Sync continues other records, commits valid
successes, returns a typed partial-failure report, and causes a nonzero CLI exit.
Missing records are warnings, not provider errors and not automatic rebinding.
`sync --dry-run` still performs metadata and required payload requests so it can
validate the complete candidate, but it does not commit.

**A record is updated as a unit.** If metadata mapped cleanly but the payload
did not arrive, nothing about that record is written — not the description, and
above all not the revision. Committing an advanced revision beside an old payload
would desync the two permanently *and* suppress the repair, because the next sync
would compare equal revisions and skip the record. The stored revision is left
alone and the next sync tries again.

Batched payloads make this a per-item rule with two sources. A `PayloadItem`
carrying `None` is that record's own absence: it is reported missing and every
other record in the batch proceeds. A `ProviderError` from `fetch_payloads` is an
ambiguous join (§7) and fails every record in that batch, because the response
gave no trustworthy pairing for any of them — but records in *other* batches, and
under other providers, still commit under the ordinary partial-batch rule.

**An uninstalled provider is a skip, not an error.** A manifest may name a
provider this build does not carry (§8), so a plain `sync` counts those records
as `unavailable`, warns once per provider name, and refreshes everything else.
Failing the entire refresh because one ADS record survives in a build without ADS
would make the manifest unmaintainable by the build that can still maintain most
of it. `sync --provider <uninstalled>` is a different case: the user named it, so
it is a usage error raised before any I/O.

`SyncReport` contains:

- examined;
- unchanged;
- refreshed;
- description changes;
- absences, each typed as provider-gone or payload-absent;
- unrefreshable;
- unavailable, grouped by provider name;
- item failures;
- whether the manifest was written.

### Export

Plain export:

1. loads the current manifest;
2. constructs `RenderOptions` from export filters;
3. renders through the shared application renderer;
4. resolves the default output as `references.bib` beside the manifest;
5. refuses any path resolving to the selected `bibi.toml`;
6. atomically writes the output.

Step 5 compares canonicalized paths, not path strings, so a symlink or an
alternate spelling of the same file is caught (`REDESIGN.md` §10). The output
path usually does not exist yet, so the comparison canonicalizes its parent
directory and joins the file name.

`export --provider <name>` first invokes sync for that provider. `--force` is
forwarded and is invalid without `--provider`. The sync provider is stored
separately from `RenderOptions`; it never populates the provider-equality filter
and therefore never limits which records are exported.

If preliminary sync has any item failure:

- successful sync updates may already have committed;
- no bibliography is written;
- the export report is a failure.

If sync succeeds, export reloads the committed manifest before rendering. This
ensures output is a pure function of bytes actually published as authority,
rather than of an uncommitted in-memory candidate.

The composed command suppresses sync's BibTeX stdout payload. Progress, retries,
and sync summary go to stderr; the export command owns the final output
contract.

### Check

Check is always offline:

1. render expected bytes using the same `RenderOptions` as export;
2. read the supplied bibliography bytes;
3. compare byte-for-byte;
4. return `Match` or `Drift`;
5. optionally construct a human-readable diff for stderr.

It never repairs, writes, syncs, or accepts provider/force flags. Drift maps to a
nonzero process exit without being represented as an I/O error.

A bibliography that does not exist is neither a match nor drift. It is reported
as a missing-file diagnostic naming the expected path, and exits nonzero.
Rendering a diff of the expected bytes against nothing would bury the actual
answer, which is that the file the user expected to check is not there.

The bibliography argument follows the target-relative path rule above.
`--diff` controls only whether a unified diagnostic is built; it is not part of
`RenderOptions` and cannot affect match versus drift.

### Fetch

Fetch is a batch application use case over a singular artifact client:

1. return an empty ordered report without loading a manifest for no selectors;
2. resolve adaptive `--output` semantics before loading the manifest;
3. load the manifest once and resolve every selector against that snapshot;
4. determine the PDF/source kind, canonical arXiv id, and destination;
5. turn invalid selectors and missing arXiv ids into per-item failures;
6. deduplicate equal artifact targets and reject destination conflicts;
7. preflight occupied destinations unless `--force` is set;
8. after the complete preflight, process ready downloads sequentially, creating
   and finishing one progress reporter per selector;
9. return one ordered success, skip, or failure outcome per selector.

With no output option, default filenames resolve in the working directory. An
existing output directory receives default filenames for one or many selectors.
For one selector a non-directory path is an exact destination; for multiple
selectors a missing or non-directory output is rejected before network access.
`--url` produces the same ordered batch without constructing a document client.

Successful unique paths or URLs go to stdout in input order. Skips and failures
go to stderr; skips do not affect exit status and any failure does. Downloads
remain atomic per file and partial across the batch. The singular
`bibi-documents` client retains its final race-safe no-clobber check.

### Reports and errors

Every use case returns a report rather than printing:

```rust
pub struct BatchReport<T> {
    pub successes: Vec<T>,
    pub skipped: Vec<SkippedItem>,
    pub failures: Vec<ItemFailure>,
}
```

The binary decides textual presentation. Reports preserve input order and
contain stable machine-relevant fields; prose is constructed at the CLI edge.

**Skips are not failures.** A refused duplicate, or an entry skipped for a key
collision, lands in `skipped` and does not affect the exit code: the requested end
state — that record present in the manifest under a stable key — already holds.
Only `failures` produce a nonzero exit, so `bibi add <locator> && make` proceeds
when the reference was already there, which is what a build script wants.

`ApplicationError` wraps typed lower-level failures and adds target resolution,
invalid flag combinations, and concurrent modification. Partial batch failure
is a successful report with `failures`, not an erased `anyhow` chain.

### Tests

Application tests use fake providers and temporary manifests to cover every
command flow, including:

- automatic first add;
- key adoption, override, collision, and preservation;
- fallback after absence but not provider error;
- import local fallback and force-local;
- partial success plus nonzero outcome;
- overwrite target ambiguity;
- key collisions: failed on `add`, skipped on `add -f`, and used as the
  overwrite target under `add -f --overwrite`;
- sync revision/force/identifier transitions;
- a payload absent from a batch, leaving that record and its revision untouched
  while the rest of the batch commits;
- an ambiguous join failing one batch while other batches and other providers
  commit;
- a record owned by an uninstalled provider, skipped by plain sync and rejected
  by `sync --provider`;
- export-provider success and failure ordering;
- check match, drift, and missing bibliography;
- JSON projection field set and absence of BibTeX;
- missing-manifest creation rules;
- concurrent modification between network work and commit.

---

## 11. `bibi` binary

### Responsibility

The binary is a thin adapter around `bibi-application`. It owns:

- Clap argument definitions and validation;
- platform path discovery;
- construction of concrete providers and document store;
- retry/progress observers;
- terminal-oriented tables and diagnostics;
- stdout/stderr routing;
- exit codes;
- shell completion generation.

It does not mutate manifests directly.

### Modules

```text
src/
  main.rs
  cli.rs
  bootstrap.rs
  output.rs
  commands/
    add.rs
    remove.rs
    rename.rs
    list.rs
    show.rs
    sync.rs
    export.rs
    check.rs
    fetch.rs
    cache.rs
    init.rs
```

Each command module converts Clap arguments to one application request and
renders its report.

### Provider bootstrap

The initial INSPIRE implementation constructs providers in this order:

1. INSPIRE;
2. local.

Local is ingest-only and therefore skipped automatically for locator resolution
until the application requests local fallback. ADS and a selected DOI provider
are follow-on crates registered in the intended roster order through the same
contract.

No provider implementation is selected by a `match` inside command code.

### Provider credentials

**v1 ships no configuration file.** INSPIRE requires no credential, so a config
format, its forward-compatibility policy, its path discovery, and its tests would
all ship with zero readers. Credentials come from the environment; `bibi-ads`
reads `BIBI_ADS_TOKEN` when that crate lands (§15, milestone 9).

`bootstrap.rs` reads a credential only while constructing the provider that needs
it, and never copies it into `bibi.toml`, reports, or diagnostics. A missing
credential is a typed provider-configuration error, not `NotFound`, so it cannot
trigger fallback (`REDESIGN.md` §4).

A configuration file returns when something needs it that the environment serves
badly. The `-p @thesis` alias of `REDESIGN.md` §9 is the likely trigger, not
credentials.

### CLI preflight

Clap enforces structural conflicts where possible:

- `--path` conflicts with `--global`;
- `fetch --force` conflicts with `--url`;
- `export --force` requires `--provider`;
- `add -f --force-local` conflicts with `--provider`;
- `cache clean` requires exactly one of `--dry-run` and `--all`;
- `add --key` is accepted only for one locator;
- `check` has no provider or force option.

Semantic provider-qualifier mismatches are validated by the application before
network I/O.

### Output and exit codes

Use:

- exit 0 for full success, a run whose only non-successes were skips, and a
  check match;
- exit 1 for operational errors, any item failure, check drift, and a
  bibliography missing under check;
- exit 2 for Clap usage errors.

Stdout contains only command results:

- BibTeX for add/remove/rename/show and `list --format bibtex`;
- a table, JSON, or `--fields` columns for list;
- one path or URL per unique successful fetch artifact;
- no progress prose.

Stderr contains warnings, retries, progress, summaries, diffs, and failure
diagnostics.

When output is piped, broken-pipe errors terminate quietly rather than printing
an error.

### Removed cita functionality

Do not port:

- `cita generate`;
- continuously maintained `references.bib`;
- `cita import` as a separate verb;
- library/shelf commands and registry;
- Git staging or commit commands;
- parent-directory discovery;
- compatibility with cita package names or schemas.

---

## 12. Manifest and output determinism

Determinism is tested at three boundaries:

### Manifest bytes

For one logical candidate:

- records are local-key sorted;
- fields serialize in schema order;
- optional fields have one representation;
- UUID, DOI, arXiv, and provider names are canonical;
- output ends with one newline.

Serialize, parse, and serialize again must be byte-identical.

### Bibliography bytes

For one manifest and `RenderOptions`:

- only local key and raw payload participate;
- records are local-key sorted after filtering;
- entries have one blank line between them;
- non-empty output has one final newline; an empty selection is zero bytes;
- no cache, clock, environment, provider registry, or working directory is read.

### JSON list bytes

For one manifest and `RecordFilter`:

- records are local-key sorted;
- object fields follow the schema-1 order;
- all optional fields are present as `null`;
- payload is absent;
- output is a JSON array plus one newline;
- output is generated in memory and never persisted by bibi.

Golden fixtures cover all three boundaries.

---

## 13. Atomic output helper

Manifest and export writes share a low-level same-directory atomic-write helper.
Only manifest commits precede it with the generation comparison of §8; exports
overwrite unconditionally, because a rendered bibliography is output rather than
authority.

```rust
fn atomic_replace(path: &Path, bytes: &[u8]) -> Result<(), AtomicWriteError>;
```

The helper:

1. rejects a destination without a parent;
2. creates the parent only when the caller explicitly permits it;
3. creates a temporary file in that parent;
4. writes all bytes;
5. flushes and syncs;
6. atomically replaces the destination;
7. syncs the parent where supported;
8. cleans up the temporary file on failure.

Export path safety is checked before the helper. Document publication uses its
own file/tree helpers because source artifacts are directories.

---

## 14. Testing layers

### Unit tests

Each library tests its domain invariants without invoking the binary.

### Contract tests

`bibi-provider` exposes a provider contract test suite callable by
`bibi-inspire` and fake providers. It verifies outcome completeness, provider
name stability, payload validity, and capability behavior.

### Application integration tests

Exercise use cases with fake providers and temporary files. These tests assert
typed reports and bytes rather than terminal prose.

### CLI integration tests

Run the compiled `bibi` binary and assert:

- exit status;
- exact stdout;
- relevant stderr fragments;
- filesystem results.

Use fixture providers through test-only base URL/environment injection. Never
depend on the real network.

### Fault injection

Provide test-only hooks around:

- manifest temp-file write;
- sync after some successful records, and after metadata but before payload;
- export after preliminary sync;
- document download before publication;
- generation comparison;
- the injected INSPIRE clock, for pacing and retry schedules.

Every injected failure must leave either the old complete state or the new
complete state, never partial bytes.

### CI

CI runs:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace
```

Run on Rust 1.88 and stable. The ignored INSPIRE e2e test remains a separate
network liveness job.

---

## 15. Delivery sequence

Implement as vertical milestones that keep the workspace compiling:

### Milestone 1: delete, then rename

Deletion comes first. This is a clean break with no users, so there is no reason
to rename, keep compiling, and keep green the subsystems v1 removes — shelves and
the library registry, Git integration, `generate`, parent discovery, and
derived-export field policy are a large fraction of the current tree, and
carrying them through seven milestones only to delete them at the end is waste.

- delete `cita-manifest::library`, `cita/src/git.rs`, the library/shelf command
  tree, `generate`, parent-directory discovery, and field-insertion export
  policy;
- delete the tests that cover only those paths;
- rename the surviving workspace packages and the binary to bibi;
- extract the raw BibTeX scanner into `bibi-bibtex`;
- implement `CitationKey`, `BibtexEntry`, file parsing, identifier candidates,
  re-keying, and the renderer.

Exit criterion: strict BibTeX fixtures and deterministic rendering pass, and no
shelf, library, or Git code survives to be renamed later.

### Milestone 2: domain and manifest

- implement bibi-core newtypes and records;
- define schema-1 TOML;
- implement indexes, selector support, candidate validation, generation checks,
  and atomic commit;
- add init, rename, remove, show, and list primitives.

`list --local` waits for milestone 3: it asks the provider registry for a
capability rather than comparing a stored provider name, so it cannot land before
the registry exists.

Exit criterion: offline manifest lifecycle and golden round trips pass.

### Milestone 3: provider contract and local ingestion

- add `bibi-provider`, registry, fake-provider tests, and `LocalProvider`;
- implement add-file local ingestion and JSON list projection;
- enforce absence-versus-error fallback semantics.

Exit criterion: complete offline/local bibliography workflows pass.

### Milestone 4: INSPIRE resolve and add

- split current INSPIRE client into transport and pure mapping;
- add structured authors, collaborations, year, identifiers, revision, and
  texkeys;
- implement batched resolve and the verified texkey join;
- add proactive pacing and the corrected 429 floor, both on an injected clock;
- implement provider texkey adoption and `--provider`.

Exit criterion: hermetic add tests and ignored live resolve test pass.

### Milestone 5: conditional sync

- implement narrowed metadata batches;
- implement revision comparison, force, batched payload fetch, missing warnings,
  identifier transition checks, and partial reports.

Exit criterion: unchanged sync performs no BibTeX request; a forced sync of three
hundred records issues six requests, preserves id/key, and commits atomically;
an ambiguous join fails its batch without touching other batches.

### Milestone 6: export and check

- implement pure renderer entry points;
- add atomic export, output protection, filters, and check;
- compose `export --provider` with sync and suppress output on sync failure.

Exit criterion: plain export is demonstrably offline; provider export ordering
and failure behavior pass.

### Milestone 7: documents and cleanup

- adapt the existing document crate to the global bibi cache;
- add force fetch, dry-run clean, and clean-all;
- wire fetch URL/open/source behavior.

Exit criterion: cache paths, archive safety, force replacement, and cleanup
scope tests pass.

### Milestone 8: documentation and release surface

Code deletion happened in milestone 1; what remains is everything around it.

- update README, shell completions, release metadata, and CI;
- audit public docs and error prose for cita terminology.

Exit criterion: `rg -i '\bcita\b'` finds only historical clean-break notes where
intentional, and the full CI matrix passes.

### Milestone 9 (post-v1): intended provider roster

- add `bibi-ads`, credential loading, captured mapping fixtures, and hermetic
  transport tests;
- select the DOI registry for software/datasets based on returned BibTeX quality;
- add its provider crate, add a matching variant to `bibi-core`'s closed
  `Provider` enum, and place both providers between INSPIRE and local in
  roster order;
- add cross-provider absence/error and explicit-migration tests.

This milestone completes the intended first-three external provider roster. It
does not change the manifest schema or application command algorithms, and it is
**not part of the Definition of Done** (§18): v1 ships with INSPIRE and local,
and milestone 9 is the first work after it.

---

## 16. Existing-code disposition

The redesign is a clean product break, but correct low-level code can be reused:

| Existing component | Disposition |
| --- | --- |
| `cita-bibliography` raw scanner/re-keying | Move and simplify into `bibi-bibtex`. |
| `cita-core` locator normalization | Port into validated bibi-core newtypes. |
| `cita-core` provider trait | Replace with object-safe multi-provider contract. |
| `cita-inspire-client` HTTP/retry/batching | Reuse transport pieces; replace snapshot and mapping model. |
| `cita-inspire-client` search batching (`batch_ids`, encoded-query bound) | Reuse as-is for both resolve and refresh; the bounds are properties of the endpoint. |
| `cita-inspire-client` texkey pairing (`attach_bibtex`) | Reuse as the basis of `join.rs`, adding the duplicate-texkey rejection and per-record absence it currently lacks. |
| `cita-manifest` atomic file helper | Reuse after adding a generation check. |
| `cita-manifest` source enum | Delete; records use provider-neutral provenance. |
| `cita-manifest` bibliography verification/generation | Replace with explicit export/check application flows. |
| `cita-manifest::library` | Delete. |
| `cita-documents` safety and publication | Reuse with global cache layout and clean operation. |
| `cita` commands | Replace with thin adapters over `bibi-application`. |
| `cita` Git integration | Delete. |
| CLI/e2e fixtures | Port behaviorally; do not preserve old command compatibility. |

Reuse is based on invariant coverage, not compatibility. No old public API needs
to remain callable.

---

## 17. Deferred work

Do not include in v1:

- providers beyond the intended INSPIRE/ADS/DOI/local roster and runtime
  provider plugins;
- user-provided attachments;
- attachment integrity lockfiles;
- DuckDB, SQL, persisted JSON, or a derived query index;
- a global library feeding projects;
- tags, groups, shelves, aliases, or aggregate bibliographies;
- field-level export policy without a concrete publisher requirement;
- automatic provider rebinding;
- Git staging or committing;
- a user configuration file, including credentials in a file and `-p @alias`
  shorthand;
- cross-process locking, or any coordination file beside the manifest.

The crate boundaries leave these extensions possible without reserving schema
fields or implementing unused abstractions.

---

## 18. Definition of done

The redesign is implemented when:

- all packages and the binary are named bibi;
- schema-1 `bibi.toml` is the only authoritative project state;
- every record has immutable UUIDv4 identity and stable local key;
- provider-owned metadata comes from structured provider mapping;
- provider and local BibTeX remain byte-preserved;
- provider texkeys are adopted at ingestion;
- no provider error triggers silent fallback;
- sync is revision-conditional and forceable;
- refresh fetches changed BibTeX in batches through a verified texkey join, and
  an unplaceable entry or a duplicated texkey fails its batch rather than
  producing a pairing;
- INSPIRE traffic is paced below the documented rate limit rather than relying
  on retries to discover it;
- plain export and check are offline and deterministic;
- provider export syncs first and writes no output after a sync failure;
- JSON list output is generated on demand without payload or persistence;
- document cache operations are global, safe, and manifest-independent;
- partial batches commit only valid successes and exit nonzero, while skipped
  items exit zero;
- all writes are candidate-validated and atomic, and bibi creates no file in a
  project directory other than the manifest;
- MSRV/stable CI and hermetic test suites pass.
