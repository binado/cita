# Implementing bibi

## Context

`cita` is a working Git-friendly bibliography CLI at v0.5.0 (~10.3k LOC across six
crates). `REDESIGN.md` supersedes it with `bibi`: a clean product break with no
migration path, built on a different central bet — bibliographic metadata is
**provider-owned and derived from structured provider records**, not projected
from BibTeX; the `.bib` is explicit output rather than a continuously maintained
artifact; and shelves, the library registry, and Git integration are removed
entirely. `IMPLEMENTATION.md` translates that into seven libraries plus a binary,
with a detailed per-crate contract.

This plan is the **execution layer** for those two documents. It does not restate
their contracts — where a milestone below says "implement §7", the authority is
`IMPLEMENTATION.md`, and where that disagrees with `REDESIGN.md`, the design wins.
What this plan adds is repo-level sequencing, the concrete file surgery, the
release-surface changes, and per-milestone verification gates.

**Intended outcome:** a `bibi` workspace at 0.1.0 satisfying `IMPLEMENTATION.md`
§18's Definition of Done, with the `cita` tree gone and CI green on 1.88 + stable.

---

## Decisions taken

| Decision | Choice | Consequence |
| --- | --- | --- |
| Code reuse | **Fresh crates, no reuse** | §16's reuse table is downgraded to a *reference* table: old code is read for its invariants and edge cases, never copied. Named files to consult are listed per milestone. |
| Git strategy | **One `bibi` branch, one commit per milestone** | `main` keeps a working `cita` until the final merge. No PR round-trips. |
| Release surface | **Reset to 0.1.0, repo renamed to `binado/bibi` now** | `repository =` points at the new URL from M0; **you** perform the GitHub-side rename. |
| Deletion timing | **Whole `cita` tree deleted in M0**, not at the end | With no reuse there is nothing to carry. Keeps CI fast and clippy signal clean for seven milestones. `main` and the `cita-v0.5.0` tag remain the working-tool fallback. |

Milestone numbering below maps onto `IMPLEMENTATION.md` §15 as: M0 replaces its
"delete, then rename" step, M1–M7 are its milestones 1–7, M8 is its milestone 8.
Its milestone 9 (ADS, DOI registry) is post-v1 and out of scope.

---

## M0 — Clean slate and workspace skeleton

Single commit. Ends with an empty-but-valid workspace that builds.

**Delete** the entire `crates/` tree (all six `cita-*` packages, their tests and
fixtures), `docs/CONTEXT.md` (describes shelves and source snapshots — both gone),
`refs.bib`, and the stale `.bibi/` and `.cita/` cache directories.

**Rewrite** the workspace root:

- `Cargo.toml` — `members` lists the eight new paths; `version = "0.1.0"`;
  `repository = "https://github.com/binado/bibi"`; keep `edition = "2024"`,
  `rust-version = "1.88"`. Workspace `[dependencies]` per §2's budget table: the
  existing set carries over (including `httpdate`, still needed for `Retry-After`),
  plus `uuid` with the `v4` feature and `directories`, both pinned to the newest
  major that builds on 1.88.
- `release-plz.toml` — rename `version_group` to `"bibi"`, one `[[package]]` per
  new crate.
- `.gitignore` — drop `/.cita/files/` (the cache is global now); keep
  `*.bib` / `!references.bib`; add `/.bibi/`.
- `.github/workflows/ci.yml` — update the `cargo install --path crates/cita` step
  to `crates/bibi`; the rest is path-agnostic.
- `AGENTS.md` / `CLAUDE.md` — replace wholesale. The current content documents
  shelves, `generate`, and the source-snapshot model, all of which are wrong now.
- `README.md` — placeholder stub; the real rewrite is M8.

**Scaffold** eight crates, each with `Cargo.toml`, `src/lib.rs` (or `main.rs`),
`README.md`, and a copy of `LICENSE`. The README and LICENSE copies are not
cosmetic: `.github/workflows/ci.yml`'s `package` job runs `cmp LICENSE
"$crate/LICENSE"` and greps the `cargo package --list` output for both files, so a
crate missing either fails CI.

```
crates/bibi-bibtex  bibi-core  bibi-provider  bibi-inspire
        bibi-manifest  bibi-documents  bibi-application  bibi
```

Dependency edges exactly per §2's graph — in particular `bibi-manifest` must **not**
depend on any provider crate (the `cita-manifest` → `cita-inspire-client` edge was
the coupling this redesign removes), and `bibi-application` must not depend on
`bibi-inspire`.

**Verify:** `cargo build --workspace && cargo clippy --workspace --all-targets -- -D warnings`

**You do:** rename the GitHub repo `binado/cita` → `binado/bibi`, then
`git remote set-url origin git@github.com:binado/bibi.git`.

---

## M1 — `bibi-bibtex`

Implement §4 in full: `CitationKey`, `BibtexEntry` (private constructor,
`parse_one` / `source` / `source_key` / `rekey` / `identifier_candidates` /
`local_metadata`), `parse_file`, and `render`.

The load-bearing design point, and the one most likely to be got wrong: the
**scanner owns entry boundaries and key spans, and `biblatex` is used only inside
`local_metadata`**. Under I3 the core must never interpret provider BibTeX
semantically, so `parse_one` and `parse_file` are purely syntactic.

The second: `identifier_candidates` and `local_metadata` are **separate
operations, and `parse_file` requires no title**. The old `cita-bibliography::parse`
called `validate_semantic` on every entry and rejected untitled ones — under
`REDESIGN.md` §6 that would make import refuse an entry carrying a DOI but no
title, which is precisely an entry a provider would resolve.

*Read for invariants (do not copy):* `crates/cita-bibliography/src/lib.rs` at the
`cita-v0.5.0` tag — `scan_raw_entries` (lines ~166–256) encodes the span,
gap-rejection, UTF-8-boundary, and closing-brace checks that took real debugging to
get right. Its `insert_field` / `validate_field_*` helpers are **not** ported:
field-level export policy is deferred (§17).

**Verify:** §4's test list — byte-preservation fixtures (braces, parens, TeX
accents, inline comments, CRLF), a property test that `rekey` changes only the key
span, an untitled-entry-with-DOI test where `identifier_candidates` succeeds and
`local_metadata` fails, and renderer golden files.

---

## M2 — `bibi-core` and `bibi-manifest`

Implement §5 and §8. Two crates in one milestone because the manifest's validated
constructors are what make the core newtypes meaningful.

`bibi-core`: `BibiId` (UUIDv4), `ProviderName`, `ProviderId`, `Revision`, `Doi`,
`ArxivId`, `Record`/`Provenance`/`Identifiers`/`Description`, `Locator` +
`QualifiedLocator` parsing, selector resolution order, `RecordFilter`, and the
`IdentifierChange` transition enum.

`bibi-manifest`: schema-1 TOML with `deny_unknown_fields` recursively, the
schema-number-first loader, `ManifestCandidate` with all five indexes rebuilt on
every load, `Generation` byte-comparison optimistic concurrency, and the atomic
commit sequence. Plus the §13 `atomic_replace` helper.

Watch: the TOML round-trip. The serializer may escape payload newlines, and the
decoded `BibtexEntry` string must be byte-equal to the input across LF, CRLF,
quotes, backslashes, and Unicode. This is where I2 either holds or quietly breaks.

*Read for invariants:* `cita-core/src/locator.rs` (arXiv version stripping, legacy
`hep-th/9901001` archive ids, DOI case-folding); `cita-manifest/src/lib.rs`
`atomic_write` (~line 872) for the fsync-then-rename sequence.

Also land the offline primitives that need no provider: `init`, `rename`, `remove`,
`show`, and `list` for every format except `--local`. `list --local` waits for M3
because it queries a *registry capability*, not the string `"local"`.

**Verify:** golden empty and populated manifests, serialize→parse→serialize byte
identity, every duplicate index rejected, newer-schema diagnostic, commit rejected
after an out-of-band edit, and failed validation leaving the original bytes intact.

---

## M3 — `bibi-provider`, local provider, `bibi-application` (offline), `bibi` binary

Implement §6, the offline half of §10, and §11. This is the milestone where the
tool becomes usable end to end without a network.

`bibi-provider`: the object-safe `Provider` trait with boxed futures, all entry
points plural, `ProviderCapabilities`, `Resolution`/`RefreshState`/`PayloadItem`,
the split `ProviderError { Retrieval, Mapping }`, `ProviderRegistry` with its
roster-order group-fallback (rule 4 of §6 — absence advances *as a subset*, errors
stop), and the registry's arity validation of every provider result.

`LocalProvider`: `ingest` via `BibtexEntry::local_metadata`, `UnsupportedLocator`
from `resolve`, `Unrefreshable` from refresh.

The rule to hold the line on: **nothing outside `bibi-provider` may test
`provider.name() == "local"`.** Commands branch on capability. `list --local` asks
the registry for ingest-without-refresh.

`bibi-application`: `Services`, `TargetResolver` (three rules, no parent search;
outputs manifest-relative, inputs cwd-relative), `BatchReport<T>` with skips
separated from failures, `add -f` local ingestion, and the `list --format json`
schema-1 projection.

`bibi` binary: Clap definitions, `bootstrap.rs`, output routing, exit codes
(0 / 1 / 2 per §11), broken-pipe quiet exit.

**Verify:** §6 and §10 test lists. Manual smoke test — `bibi init`,
`bibi add -f some.bib`, `bibi list`, `bibi show <key>`, `bibi rename`, `bibi remove`
— entirely offline, with a `.bib` of entries no provider would know.

---

## M4 — `bibi-inspire`: resolve and add

Implement §7's resolve half. The transport/mapping seam is literal: `transport` is
public and returns raw JSON/BibTeX wrappers (so diagnostics and fixture capture can
call retrieval alone) and owns pacing + retry; `mapping` is public and pure; `wire`
serde structs stay private; `provider` composes them.

Three things here are new relative to `cita` and deserve the most care:

1. **Batched resolve.** `arxiv:X or doi:Y or control_number:Z` in one search, with
   results matched back to input locators by *normalized identifier* — the JSON
   record carries its own identifiers, so no join protocol is needed. Two locators
   may legitimately resolve to one record; the provider does not deduplicate.
2. **The verified texkey join** (`join.rs`). Build `texkey -> control_number`,
   fail the batch if a texkey is claimed twice or an entry matches nothing, emit
   `None` for a record no entry claimed. The old `attach_bibtex`
   (`cita-inspire-client/src/client.rs` ~lines 116–141) is the shape to beat: it
   lacks duplicate-texkey rejection and treats per-record absence as a hard error.
3. **Proactive pacing on an injected clock.** A twelve-permit rolling five-second
   bucket, plus the corrected 429 policy: `Retry-After` clamped to **[5s, 60s]**.
   The old client honoured `Retry-After` verbatim, and any value under five seconds
   guarantees a second rejection that also costs quota. Both waits go through an
   injected sleep handle so the suite never sleeps for real.

*Read for invariants:* `cita-inspire-client/src/client.rs` `batch_ids` and
`encoded_query_len` (~lines 274–301) — the 100-record and 6 KiB bounds are
properties of INSPIRE's query endpoint and carry over unchanged.

Then wire `add <locator>` in `bibi-application`: provider texkey adoption,
`--key` override, duplicate/overwrite policy, and the §10 citation-key collision
rules (fail the item on `add`, skip on `add -f`, first-claim-wins within one
invocation, collision-identifies-target under `add -f --overwrite`).

**Verify:** hermetic `TcpListener` suites per §7's list, especially the five join
cases. Plus one `#[ignore]`d live test resolving a stable public record.

---

## M5 — Conditional sync

Implement §7's refresh half and §10's sync flow. Narrowed metadata batches, revision
comparison, `--force`, batched payload fetch, missing-record warnings, identifier
transition checks, uninstalled-provider skipping, and `SyncReport`.

The invariant that must not be compromised: **a record is updated as a unit.** If
metadata mapped but the payload did not arrive, nothing is written — above all not
the revision, because an advanced revision beside an old payload desyncs the two
permanently *and* suppresses the repair on the next sync.

**Verify:** an unchanged sync issues zero BibTeX requests; a forced sync of 300
records issues six requests total; an ambiguous join fails its own batch while other
batches and other providers still commit; a record under an uninstalled provider is
skipped by plain `sync` and is a usage error under `sync --provider`.

This milestone also answers `FEEDBACK.md` item 4 ("sync takes an extremely long
time") — conditional refresh plus narrowed field sets is the fix.

---

## M6 — Export and check

Implement §10's export/check. One `render_manifest` entry point shared by
`export`, `check`, and `list --format bibtex`; every other BibTeX-producing path
calls `bibi_bibtex::render` directly, so separator and trailing-newline rules exist
in exactly one place.

Export path safety compares **canonicalized** paths (canonicalize the parent, join
the file name, since the output usually does not exist yet) so no symlink or
alternate spelling can overwrite `bibi.toml`.

`export --provider` composes sync then render, reloading the *committed* manifest
before rendering so output is a function of published bytes. On any sync item
failure: successes commit, no bibliography is written, exit nonzero.

`check` is offline, accepts no `--provider`/`--force`, and reports a missing file as
a missing-file diagnostic rather than as drift.

**Verify:** plain export demonstrably offline (assert no HTTP client is even
constructed); §12's three determinism boundaries have golden fixtures.

---

## M7 — `bibi-documents`, fetch, cache clean

Implement §9. Global platform cache root via `directories`, addressed by normalized
arXiv id and artifact kind — never by record identity. `fetch --force` replaces one
artifact atomically; sync never touches this crate.

*Read for invariants:* `cita-documents/src/lib.rs` — the archive safety caps
(64 MiB compressed, 256 MiB decompressed, 10,000 files) and the rejection list for
absolute paths, traversal, symlinks, hard links, and device entries. These are the
security-relevant parts of the old tree and the ones worth re-deriving carefully.

`clean --all` must refuse a root that is empty, `/`, or a home directory.

**Verify:** exact modern and legacy (`hep-th/9901001`) cache paths, force
replacement, dry-run accounting, clean-all scope protection, malicious-archive
fixtures.

---

## M8 — Documentation and release surface

Rewrite `README.md` for the new command set and the manifest-as-only-state model.
Write a fresh `AGENTS.md`. Regenerate shell completions. Confirm `release-plz.toml`,
CI matrix, and the e2e liveness job.

**Exit gate:** `rg -i '\bcita\b'` returns only intentional clean-break notes, and
the full CI matrix passes on 1.88 and stable.

---

## Verification, every milestone

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace
cargo doc --workspace --no-deps          # CI runs this with -D warnings
```

The commit for a milestone is not made until all five pass. `cargo test --test e2e
-- --ignored` is run manually at M4 and M8 only.

---

## Risks I will surface rather than decide alone

- **`biblatex` may not expose what `local_metadata` needs** for collaboration
  extraction without brace-stripping that violates byte-preservation elsewhere. If
  so I will scope `local_metadata` narrowly and report the gap rather than
  hand-rolling a name parser.
- **INSPIRE's `updated` field as a revision token** is assumed stable and
  monotonic. If live testing at M4 shows it churning on non-bibliographic edits,
  conditional sync degrades to near-unconditional and the revision source needs
  reconsidering — a design-level question, not an implementation one.
- **`directories` crate MSRV** against Rust 1.88 is unverified; if it does not
  build I will pick the nearest equivalent and note the substitution.
- **`uuid` v4 requires a `getrandom` backend** that must build on the 1.88 CI
  toolchain; checked in M0, before anything depends on it.
