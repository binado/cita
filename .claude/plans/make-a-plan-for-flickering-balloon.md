# Fix findings from PR #15 review (schema-3 BibTeX-authoritative migration)

## Context

A 5-agent review of `feat/schema-3-bibtex-authoritative` (PR #15) found one confirmed
correctness bug and several test/documentation gaps introduced or exposed by the
migration that makes BibTeX authoritative (`Reference` content is now projected from
BibTeX with only a curated arXiv/DOI override persisted for INSPIRE entries). The user
asked for a plan covering all recommended actions, including the optional fast-follows
and minor cleanup items.

While reviewing, the user also decided: since cita has no released users yet, there's
no reason to keep incrementing the schema number for in-development breaking changes.
This PR renumbers the manifest schema to **1** instead of 3, discarding the "2 and 3
are rejected, no migration" ceremony that only matters once real users exist. This is
landed as a **new commit on top** of the existing pushed branch — the two already-
pushed commits (`70de02f`, `584250d`) keep their "schema 3" commit messages as-is; no
force-push, no history rewrite.

Goal: land a change that fixes the bug, closes the test gaps that let it (and a
related invariant-loss risk) through unnoticed, renumbers the schema to 1 consistently
across code/docs/tests, and hardens the two weakest new types — without redesigning
the type system beyond what's justified.

## 1. Renumber schema to 1

Touches every place `70de02f`/`584250d` introduced "schema 3" / "schema-3" language,
plus the README's still-stale "schema-2" references (never updated for the migration
in the first place) — both converge on "schema 1" / "schema-1".

- **`crates/cita-manifest/src/lib.rs:20`**: `pub const SCHEMA: u32 = 3;` → `1`.
- **`crates/cita-manifest/src/lib.rs:186`**: `Error::UnsupportedSchema` message
  "...this version supports schema 3 and provides no legacy migration" → "schema 1".
- **`crates/cita-manifest/src/lib.rs` tests**: `schema_round_trips_and_generated_output_is_verified`
  asserts `text.contains("schema = 3")` → `"schema = 1"`. See item 4 below for the
  `legacy_schema_is_explicitly_unsupported` test, which needs more than a number swap
  since schema 1 stops being an invalid value.
- **`AGENTS.md`** (lines 6, 48): "schema-3 `cita.toml`" / "schema-3 authority" →
  "schema-1".
- **`README.md`** (~lines 28-29, 72): "schema-2" → "schema-1" (these were never
  updated when the branch bumped to 3, so this folds the README doc-rot fix in here
  instead of a separate schema-3 pass).
- **`docs/adr/0002-bibtex-authoritative-with-curated-identifiers.md`**: this ADR was
  written and merged into history only within this same unreleased branch, so it's
  edited in place rather than preserved as an immutable historical record:
  - Line 21: "`cita.toml` is schema 3" → "schema 1".
  - Lines 42-43: "Schema 3 is a clean break: schema 1 and 2 are rejected with no
    automatic migration, consistent with the 1→2 policy." → replace with something
    reflecting the actual current policy, e.g. "This is the current schema; cita has
    no released users yet, so no compatibility is promised for any prior
    in-development format."
- **`docs/adr/0001-source-snapshots-and-generated-bibliography.md`**: its status line
  (line 5) cross-references ADR 0002 as "...the manifest is schema 3" → "schema 1".
  Line 17 ("`cita.toml` schema 2 is the sole authority") describes ADR 0001's own
  original decision and stays as-is — it's still accurate as a record of what ADR
  0001 itself decided before being superseded.

## 2. Critical bug fix: `cross_check` stores the wrong texkey

**File:** `crates/cita-inspire-client/src/client.rs:271-294`

`cross_check` validates that `bibtex_key` (the key actually embedded in the fetched
BibTeX text) is a member of `record.texkeys()` (line 276), then discards it and builds
the stored `InspireRecord` from `record.texkeys().first()` instead (lines 288-293).
Both call sites (`resolve_snapshot` line 77, `refresh_records` line 122) already hold
the correctly-matched key and pass it in — it just never makes it into the record. When
an INSPIRE record legitimately carries multiple texkeys (author-name canonicalization
is a documented cause) and the matching one isn't first, the stored `texkey` — the
value used as the suggested local citation key in `cita add`/`cita open --save`
(`crates/cita/src/main.rs`) — silently stops corresponding to the BibTeX it's paired
with.

**Fix:** replace the `texkeys().first()` block with the already-validated `bibtex_key`:

```rust
Ok(record.into_record(bibtex_key, bibtex, &bib_reference))
```

Verify during implementation whether `cita-bibliography::parse` can ever yield an
empty key; if not, the now-redundant `ok_or_else("INSPIRE record has no citation
key")` guard can be deleted along with it, otherwise keep a defensive non-empty check
on `bibtex_key`.

**Regression test** (`crates/cita-inspire-client/tests/client.rs`, alongside the
existing `rejects_a_record_whose_texkeys_match_no_bibtex_entry` test using the same
`server`/`response` helpers): a record whose JSON `texkeys` lists a non-matching key
first and the actually-matching key second, BibTeX keyed to the second. Assert the
returned `InspireRecord.texkey` equals the matching key, not the first one. This test
fails on current code and passes once the fix lands — concrete proof the bug is real
and fixed.

## 3. Test gap: curated-identifier override is never exercised

**File:** `crates/cita-manifest/src/lib.rs`, `InspireEntry::project` (lines 62-77)

Every existing manifest test builds INSPIRE entries through the `record()` helper
(lib.rs:661-670), which always sets `arxiv: None, doi: None` — so the two `if let
Some(...)` override branches are dead in the whole suite. This is the core behavior
schema 3→1 exists to deliver.

**New test** in `crates/cita-manifest/src/lib.rs`'s test module: construct an
`InspireRecord` (via a small local literal, not the `record()` helper) whose `bibtex`
embeds its own `eprint`/`doi`, and whose `arxiv`/`doi` fields hold *different* curated
values — first with both set, then a second case with only one set to confirm the
other field still falls through to the BibTeX-derived value. Add via `add_batch`,
then assert `manifest.projected()` reflects the curated values (and the untouched
field falls back to BibTeX) rather than the BibTeX-derived ones.

## 4. Test gap: `cross_check`'s error paths are untested at the client boundary

**File:** `crates/cita-inspire-client/tests/client.rs`

The old `InspireSnapshot::validate()` had dedicated unit tests for these invariants;
they moved into `cross_check`/`resolve_snapshot` (`client.rs:65-78, 271-294`) without
equivalent coverage. Add, using the existing `server()`/`response()`/`json_record()`
helpers and the pattern from `rejects_a_record_whose_texkeys_match_no_bibtex_entry`:

- `resolve_snapshot`'s single-entry check (`client.rs:70-74`): a single-record BibTeX
  response containing 0 or 2 entries → `Error::Malformed`.
- `cross_check`'s texkey-containment check reachable via `resolve_snapshot`
  (`client.rs:276-280`): BibTeX key not among the JSON record's texkeys.
- `cross_check`'s identity-mismatch check reachable end-to-end
  (`client.rs:283-287`): JSON supplies an arXiv/DOI that contradicts the BibTeX's own
  identifiers, via `resolve_snapshot` or `refresh_records`.

`SelectedRecord::identity_matches` already has a unit test one layer down
(`crates/cita-inspire-client/src/snapshot.rs`) — these new tests close the boundary
gap by driving the actual client entry points.

## 5. Fast-follow: load-time re-validation of curated identifiers against BibTeX

**File:** `crates/cita-manifest/src/lib.rs`, `validate_references` (lines 544-583)

On `main`, every load re-derived the projected reference from stored JSON and
re-checked it agreed with the stored BibTeX. On this branch, `InspireEntry::project()`
unconditionally trusts `identifiers.arxiv`/`doi` with no comparison against the
BibTeX's own identifiers ever again after fetch time. This is a disclosed trade-off in
`docs/adr/0002-bibtex-authoritative-with-curated-identifiers.md`, mitigated at
write-time by the JSON∩BibTeX intersection logic — but nothing stops a hand-edited or
future-buggy `cita.toml` from carrying a silent mismatch indefinitely.

**Add**, in `validate_references`, for each `Inspire` entry after the `record_id`
check: parse `project_bibtex(&entry.bibtex)` once, and if `identifiers.arxiv`/`doi` is
`Some`, confirm the normalized value is contained in that BibTeX projection's own
`identifiers.arxiv`/`dois`. Return `Error::InvalidSource` with a clear message
(e.g. "curated arXiv id ... is not present in stored BibTeX") otherwise. This adds one
extra BibTeX parse on the validation path (already not a hot path — validation runs at
load/mutation time, not per-reference-access) rather than restructuring `project()`.

**Test:** a manifest with a hand-constructed `InspireEntry` whose `identifiers.arxiv`
doesn't appear in its own `bibtex` → `add_batch`/`load` rejects it with
`Error::InvalidSource`.

## 6. Fast-follow: normalize `HepIdentifiers` at its one legitimate construction site

**File:** `crates/cita-manifest/src/lib.rs`, `HepIdentifiers` (lines 49-60) and
`SourceSnapshot::inspire` (lines 81-91)

`HepIdentifiers`'s doc comment promises "canonical, normalized identifiers," but
nothing enforces that — `SourceSnapshot::inspire` currently copies `record.arxiv`/
`record.doi` straight through as a struct literal. (In practice `InspireRecord.arxiv`/
`doi` already arrive normalized via `SelectedRecord::into_record`'s `curated()` logic,
so this doesn't change current behavior — it makes the guarantee structural instead of
incidental.)

**Add** `HepIdentifiers::new(arxiv: Option<String>, doi: Option<String>) -> Self` that
runs each through `normalize_arxiv`/`normalize_doi`, and have `SourceSnapshot::inspire`
call it instead of building the struct literal directly. Keep fields `pub` (they're
already exercised via TOML `Deserialize`, and full field-privacy would require
touching every test helper and the `Manifest` aggregate's trust model for marginal
benefit — noted as a deliberate scope boundary, not an oversight). A struct-literal
`InspireEntry`/`HepIdentifiers` with `record_id: 0` or unnormalized identifiers still
compiles, exactly as before; it's still caught by `validate_references` (record_id) and
the new check in item 5 (identifier/BibTeX agreement) before it can ever be persisted
through `Manifest`.

**Test:** constructing a `SourceSnapshot::inspire(InspireRecord { arxiv: Some("2401.00001v2".into()), .. })`
and confirming the stored `HepIdentifiers.arxiv` is the normalized `"2401.00001"`.

## 7. Minor cleanup

- **`record_id == 0` regression test** (`crates/cita-manifest/src/lib.rs`): add a case
  using `add_batch(vec![inspire("Local", "Key", 0)])` (record_id 0) and assert
  `Error::InvalidSource`. Currently untested on both `main` and this branch.
- **Rename the misleadingly-named test** `nested_schema_two_shape_and_unknown_fields_are_rejected`
  (lib.rs:906) → `nested_shape_and_unknown_fields_are_rejected` — it tests the current
  schema's nested/unknown-field rejection, not schema 2, and its name could lead a
  future contributor to believe schema-2 rejection is already covered there.
- **Explicit invalid-schema rejection tests**: with `SCHEMA` now 1,
  `legacy_schema_is_explicitly_unsupported` (lib.rs:895-903, currently asserting
  `schema = 1` is rejected) must change — schema 1 becomes the valid value. Rework it
  to assert rejection of `schema = 0` and `schema = 2` (both real prior values from
  this project's history), proving the numeric gate at `lib.rs:282-284` rejects any
  non-1 value before any content shape is ever parsed.
- **Delete the stale untracked `cita.toml`/`references.bib`** in the repo root — confirmed
  to still say `schema = 2`, i.e. leftover from manually testing the CLI pre-migration.
  Will double-check their contents look like disposable test output (not real work)
  immediately before removing them.

## Verification

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test -p cita-inspire-client` (new `cross_check`/texkey tests)
- `cargo test -p cita-manifest` (new override/re-validation/normalization/record_id/schema tests)
- `cargo test --workspace` (full suite, confirm no regressions from the rename, the
  schema renumbering, or the new `validate_references` check tightening rejection
  behavior)
- Confirm the new `cross_check` regression test fails without the fix and passes with
  it (proves it actually catches the bug class, not just exercises the code path)
- `grep -rn "schema.3\|schema-3" --include=*.rs --include=*.md .` (excluding
  `docs/adr/0001-...md`'s intentional historical line) comes back empty, confirming
  the renumbering swept every reference
