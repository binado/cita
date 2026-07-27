# Fix review findings on `codex/global-personal-shelf-store` (PR #38)

## Context

PR #38 replaces directory-discovered cita projects with a single user-global library
at `$CITA_HOME`. A five-agent review plus direct reproduction against the built
binary found two ways to put a store into a **permanently unrecoverable state**, a
reserved-name collision between the registry lock and a user shelf, batch failures
that vanish when stdout is redirected, and a set of type-design gaps where the new
design's invariants live in reviewers' heads rather than in types.

The architecture is sound — a fixed `shelves/<name>` layout makes path-escape
structurally impossible. These are defects in the new code, not in the direction.
All fixes land in PR #38 before merge.

Every finding below was reproduced, not inferred. The reproductions are in the
review that preceded this plan.

### Scope decisions (confirmed)

| Decision | Choice |
|---|---|
| `ShelfName` newtype + `Target<'a>` refactor | **In PR #38** — crate is 0.x/pre-release, API break is free |
| Doubled-cause error fix | **Whole workspace** — 12 variants across 3 files |
| Batch failures → stderr | **Yes** |
| Non-blocking lock notice / shared read lock | **Out of scope** (finding #4 stays open — see Deferred) |

---

## Phase A — Correctness (the two unrecoverable states + the collision)

### A1. `create_shelf` must not be wedged by an unrelated damaged shelf

`crates/cita-manifest/src/library.rs:315`, `:432-452`

`validate_unique_shelf_paths` canonicalizes **every** registered shelf and
propagates the raw io error, so one broken shelf blocks creating any other — and
`ensure_shelf_manifest` (`:312`) has already written `shelves/<new>/shelf.toml`,
leaving an unregistered orphan.

Replace it with a check scoped to the new name, run **before** anything is created
so no orphan is possible:

```rust
fn validate_new_shelf_alias(
    root: &Path, name: &ShelfName, registered: &BTreeSet<ShelfName>,
) -> Result<(), LibraryError> {
    // Not yet on disk => cannot alias anything. Only a case-insensitive FS (or a
    // pre-existing entry) makes the candidate resolvable before creation.
    let Ok(candidate) = fs::canonicalize(shelf_directory(root, name)) else {
        return Ok(());
    };
    for other in registered.iter().filter(|o| *o != name) {
        // A sibling that cannot be canonicalized is damaged, not aliasing.
        // Skipping it is what keeps one broken shelf from blocking creation.
        if fs::canonicalize(shelf_directory(root, other)).is_ok_and(|e| e == candidate) {
            return Err(LibraryError::InvalidShelf { /* aliases `{other}` */ });
        }
    }
    Ok(())
}
```

Reorder `create_shelf` to: validate name → lock → reload → alias-check → create →
persist. This extends commit 7686a47's isolation intent from the read path to the
write path.

### A2. `open_or_create` must repair a missing default shelf

`crates/cita-manifest/src/library.rs:182-195`, `crates/cita/src/commands/library.rs:32-42`

`ensure_shelf_manifest(DEFAULT_SHELF)` runs only on the registry-absent branch, so
deleting `shelves/main/` bricks the store: `cita init` prints "Already initialized"
and repairs nothing, and `shelf new main` short-circuits on the already-registered
branch.

Restructure so both branches converge on a **repair** step:

- Add `repair_default_shelf(root)` that creates the directory and an empty manifest
  **only when absent**. It must not `Manifest::load` an existing manifest —
  otherwise a corrupt `main/shelf.toml` would fail every command and destroy the
  batch-isolation property that `load`'s doc (`:205-209`) promises.
- Call it on both the load branch and the create branch of `open_or_create`.
- `init_global` reports what it repaired instead of a flat "Already initialized".
- Map the `validate_shelf_manifest` NotFound case to `InvalidShelf` with a repair
  hint rather than letting a bare `LibraryError::Read` ENOENT escape.

This makes AGENTS.md:69 ("Every data command opens or lazily creates the library
and `main`") true — today it holds only on first init.

### A3. Separate the registry-lock and shelf-lock namespaces

`crates/cita-manifest/src/library.rs:178`, `:305`, `:324`

A shelf named `library` reuses `locks/library.lock`, the registry lock that
`open_or_create` takes on **every** invocation — so `cita sync -s library` would
block every other cita command. Verified: after creating and using shelf `library`,
`locks/` contains only `library.lock`.

All five lock paths are bare literals with no shared constant. Introduce:

```rust
const LOCKS_DIR: &str = "locks";
fn registry_lock_path(root: &Path) -> PathBuf   // locks/registry.lock
fn shelf_lock_path(root: &Path, name: &ShelfName) -> PathBuf  // locks/shelf-<name>.lock
```

`shelf-*` cannot collide with `registry.lock`, so no name needs reserving. Old
`locks/library.lock` files become inert leftovers — locks are ephemeral coordination
state, not data, so no migration is required.

---

## Phase B — Diagnostics

### B1. Batch failures to stderr, with an aggregate summary

`crates/cita/src/commands/export.rs:57-64`, `crates/cita/src/commands/library.rs:108-116`

`cita export --all-shelves out/ 2>&1 >/dev/null` currently prints nothing and exits
1. Successes stay on `println!`; failure blocks move to `eprintln!`; add a final
`eprintln!` summary ("2 of 5 shelves failed") before returning the aggregate.

⚠️ **This breaks six assertion sites** in `crates/cita/tests/cli.rs` (`:311`,
`:313-318`, `:336`, `:338-343`, `:350`, `:352-357`). They call `.unwrap()` on
`str::find` against a single stream, so a moved line **panics** rather than soft-
fails, and the alpha < main < zeta ordering is asserted by byte offset within one
stream. Restructure: assert success ordering within stdout and failure ordering
within stderr, separately.

### B2. Stop printing every cause twice — workspace-wide

12 variants name their field `source` (which thiserror auto-promotes to the error
chain) **and** interpolate `{source}` in the format string, so anyhow's `{:#}`
prints it twice:

```
could not read .../shelves/main: No such file or directory (os error 2): No such file or directory (os error 2)
```

| File | Variants |
|---|---|
| `crates/cita-manifest/src/library.rs` | `CreateDirectory` :56, `Read` :80, `Lock` :118, `Write` :129, `Manifest` :137 |
| `crates/cita-manifest/src/lib.rs` | `Read` :236, `Write` :260 |
| `crates/cita-documents/src/lib.rs` | :311, :319, :350, :359, :381 |

Drop `: {source}` from each format string; keep the field. Same for
`LibraryError::Serialize` (`:125`), which duplicates via `#[from]` + `{0}`.

### B3. Error-message wording

- `crates/cita-manifest/src/lib.rs:233` — `"project already contains {0}"` uses
  vocabulary this PR deleted. → `"shelf already contains {0}"`.
- `library.rs:79` — `Read`'s doc says "file" but it is constructed for directories
  (`:380`, `:408`, `:439`). → "A managed path could not be read."
- `library.rs:164` — `RelativeRoot` says "cita home must be absolute" even when the
  offender is a relative `$HOME`. Distinguish the two.
- Carve `ShelfMissing { name }` out of `Read` so "registered but directory gone" —
  a state the design explicitly supports — is distinguishable from an I/O error.

---

## Phase C — Type design

### C1. `ShelfName` newtype

Shelf names travel as bare `String` from clap into `Path::join`. The old `Shelf`
newtype was removed with nothing replacing it; `ensure_shelf_manifest:360`
defensively re-validates because the type cannot carry the guarantee.

Add to `library.rs`: `pub struct ShelfName(String)` with validating
`TryFrom<&str>`/`FromStr`, plus `Deref<Target = str>`, `AsRef<str>`, `Display`,
`Ord`/`Eq`/`Hash`. Absorb `validate_shelf_name`'s rule — the leading-alphanumeric
requirement is what makes `shelves/<name>` traversal-safe, and that belongs in the
type's doc.

Ripple: `Library::shelves() -> &BTreeSet<ShelfName>`, `shelf_manifest(&ShelfName)`,
`create_shelf(&ShelfName)`, `lock_shelf(&ShelfName)`; export `ShelfName` and remove
the now-redundant `validate_shelf_name` from `lib.rs:6-9`. Conversion happens once,
at the clap boundary in `resolve_target`/`new_shelf`. Deletes the defensive
re-validation at `:266`, `:304`, `:360`.

### C2. `ShelfLock` carries what it locks

`library.rs:37-41` — the lock names no shelf, so "lock A, mutate B" and "mutate with
no lock at all" are both representable. Every call site is already `lock(); load();`
back to back, so making the lock produce the manifest costs nothing:

```rust
pub struct ShelfLock { name: ShelfName, manifest_path: PathBuf, _file: File }
impl ShelfLock {
    pub fn name(&self) -> &ShelfName;
    pub fn manifest(&self) -> Result<Manifest, Error>;
}
```

Call sites become `let lock = target.lock()?; let mut manifest = lock.manifest()?;`
in `add.rs:37`, `import.rs:34`, `remove.rs:5`, `sync.rs:64`. `fetch.rs:23` locks
conditionally, so it needs
`match &lock { Some(l) => l.manifest()?, None => target.load()? }`.

Also add `#[must_use = "the shelf lock is released when dropped; bind it for the
whole critical section"]` to `ShelfLock`, `lock_shelf`, and `Target::lock` —
today a future `target.lock()?;` with no binding drops the lock immediately with
**no compiler warning**, because `?` already consumed the `Result`.

Give the registry lock the same treatment (a `LibraryLock` type, or reuse
`ShelfLock`'s shape) so one concept stops having two representations.

Document that `fs2` uses `flock`, which is per-file-description and therefore
**not reentrant**: nesting two acquisitions in one process deadlocks. No current
path nests, but `ShelfLock` is `Send` and is held across `.await` in `sync_outcome`
and `fetch::select`, so a future concurrent batch loop would hang rather than error.

### C3. `Target` borrows instead of cloning

`crates/cita/src/commands/library.rs:8-30`

- Drop `#[derive(Clone)]` — the only `.clone()` in the module is `library.clone()`
  *inside* `target_in`; the derive is dead capability that lets a caller mutate
  `manifest_path` out of correspondence with `name`.
- Make `name`/`manifest_path` accessors. Cheap: `name` is read in exactly one place
  outside the module (`export.rs:26`) and `manifest_path` in **zero**.
- Change to `Target<'a> { library: &'a Library, .. }`. Both batch call sites already
  hold the `Library` alive for the whole loop. `resolve_target` currently creates and
  returns the `Library` inline, so `main.rs` gains
  `let library = commands::open_library()?;` before each `target_in(&library, ..)` —
  mechanical across ~8 match arms, and it collapses the three independent
  `open_or_create` entry points (`library.rs:45`, `:100`, `export.rs:38`) into one.
- Add a doc comment stating the three-field correspondence.

### C4. Smaller type/logic cleanups

| Item | Location | Change |
|---|---|---|
| `Manifest::create` overload | `lib.rs:312-321` | Drop the `is_dir()` branch; always take the manifest file path. 1 production caller (`library.rs:396`) already passes a file path; 22 test call sites need `dir.path().join(MANIFEST_FILE)` |
| `resolved()` swallows error kinds | `export.rs:98` | Fall back only on `ErrorKind::NotFound`; propagate `PermissionDenied`/`ELOOP`/`ENOTDIR` with context |
| Dead disjunct | `export.rs:86` | `Path::starts_with` is already true for equal paths — drop `target == store \|\|` |
| `LibraryError` overlap | `library.rs:406-430` | `validate_shelf_manifest` reimplements `validate_managed_directory` + `validate_managed_file` line-for-line, differing only in which error it builds — which is why the test at `:648` must accept a *set* of variants. Delegate to the shared validators |
| Enum evolution | `library.rs:43` | Add `#[non_exhaustive]` to `LibraryError` |
| Shadowing | `fetch.rs:113` | `let target = ...` shadows the `target: &Target` parameter with a `FetchTarget`; rename |
| Column alignment | `commands/library.rs:89-92` | `shelf list` concatenates `"  yes"` with no padding, so columns misalign under the `Shelf  Default` header |

---

## Phase D — Tests

`crates/cita/tests/cli.rs` went 70 → 17 tests. Most deletions were legitimate
(git hooks, `generate`, ancestor discovery); these are real regressions plus
coverage for everything fixed above.

**Infrastructure prerequisite:** add `fs2` to a new `[dev-dependencies]` in
`crates/cita/Cargo.toml` — `cita` does not currently depend on it, so `cli.rs`
cannot take a lock directly. `tempfile` is already a normal dep of both crates.

### Regression tests for this plan's fixes

| Test | Asserts |
|---|---|
| `init_repairs_a_deleted_default_shelf` | delete `shelves/main`, `cita init` reports the repair, `cita list` then succeeds (A2) |
| `damaged_shelf_does_not_block_creating_another` | damage `bar`, `shelf new foo` succeeds, `shelf list` shows `foo`, no orphan (A1) |
| `a_shelf_named_library_gets_its_own_lock` | create shelf `library`, assert `locks/shelf-library.lock` and `locks/registry.lock` both exist (A3) |
| `batch_failures_are_reported_on_stderr` | `export --all-shelves` with one corrupt shelf: successes on stdout, failure block + summary on stderr, exit 1 (B1) |
| `errors_report_their_cause_once` | assert the ENOENT text appears exactly once (B2) |

### Coverage gaps to backfill

| Test | Why |
|---|---|
| `cita_home_defaults_to_home_dot_cita` | **Highest blast radius in the PR.** `library.rs:147-166` decides where every real user's library lives and no test exercises it — every test sets `CITA_HOME`, and `env_remove` appears nowhere in the workspace. Needs `.env_remove("CITA_HOME")` + `.env("HOME", tempdir)`; gate `#[cfg(unix)]`. Must stay hermetic — it must never touch the real `$HOME` |
| `shelf_locks_are_per_shelf_and_exclusive` | Replaces `concurrent_mutations_do_not_lose_updates` (`cli.rs:404`), which cannot fail reliably — nothing forces the two processes to overlap. Take the flock directly, spawn `cita import`, assert `try_wait()` is `None` while held and `Ok` after release; assert a *different* shelf's lock is unaffected. Without this, collapsing per-shelf locks into one global lock would pass every existing test |
| `symlinked_shelf_manifest_is_rejected` | `shelf.toml` and `library.toml` are the files atomic writes clobber; only the shelf *directory* symlink case is covered today |
| `remove_resolves_doi_and_arxiv_selectors` + `remove_is_atomic` | `Manifest::find`/`remove_batch` have **zero** unit tests; their only coverage was deleted. Atomicity matters: a typo'd selector must leave the manifest byte-identical |
| `export_allows_a_sibling_prefixed_destination` | Pins that `~/.citaX` is not falsely rejected — i.e. that the guard stays component-wise `Path::starts_with` |
| Fix `case_aliases_are_rejected...` (`library.rs:664`) | Self-defeating: asserts *success* on case-sensitive filesystems, so the rejection branch is dead on Linux CI. Rewrite as a symlink-based alias test that runs everywhere |
| `registry_missing_main_is_rejected`, `unsupported_schema_is_rejected` | `library.rs:259-264` and `:236-238`; the latter carries the user-facing "no legacy migration" message |

---

## Phase E — Docs, comments, changelogs

### E1. Restore rationale deleted from surviving code

| Location | What |
|---|---|
| `export.rs:95-107` | **Highest-value loss.** The `resolved()` comment explained why an existing target canonicalizes outright (case-insensitive aliasing) vs. parent-only for a non-existent one. The body is byte-identical; a maintainer "simplifying" it silently defeats `export_refuses_a_symlink_alias_into_the_store` |
| `export.rs:70-80` | `derive_entry`: why the *generic* projection reaches an INSPIRE snapshot's *curated* arXiv ID — i.e. why there is no `match source` here |
| `commands/library.rs:96` | `run_batch`: batches are independent mutations, **not** a transaction — nothing is rolled back |
| `commands/library.rs:58` | `explain_lookup_failure`: why the hint names the creating verb (selection never registers) |
| `main.rs:210` | Why `batch_failed` exists alongside `Result`, and why `ReportedFailure` prints nothing extra |
| `main.rs:57`, `:64` | `ShelfArg`/`ScopeArgs`: why `--shelf` is per-command rather than global, and which commands may take `--all-shelves` |
| `mod.rs:46` | `highlight_style`: the `NO_COLOR` behavior and the `list::print_rows` cross-reference |

### E2. New/corrected API docs

`create_shelf`'s `bool` return is undocumented (`:302`); `shelf_manifest` reads as
pure path computation but does I/O and can fail three ways (`:294`); `lock_shelf`
does not warn that it **blocks indefinitely** (`:321`); `ShelfLock` does not say
"advisory" excludes only cooperating cita processes (`:37`); `open_or_create`
implies full repair (`:170`); `render_derived`'s closing sentence
(`lib.rs:623`) references a managed bibliography that no longer exists.

`docs/CONTEXT.md` deleted its **Reference** and **Provider identity** glossary
entries while the new text still uses both terms as if defined — restore them.

Update `README.md:80` (the `locks/` tree) and AGENTS.md:75-78 for the new lock
filenames and the batch stderr routing.

### E3. Changelogs — hand-edit `[Unreleased]`

release-plz generates released sections from Conventional Commits but the
`## [Unreleased]` bodies are hand-written (commit 59be66a did exactly this), so
editing them is the established convention.

**`crates/cita/CHANGELOG.md`** — add a `### Changed` section (currently absent):
`-o/--output` became positional, and `cita init --path` was removed. Today the file
covers four of the five removals that `main.rs:307-319` pins and misses `--output`,
so a `cita export -o refs.bib` in a Makefile breaks with nothing pointing at it.

**`crates/cita-manifest/CHANGELOG.md`** — the current line omits the removal of
`Shelf`, `Manifest::{import_existing, load_verified, verify_bibliography, generate,
bibliography_path}`, `BIBLIOGRAPHY_FILE`, and `Error::BibliographyDrift` — and, most
dangerously, that **two exported constants changed value**:

- `MANIFEST_FILE`: `cita.toml` → `shelf.toml`
- `LIBRARY_FILE`: `cita-library.toml` → `library.toml`

A downstream user of the published crate keeps compiling and silently reads the
wrong filename. Add `### Changed` and `### Fixed` sections covering these and the
Phase A/B fixes.

### E4. Mark the change breaking

The repo **squash-merges** — 79e328c landed as a single commit with a `(#36)`
suffix, and release-plz read its `feat!` to emit the `[**breaking**]` marker at
`crates/cita/CHANGELOG.md:26`. So the PR **title** becomes the commit subject that
release-plz parses.

**Retitle PR #38 to `feat!: add global personal shelf store`.** No history rewrite,
no force-push. (If you merge-commit instead of squashing, the local commits need
amending to `feat!:` — confirm the merge strategy first.)

---

## Verification

```bash
cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
```

CI additionally runs `cargo doc --workspace --no-deps` with `RUSTDOCFLAGS: -D
warnings` on both 1.88 and stable, so every new public item needs a doc comment
(`cita-manifest` sets `#![warn(missing_docs)]`). Confirm MSRV separately:

```bash
cargo +1.88.0 check --workspace --all-targets
```

Then re-run the manual reproductions that found these bugs, against a throwaway
store — each must now behave correctly:

```bash
export CITA_HOME=$(mktemp -d)/store
cargo run -q -p cita -- init
rm -rf "$CITA_HOME/shelves/main"
cargo run -q -p cita -- init          # must repair, not claim success
cargo run -q -p cita -- list          # must succeed
```

```bash
export CITA_HOME=$(mktemp -d)/store
cargo run -q -p cita -- init
cargo run -q -p cita -- shelf new bar && rm -rf "$CITA_HOME/shelves/bar"
cargo run -q -p cita -- shelf new foo   # must succeed despite damaged bar
cargo run -q -p cita -- shelf list      # must list foo
```

```bash
# batch failures must be visible on stderr and absent from stdout
cargo run -q -p cita -- export --all-shelves out/ 2>/dev/null   # no failure text
cargo run -q -p cita -- export --all-shelves out/ 2>&1 >/dev/null # failure text + summary
```

Also confirm error text appears exactly once (no `os error 2): No such file` doubling),
and that `locks/` contains `registry.lock` plus `shelf-<name>.lock` entries.

Finally, `cargo test --test e2e -- --ignored` (live INSPIRE, network required)
before merge, since Phase C touches the `Target`/lock plumbing every command uses.

---

## Deferred (explicitly out of scope)

**Finding #4 — blocking locks with no diagnostic.** `fs2::lock_exclusive` has no
timeout and no message, and `open_or_create` takes it *exclusively* even for
read-only commands. Measured: `cita list` blocked **3.3s** in silence while another
process held the registry lock. Per your selection this stays open; the fix would be
`try_lock_exclusive` first + a "waiting for lock" notice on stderr, and a shared
lock for the read path. Worth a follow-up issue.

**`fs2` is unmaintained** (0.4.3, last released 2020). `fs4` is the maintained fork.
Rust stabilized `File::lock_exclusive` in std at **1.89** — one minor above this
workspace's 1.88 MSRV — so bumping MSRV would drop the dependency entirely.

**Untracked repo-root leftovers** — `cita.toml` and `references.bib` are stale
artifacts of the pre-PR workflow. Harmless (untracked) but confusing in a PR whose
premise is that legacy `cita.toml` is ignored. Delete or gitignore.
