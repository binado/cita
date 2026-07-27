# E2E test against the live INSPIRE API

## Context

The current test suite is comprehensive but entirely **hermetic**: every INSPIRE
interaction in `crates/cita/tests/cli.rs` and `crates/cita-inspire-client/tests/client.rs`
runs against a canned local `TcpListener`, injected via the `CITA_INSPIRE_BASE_URL`
env var. Nothing ever exercises the real `https://inspirehep.net/` API, so a
breaking change in how INSPIRE shapes its JSON/BibTeX, or a regression in our
JSON↔BibTeX cross-check, projection, or identifier curation, would pass CI silently.

The goal is a single **end-to-end test that makes real network calls** to INSPIRE,
driving a realistic progression of `cita` commands against a handful of handpicked
papers (`E2E_TEST.md`) and asserting on the resulting `cita.toml` / `references.bib`
after each step. Because it depends on the network it must not run in the default
suite; it is gated with `#[ignore]` and executed on demand or in a dedicated,
opt-in CI job.

No mocking: the test simply omits `CITA_INSPIRE_BASE_URL`, so `inspire_client()`
(`crates/cita/src/commands/mod.rs:83`) falls back to `DEFAULT_BASE_URL`
(`crates/cita-inspire-client/src/client.rs:13`) and hits the real API.

## Decisions (confirmed with user)

- **Mechanism:** new `crates/cita/tests/e2e.rs`, `#[ignore]`d, reusing the same
  invocation pattern as `cli.rs`. Run with `cargo test --test e2e -- --ignored`.
- **Assertions:** structural/stable only (no golden snapshots) — resilient to
  upstream BibTeX re-rendering.
- **CI:** a **separate opt-in job** (manual `workflow_dispatch` + optional nightly
  `schedule`), not part of the default matrix.
- **Start state:** seed a small committed `.bib` fixture via `import`, then
  progressively `add` the INSPIRE papers.

## Key facts established during exploration

- Papers in `E2E_TEST.md` are given as `inspirehep.net/literature/<id>` URLs.
  `cita add` does **not** accept URLs — locators are `inspire:<id>`, bare arXiv,
  `arxiv:`, or `doi:` only (`crates/cita-core/src/locator.rs:33`). So the URLs map to:
  - `inspire:451647` — legacy arXiv id, has v2, math in title → **arXiv present**
  - `inspire:33714` — no arXiv number → **arXiv absent** (DOI/inspire only)
  - `inspire:1421100` — many authors → arXiv present
  - `inspire:51188` — no arXiv number → **arXiv absent**
- `cita.toml` is schema-1 (`crates/cita-manifest/src/lib.rs`): top-level `schema = 1`
  plus `[references."<KEY>"]` tables. INSPIRE entries carry `source = "inspire"`,
  `record_id`, `updated` (raw RFC3339 — **do not** hard-assert its value), `bibtex`,
  and an optional `[references."<KEY>".identifiers]` sub-table with optional
  normalized `arxiv` / `doi` (the block/field is **omitted** when absent).
- `references.bib` is regenerated deterministically (sorted by local key, entries
  joined by one blank line, trailing newline). `load_verified` byte-compares it and
  errors with "run `cita generate`" on drift — the ideal no-drift assertion.
- `add` issues two GETs (`format=json`, then `format=bibtex`) and cross-checks them;
  `sync` issues a search then a batched BibTeX. ~10 requests total for this scenario,
  well within the client's built-in 429 retry handling.
- Existing tests invoke the binary via `env!("CARGO_BIN_EXE_cita")` with
  `.current_dir(tempdir)` and `NO_COLOR=1`. There are **no** existing `#[ignore]`
  tests and **no** `tests/fixtures/` dir yet.

## Implementation

### 1. Seed fixture — `crates/cita/tests/fixtures/e2e_seed.bib` (new)

Two minimal `import`-shaped entries that must survive untouched through the whole
run (used to prove import/add coexistence and that `sync` leaves imports alone):

```bibtex
@misc{SeedAlpha,
  title = {A seeded import entry},
  author = {Doe, Jane},
  year = {2020}
}

@misc{SeedBeta,
  title = {Another seeded import entry},
  doi = {10.1000/e2e.seed}
}
```

### 2. Test file — `crates/cita/tests/e2e.rs` (new)

Each Cargo integration test is its own crate, so it cannot import `cli.rs`'s
helpers directly. Copy the **minimal** subset needed (keeps `cli.rs` untouched):
`cita(cwd, args)` (via `env!("CARGO_BIN_EXE_cita")`, `NO_COLOR=1`, no base-url env),
`cita_stdin`, `success`, `success_streams`, `failure`, and `init_git`/`git_success`
for the commit step. Add a small `read(dir, name)` helper for `cita.toml` /
`references.bib`.

One `#[test] #[ignore = "live INSPIRE network"]` function running the full
progression and asserting after each step (the user explicitly wants the manifest
checked after each command). Structure:

1. `tempdir()` + `cita init` → assert `cita.toml` starts with `schema = 1`,
   `references.bib` is empty.
2. `import fixtures/e2e_seed.bib` → assert manifest contains `SeedAlpha`,
   `SeedBeta`, `source = "import"`; capture the two imported entries for a later
   "untouched" comparison.
3. For each paper, `add inspire:<id>` then re-read `cita.toml` and assert:
   - `record_id = <id>` and `source = "inspire"` present;
   - a stable title substring appears in the stored `bibtex`;
   - **identifier shape:** `inspire:451647` and `inspire:1421100` have an
     `identifiers` block with an `arxiv = "..."` value that carries **no** `v<n>`
     suffix (versionless); `inspire:33714` and `inspire:51188` have **no**
     `arxiv = ` line (assert absence).
4. **Idempotency:** re-run `add inspire:451647` → stdout is `Already present: <key>`;
   assert `cita.toml` bytes are unchanged from before the re-add.
5. **No drift:** run `cita generate`; assert `references.bib` is byte-identical to
   before, and `cita list` succeeds and prints an expected title substring. (This
   is the load-time `verify_bibliography` guarantee exercised end to end.)
6. **Sync:** run `cita sync` (real network) → assert stdout reports the managed
   count refreshed and imports left unchanged, and that the two seed import entries
   are still byte-identical in `cita.toml`.
7. **Remove + git commit:** `init_git` at the start (or here), `cita remove
   inspire:51188` → assert it disappears from both files; then `cita commit` →
   assert `git show --name-only HEAD` lists exactly `cita.toml` and `references.bib`.

Keep assertions on **substrings and structural facts**, never exact byte snapshots
of upstream content. Guard against transient flakiness by relying on the client's
existing 429 retry; do not add sleeps unless a rate-limit problem is observed.

### 3. CI — `.github/workflows/ci.yml` (edit)

Add a second, independent job (the existing `test` matrix is unchanged and stays
offline — `cargo test --workspace` skips `#[ignore]` tests by default):

```yaml
  e2e:
    if: github.event_name == 'workflow_dispatch' || github.event_name == 'schedule'
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo test --test e2e -- --ignored
```

Add the matching triggers at the top:

```yaml
on:
  push:
    branches: [main]
  pull_request:
  workflow_dispatch:
  schedule:
    - cron: "0 6 * * 1"   # optional weekly liveness check
```

### 4. Docs — `AGENTS.md` / `CLAUDE.md` (edit, small)

Add one line under **Commands** noting the opt-in live test:
`cargo test --test e2e -- --ignored  # live INSPIRE, network required`, and a
sentence clarifying that the default suite remains hermetic.

## Reuse / references

- Invocation pattern & helpers to mirror: `crates/cita/tests/cli.rs:10-67`
  (`cita`, `cita_stdin`, `success`, `success_streams`, `failure`) and `:111-128`
  (`git`, `init_git`).
- Real-network switch is purely the absence of `CITA_INSPIRE_BASE_URL`
  (`crates/cita/src/commands/mod.rs:83-94`).
- Manifest fields to assert on: `crates/cita-manifest/src/lib.rs:36-53`
  (`InspireEntry`, `HepIdentifiers`).
- Locator forms: `crates/cita-core/src/locator.rs:33-58`.

## Verification

1. Offline default suite is unaffected:
   `cargo test --workspace` → the new e2e test shows as **ignored**, everything
   else passes. Also `cargo fmt --all -- --check` and
   `cargo clippy --workspace --all-targets -- -D warnings` (clippy compiles the new
   test target).
2. Live run (needs network):
   `cargo test --test e2e -- --ignored --nocapture` → passes, and `--nocapture`
   lets you eyeball the real titles/identifiers the first time so you can tighten
   the substring assertions to the actual upstream values.
3. Sanity-check the identifier expectations against the real API before finalizing
   assertions, e.g. `cargo run -p cita -- add inspire:33714` in a scratch dir and
   confirm no `arxiv =` line is written for the no-arXiv papers.
4. CI: trigger the `e2e` job via **Run workflow** (workflow_dispatch) on a branch
   and confirm it runs `--ignored` green; confirm a normal PR does **not** run it.
