# Fix the reviewer's findings on `feat/bibtex-export`

## Context

A reviewer raised two critical and five important findings against the derived
BibTeX export branch. I verified each against the code; the review is
substantially correct, with two details that need correcting (recorded below so
the fixes are grounded in what actually happens, not in the review's wording).

The load-bearing problem is real and destructive: on a case-insensitive
filesystem (macOS APFS, this machine) `cita export -o References.bib` writes
over the managed `references.bib`, and `-o Cita.toml` writes over `cita.toml`.
The bibliography is recoverable with `cita generate`; **the manifest is not**.
That directly violates the invariant AGENTS.md states for exports ("never
touches `references.bib`", "refuse to overwrite a managed file").

Verified while investigating:

- `resolved()` canonicalizes only the *parent* and joins the literal file name,
  then compares by string equality — so `References.bib` and `references.bib`
  compare unequal while naming the same inode.
- macOS `realpath(3)` **does** fold the case to the on-disk name
  (`/…/References.BIB` → `/…/references.bib`), so `fs::canonicalize` on the full
  path is a working fix. (Python's `os.path.realpath` does not, which is a
  misleading way to test this.)
- `insert_field`'s own test proves the doc comment wrong: with input
  `year = {2025}  % note`, the `% note` ends up trailing the inserted `url`
  field, so the code does *not* "keep a trailing inline comment attached to the
  field it documents."

Two corrections to the review, which change the fixes:

1. Important #1 is a robustness gap, not an exploitable one. Its example
   `missing/../references.bib` cannot actually clobber anything: the parent
   fails to canonicalize *because it does not exist*, and `atomic_write` then
   fails at `NamedTempFile::new_in`. Worth failing closed anyway; not a second
   data-loss path.
2. Important #3 (symlink alias) is already handled — `resolved()` canonicalizes
   the parent, so `-o alias/references.bib` with `alias -> .` is caught today.
   The gap is test coverage only.

Out of scope by decision: the optional polish (typed `UnsafeFieldName` /
`UnsafeFieldValue` variants, explicit zero-field `insert_field` behaviour,
`render_derived` output re-validation, `references`-named-project message).

## Changes

### 1. Close the case-alias hole — `crates/cita/src/commands/export.rs`

Rewrite `resolved()` (lines 91–103) to canonicalize the **full** path when the
target resolves, and to fail closed otherwise:

```rust
/// An absolute, canonical path for comparing against managed files.
///
/// An existing target is canonicalized outright, so a case-insensitive
/// filesystem reports the on-disk name and `References.bib` cannot alias
/// `references.bib`. A target that does not exist yet cannot alias an existing
/// managed file, so only its parent is canonicalized, which still collapses
/// `..` segments and symlinked directories.
fn resolved(path: &Path) -> Result<PathBuf> {
    let absolute = std::path::absolute(path)
        .with_context(|| format!("could not resolve {}", path.display()))?;
    if let Ok(canonical) = fs::canonicalize(&absolute) {
        return Ok(canonical);
    }
    let (Some(parent), Some(file)) = (absolute.parent(), absolute.file_name()) else {
        bail!("could not resolve {}", absolute.display());
    };
    let parent = fs::canonicalize(parent)
        .with_context(|| format!("could not resolve the directory {}", parent.display()))?;
    Ok(parent.join(file))
}
```

Why this is symmetric: `ensure_not_managed` runs after `Manifest::load_verified`,
so both managed paths exist and take the full-canonicalize branch. A
non-existent target takes the parent branch and produces `parent-canonical +
exact name`, which equals the managed canonical form whenever they are the same
file. On a case-insensitive volume a "non-existent" case alias *does* exist, so
it takes the first branch and folds onto the managed name — exactly the case we
need to catch. `\\?\` prefixes on Windows appear on both sides, so the
comparison stays consistent.

`ensure_not_managed` and its error message need no change.

### 2. Correct the trailing-comment rationale

`crates/cita-bibliography/src/lib.rs` (~line 333) — replace the inaccurate
claim with the demonstrated invariant. Roughly:

```rust
// The splice point is the end of the last field's value, before any trailing
// whitespace or inline comment, so the existing comma placement is exact and
// the new field can never land inside a `%` comment's line scope. A trailing
// comment therefore ends up documenting the inserted field, as the test shows.
```

`AGENTS.md` "Derived exports" (~lines 86–88) — soften "inserting before the
closing brace would break entries carrying a trailing inline comment" to state
the invariant rather than an unproven failure, e.g. "splices after an entry's
last field value using scanner-owned spans, before any trailing whitespace or
inline comment, so comma placement is exact and the field cannot be swallowed by
a comment."

Also add one sentence to the `insert_field` doc comment recording that a
trailing inline comment moves to the inserted field, since that is observable
output.

### 3. Fix the shelf `--output` help — `crates/cita/src/main.rs` (`ExportArgs`)

The struct is shared between `cita export` (default `<project>.bib`) and
`cita library shelf <name> export` (default `<name>.bib`, from
`shelf_export_path` in `commands/library.rs`), so the current "instead of
`<project>.bib`" is wrong for half its uses. Reword generically, e.g.
`Write the export here instead of the default `<name>.bib`; relative paths use
the caller's directory`.

### 4. Tests — `crates/cita/tests/cli.rs`

Add a probe helper next to the existing fixtures (~line 187) rather than
assuming the platform:

```rust
/// Whether `directory` is on a case-insensitive filesystem, probed so the
/// regression test means something on both macOS and Linux CI.
fn case_insensitive(directory: &Path) -> bool { … write `case-probe`, test `CASE-PROBE`, remove … }
```

Then:

- **Case alias (regression for the critical).** New test: build the fixture with
  `arxiv_library`, snapshot both managed files, then for `References.bib` and
  `CITA.toml` assert `failure(...)` containing `"managed file"` when the probe
  says case-insensitive, and assert a successful, distinct write when it says
  case-sensitive. Assert both managed files are byte-identical afterwards in
  either branch.
- **Symlink alias.** Extend `export_refuses_to_overwrite_the_managed_files` with
  a `#[cfg(unix)]` arm: `std::os::unix::fs::symlink(".", dir.join("alias"))`,
  then `-o alias/references.bib` must fail. (No symlink helper exists in
  `cli.rs` yet, so gate it directly.)
- **INSPIRE → export wiring.** New test using the existing `server(...)` /
  `cita_with_server(...)` / `json_record(...)` helpers (pattern at
  `crates/cita/tests/cli.rs:851`): serve a record whose JSON carries
  `arxiv_eprints` while the BibTeX `entry(...)` carries only a `doi`, `add
  inspire:42`, then `export` and assert
  `url = {https://arxiv.org/pdf/<id>}`. This pins the curated-identity path in
  `derive_entry` that the import-only fixtures never reach. If the client's
  BibTeX/JSON cross-check rejects the eprint-less entry, fall back to a matching
  eprint in both — the wiring is still covered.

## Verification

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Manual check of the actual bug on this machine (macOS APFS, case-insensitive):

```bash
cd "$(mktemp -d)" && cargo run -p cita --manifest-path /Users/binado/personal/cita/Cargo.toml -- init
# import a fixture entry, then:
cargo run -p cita -- export -o References.bib   # must now fail with "managed file"
shasum references.bib                            # must be unchanged
cargo run -p cita -- export -o Cita.toml         # must now fail with "managed file"
cargo run -p cita -- export                      # still succeeds, writes <dir>.bib
```

Confirm before/after: on `main`+branch today the first command succeeds and
rewrites `references.bib` with `url = {…}` lines.
