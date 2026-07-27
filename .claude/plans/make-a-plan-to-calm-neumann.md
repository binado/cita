# Apply PR #32 review findings and further polish

## Context

PR #32 ("feat: add library and shelf support") was reviewed with a 5-agent
pass (general review, test coverage, type design, silent-failure, docs) and
the findings were posted to the PR
(https://github.com/binado/cita/pull/32#issuecomment-5062793645). The user
asked to apply all recommended actions plus the additional polish
suggestions from that review. No new bugs were found in this second pass —
this plan is entirely about closing the two "Important" gaps and cleaning up
the nine "Suggestions" items, without changing any documented behavior in
`AGENTS.md`/`CLAUDE.md`.

All work is confined to `crates/cita/src/{main.rs,commands/library.rs,commands/sync.rs}`,
`crates/cita-manifest/src/library.rs`, `crates/cita/tests/cli.rs`,
`docs/CONTEXT.md`, and `crates/cita-manifest/README.md`.

## A. Important fixes

### A1. Fix batch error formatting (`crates/cita/src/commands/library.rs:167-169`)

`one_line()` currently does
`format!("{error:#}").replace(['\n', '\r'], " ")`, which collapses a
multi-line diagnostic (e.g. a `toml` parse error with its `|`/`^^^^^^^`
caret gutter) into an unreadable run-on line during `cita library
generate`/`sync`. Replace it with a helper that prints a one-line header
plus the full diagnostic indented on following lines:

```rust
fn print_batch_failure(name: &str, error: &anyhow::Error) {
    println!("Shelf {name}: failed:");
    for line in format!("{error:#}").lines() {
        println!("  {line}");
    }
}
```

Replace both call sites (`batch_generate` line ~132, `batch_sync` line
~152) — currently `println!("Shelf {name}: failed: {}", one_line(&error));`
— with `print_batch_failure(name, &error);`, and delete `one_line`. The new
output still starts with `"Shelf {name}: failed:"`, so the existing
assertion in `library_batches_are_ordered_continue_after_failure_and_report_once`
(`crates/cita/tests/cli.rs:554`, `stdout.contains("Shelf alpha: failed:")`)
keeps passing unchanged.

### A2. Test `ensure_direct_shelf` on the single-shelf routed path

`ensure_direct_shelf` (`crates/cita/src/commands/library.rs:171-180`) is the
guard that stops `cita library shelf <name> <cmd>` from silently climbing
past a broken shelf into an unrelated ancestor `cita.toml` via
`find_manifest`'s ancestor walk. It's currently only exercised through
`batch_generate`/`batch_sync`, and only for a *missing directory*
(`crates/cita/tests/cli.rs:548-558`, via `fs::rename`), never a *missing
`cita.toml`* reached through `run_shelf_command`.

Add a new test in `crates/cita/tests/cli.rs`, following the existing
register-then-tamper pattern used by
`failed_library_registration_leaves_a_valid_shelf_for_retry` (lines
577-613) and the shelf-init pattern from
`library_and_shelf_initialization_cover_new_and_existing_projects` (lines
285-342):

1. `cita library init`, then `cita library shelf orphan init`.
2. `fs::remove_file(directory.path().join("orphan/cita.toml"))`.
3. Run `cita library shelf orphan list`, assert failure via the existing
   `failure()` helper (returns stderr).
4. Assert the stderr contains `"does not contain cita.toml"`.

## B. Suggestions / polish

### B1. De-duplicate `validate_registration`/`register` (`crates/cita-manifest/src/library.rs:249-301`)

Both methods independently clone `self.shelves`, check
"already registered at a different path", and insert the candidate
`Shelf`. Factor the shared part into a private helper:

```rust
fn candidate_shelves(
    &self,
    name: &str,
    path: impl AsRef<Path>,
) -> Result<Option<BTreeMap<String, Shelf>>, LibraryError> {
    if let Some(existing) = self.shelves.get(name) {
        if existing.path == path.as_ref() {
            return Ok(None);
        }
        return Err(LibraryError::AlreadyRegistered {
            name: name.into(),
            path: existing.path.clone(),
        });
    }
    let mut candidate = self.shelves.clone();
    candidate.insert(name.into(), Shelf { path: path.as_ref().to_path_buf() });
    Ok(Some(candidate))
}
```

`validate_registration` keeps its existing `validate_name(name)?` call,
then matches on `candidate_shelves` (`Some` → `validate_shelf_set(..,
false)`, `None` → `Ok(())`). `register` matches on `candidate_shelves`
(`Some` → validate with `require_exists: true`, persist, swap in
`self.shelves`; `None` → `Ok(())`). This preserves both methods' current
external behavior exactly (including that `register` never called
`validate_name` directly, relying on `validate_shelf_set`'s per-shelf
check — that asymmetry is unchanged).

### B2. Compute `Library.root` instead of storing it (`crates/cita-manifest/src/library.rs:35`)

Confirmed both constructors keep `root` in sync with `path.parent()` by
hand: `create()` sets `root: root.to_path_buf()` where `path =
root.join(LIBRARY_FILE)` (line 135), and `load()` computes `root` from
`path.parent()` (lines 190-193). Remove the stored `root: PathBuf` field
and compute it on access:

```rust
pub fn root(&self) -> &Path {
    self.path.parent().unwrap_or_else(|| Path::new("."))
}
```

Update the `Self { .. }` literals in `create()` and `load()` to drop the
`root` field (in `load()`, keep the local `let root = ...` binding used for
the early `reject_root_shelf(&root)?` call — only the struct field goes
away). Update internal call sites that currently read `self.root` directly
(`shelf_directory`, `validate_shelves`, the new `candidate_shelves` /
`validate_registration` / `register` from B1) to call `self.root()`
instead.

### B3. Preserve the specific `LibraryError` when `Library::load` fails validation (`crates/cita-manifest/src/library.rs:212-217`)

Today, `library.validate_shelves(false)` errors (which can be
`InvalidName`, `InvalidPath`, etc.) are stringified into a generic
`LibraryError::Invalid { path, message: error.to_string() }`, while the
same violation reached through `register`/`validate_registration` keeps
its specific variant. Add a new variant that boxes the real cause instead
of flattening it:

```rust
/// A loaded registry's shelves failed validation.
#[error("invalid library {path}: {source}")]
InvalidShelf {
    /// Invalid registry path.
    path: PathBuf,
    /// The specific validation failure.
    #[source]
    source: Box<LibraryError>,
},
```

Change only the `validate_shelves` call site in `load()` to map into
`InvalidShelf` (boxing the inner error) instead of `Invalid`. Leave every
other `LibraryError::Invalid { .. }` construction (TOML parse/schema
errors, which have no structured cause to preserve) untouched — this keeps
the existing test `rejects_names_escapes_duplicates_nesting_and_unknown_data`
(`crates/cita-manifest/src/library.rs:541-544`, which only exercises the
TOML-parse-error path) passing unchanged, since it's additive, not a
rename.

### B4. Collapse `SyncOutcome`'s duplicated wording (`crates/cita/src/commands/sync.rs:12-51`)

`Display` and `batch_message()` hand-maintain two independently-worded
format strings per state. The exact output text is asserted verbatim by
several tests (`crates/cita/tests/cli.rs:403,447,525-527,539-541,1013,1096`),
so wording must **not** change — only the duplication. Extract two private
helpers parameterized by the differing lead words:

```rust
impl SyncOutcome {
    fn synced_message(self, lead: &str, include_left: bool) -> String {
        if include_left {
            format!("{lead} {} managed references; left {} imported unchanged", self.managed, self.imported)
        } else {
            format!("{lead} {} managed references; {} imported unchanged", self.managed, self.imported)
        }
    }

    fn already_message(self, lead: &str) -> String {
        format!("{lead}: {} managed, {} imported", self.managed, self.imported)
    }

    pub(crate) fn batch_message(self) -> String {
        if self.changed { self.synced_message("synced", false) } else { self.already_message("already in sync") }
    }
}

impl fmt::Display for SyncOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", if self.changed { self.synced_message("Synced", true) } else { self.already_message("Already in sync") })
    }
}
```

Byte-identical output to today; only the format-string duplication is
removed.

### B5. Share the `FetchArgs` → `FetchOptions` conversion

The same 6-field mapping is duplicated verbatim in `main.rs:220-242` and
`commands/library.rs:94-116` (the `selector` field is threaded separately
in both places). Add one method on `FetchArgs` in `main.rs`:

```rust
impl FetchArgs {
    fn into_options(self) -> (String, commands::FetchOptions) {
        (
            self.selector,
            commands::FetchOptions {
                force: self.force,
                cache_only: self.cache_only,
                return_url: self.url,
                source: self.source,
                open: self.open,
                save: self.save,
            },
        )
    }
}
```

Update both call sites to destructure via `let (selector, options) =
args.into_options();` then call `commands::fetch`/`super::fetch` with
those two values, dropping the manual field-by-field struct literals.

### B6. Make the `ShelfCommand::Init` illegal state unrepresentable in `run_shelf_command`

`run_shelf_command` (`crates/cita/src/commands/library.rs:72-119`) has to
carry `ShelfCommand::Init { .. } => unreachable!(...)` because `main.rs`
already special-cases `Init` before routing (`main.rs:255-263`), but the
type system doesn't know that. Introduce a second, non-clap enum with only
the routable variants:

```rust
pub(crate) enum ShelfAction {
    Import(ImportArgs),
    Add(AddArgs),
    Sync,
    Remove(RemoveArgs),
    List(ListArgs),
    Generate,
    Fetch(FetchArgs),
    Commit,
}
```

Change `run_shelf_command`'s signature to take `action: ShelfAction`
instead of `command: ShelfCommand`, and drop the `unreachable!()` arm along
with the `ShelfCommand` import it no longer needs. In `main.rs`, expand the
current wildcard arm (`ShelfCommand::Init { path } => {...}, command =>
{...}`) into an explicit match that constructs `ShelfAction` per variant,
e.g.:

```rust
ShelfCommand::Init { path } => {
    commands::init_shelf(&cwd, &name, path.as_deref())?;
    Ok(false)
}
ShelfCommand::Import(args) => { commands::run_shelf_command(&cwd, &name, ShelfAction::Import(args)).await?; Ok(false) }
ShelfCommand::Add(args) => { commands::run_shelf_command(&cwd, &name, ShelfAction::Add(args)).await?; Ok(false) }
// ... Sync, Remove, List, Generate, Fetch, Commit similarly
```

This is the largest of the polish items — it's pure internal refactor (no
CLI-facing or test-facing behavior change, since `ShelfCommand` still
drives clap parsing unchanged), so all existing shelf-routing tests in
`crates/cita/tests/cli.rs` continue to exercise the same code paths.

### B7. Document the intentional `is_file()` symlink-follow in `ensure_direct_shelf`

Low-likelihood, manual-tampering-only concern: if a registered shelf's
`cita.toml` were replaced with a symlink after registration, `is_file()`
would follow it with no re-validation. No functional change — add a
one-line comment above the check noting this is accepted (the manifest
crate doesn't reject symlinked `cita.toml` elsewhere either, so this would
be an inconsistent place to start).

### B8. Doc fixes

- `docs/CONTEXT.md:11` — "Shelf names remain stable even if path spelling
  is a separate concern" overstates a rename capability that doesn't
  exist: `register`/`validate_registration` reject re-registering an
  existing name under a different path (`AlreadyRegistered`). Reword to
  state that plainly, e.g.: "Shelf names are stable identifiers; the
  registered path is fixed at registration time, and registering an
  existing name under a different path is rejected."
- `main.rs:161-162` — `ShelfCommand::Commit`'s doc comment (`/// Commit
  this shelf's managed files`) is missing the staged-file caveat that
  `Command::Commit`'s carries (`main.rs:33-34`, "refuses to run if either
  managed file is already staged") for the identical underlying behavior
  (`crate::git::commit`). Add the same caveat.
- `crates/cita-manifest/src/library.rs:95-102` —
  `LibraryError::InvalidPath`'s doc comment doesn't mention the
  "escapes the library root" case that `validate_shelf_set` actually
  checks (line 370-375). Extend the doc comment to mention it.
- `crates/cita-manifest/README.md:6` — "safe relative shelf paths, symlink
  aliases, and non-overlapping registrations" reads ambiguously, as if
  symlink aliases were a supported feature rather than a rejected case.
  Reword to e.g. "safe relative shelf paths (rejecting escapes, overlaps,
  and symlink aliases), and non-overlapping registrations."

### Explicitly not changing

- `RunOutcome`'s bool→enum conversion (`main.rs:192-269`): the review
  concluded this is "fine as a low-cost compromise" — `batch_generate`/
  `batch_sync` return `bool` internally and get converted once at the end
  of `run()`. Not touched.

## Verification

1. `cargo fmt --all -- --check`
2. `cargo clippy --workspace --all-targets -- -D warnings`
3. `cargo test --workspace` (covers `cita-manifest`'s unit tests for B1-B3
   and `cita`'s CLI suite for A1, A2, B4-B6)
4. `cargo test --test cli` specifically, to confirm the new A2 test passes
   and the exact-wording assertions touched by B4 (`cli.rs:403,447,525-527,
   539-541,1013,1096`) and the batch-failure-header assertion touched by A1
   (`cli.rs:554`) still pass unchanged.
5. `cargo build --workspace` as a final sanity check (the plan doesn't run
   the network-gated `--ignored` e2e test, which has no library/shelf
   coverage).
