# Split `cita/src/main.rs` into one module per subcommand

## Context

`crates/cita/src/main.rs` has grown to ~685 lines and mixes three unrelated
concerns in one file: the clap CLI surface, a thin dispatch, and eight command
handlers with their private helpers (e.g. `list`'s `wrap`/`truncate`/`print_rows`
and `fetch`'s `Selected`/`select`/`FetchTarget`). This hurts legibility — finding
the code for one subcommand means scrolling past all the others.

The crate already has a precedent for this: `mod git;` lives in `git.rs` and
exposes `pub fn commit(...)` called as `git::commit(...)`. We extend that pattern
with a `commands/` module directory, one file per subcommand, leaving `git.rs`
untouched. This is a **pure structural refactor** — no behavior changes.

## Target layout

```text
crates/cita/src/
  main.rs          # Cli, Command, SortBy, Order, main(), run() dispatch, CLI-parse tests
  git.rs           # unchanged
  commands/
    mod.rs         # module decls + re-exports + shared helpers
    init.rs
    import.rs
    add.rs
    sync.rs
    remove.rs
    generate.rs
    list.rs
    fetch.rs
```

## What stays in `main.rs`

- `Cli`, `Command`, `SortBy`, `Order` (the clap surface stays in one place).
- `main()` and `run()`. `run()` keeps its `match cli.command` but each arm calls a
  handler re-exported from `commands` (e.g. `commands::add(&cwd, ...)`).
- `mod git;` and a new `mod commands;`.
- The CLI-parse tests `key_only_accepts_one_locator` and
  `document_conflicts_are_enforced` (they exercise `Cli::try_parse_from`).
- `SortBy`/`Order` are referenced by `commands::list` via `crate::{SortBy, Order}`.

## `commands/mod.rs` — shared helpers

Move the cross-command helpers here as `pub(crate)` so every handler can reach
them (this is what prevents circular `use` noise):

- `find_manifest` (used by import/add/sync/remove/generate/fetch/commit).
- `ensure_cache_layout` + consts `CACHE_IGNORE_COMMENT`, `CACHE_IGNORE_RULE`
  (used by init and fetch).
- `inspire_client` (used by add/sync/fetch).
- `add_message` + `print_add` (used by add/import/fetch).

Declare the eight submodules and `pub use` each handler fn so `run()` can call
`commands::init(...)`, `commands::add(...)`, etc.

## Per-command modules

Each file holds its handler plus its private helpers/types and its own tests:

- `init.rs` — `init` (uses `git::repository_root`, `super::ensure_cache_layout`).
- `import.rs` — `import`.
- `add.rs` — `add`.
- `sync.rs` — `sync`.
- `remove.rs` — `remove`.
- `generate.rs` — `generate`.
- `list.rs` — `list`, `Row`, `ordered`, `print_rows`, `column_width`, `truncate`,
  `wrap`, and the `unicode_title_helpers` test.
- `fetch.rs` — `fetch`, `Selected`, `select`, `FetchTarget`, `open_target`,
  `fetch_selected`, `document_error_with_hint`, `fetch_message`, and the
  `fetch_targets_render_the_value_passed_to_the_opener` test. **Also move the
  `FetchPolicy` construction** (the `if force {…} else if cache_only {…}` block)
  out of `run()` and into `commands::fetch`, which takes the raw flags
  (`force, cache_only, url, open, save, selector`). This keeps `main.rs` from
  importing `FetchPolicy` and keeps the dispatch arm trivial.

## Import mechanics

- Each command module pulls its own crate deps (`cita_manifest::…`, `anyhow::…`,
  `std::path::Path`, etc.) — imports become local instead of one giant top-level
  block. Reach shared helpers via `super::{find_manifest, print_add, …}` and CLI
  enums via `crate::{SortBy, Order}`.
- Trim `main.rs`'s `use` block down to what dispatch + the CLI enums still need
  (clap, tokio, `std::env`, `std::path::Path`).
- Mark moved items `pub(crate)` (or `pub` within `commands`) only as far as each
  call site requires; keep command-private helpers private to their module.

## Verification

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings   # dead-code / unused-import proof
cargo test --workspace
cargo test --test cli                                   # end-to-end CLI behavior unchanged
cargo build --workspace
```

Then a quick manual smoke check that dispatch still wires up:

```bash
cargo run -p cita -- --help        # help lists every subcommand
cargo run -p cita -- list          # a real handler runs
```

Success = identical behavior, no clippy warnings, all existing tests green, with
no test logic changed (only relocated next to the code it covers).
