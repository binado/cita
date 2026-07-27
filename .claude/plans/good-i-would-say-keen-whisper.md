# Plan: shell completions + automated publishing (release-plz)

## Context

We're preparing Cita's first crates.io release on branch `agent/release-readiness`.
The package metadata is already release-ready (inherited workspace metadata,
per-crate READMEs, LICENSE/README landing in every `cargo package`, documented
manual publish order). Two ergonomics/toil gaps remain:

1. **No shell completions** — the single biggest missing ergonomic for a CLI that
   people `cargo install`.
2. **Publishing is fully manual** — the README documents a hand-run, six-crate
   dependency-ordered publish. Error-prone and repeated every release.

Decisions locked with the user:
- **Versioning:** keep **lockstep** (all six crates share one version) via
  release-plz `version_group`.
- **crates.io auth:** classic **`CARGO_REGISTRY_TOKEN`** repo secret (simplest for
  the first publish of brand-new crate names).

release-plz is chosen over cargo-release because the repo already mandates
Conventional Commits (which release-plz parses for version bumps + changelogs),
it's PR-driven (matches Cita's review-everything ethos), and it handles the
six-crate topological publish order automatically. It also generates the
per-crate `CHANGELOG.md`s the repo currently lacks.

---

## Part 1 — Shell completions (`cita completions <shell>`)

Add a `clap_complete`-backed subcommand that prints a completion script to stdout.

**Files:**
- `Cargo.toml` (root): add `clap_complete = "4"` to `[workspace.dependencies]`
  (pin matches the existing `clap` major).
- `crates/cita/Cargo.toml`: add `clap_complete.workspace = true`.
- `crates/cita/src/main.rs`:
  - New subcommand variant on the `Command` enum:
    ```rust
    /// Generate a shell completion script on stdout
    Completions {
        /// Shell to generate completions for
        shell: clap_complete::Shell,
    },
    ```
    `clap_complete::Shell` already derives `ValueEnum`, so it works directly as a
    positional arg (values: bash, elvish, fish, powershell, zsh).
  - Handle it in `run()` — synchronous, needs neither `cwd` nor a manifest:
    ```rust
    Some(Command::Completions { shell }) => {
        let mut command = Cli::command();
        let name = command.get_name().to_string();
        clap_complete::generate(shell, &mut command, name, &mut std::io::stdout());
    }
    ```
    `Cli::command()` / `CommandFactory` is already imported in `main.rs`.
  - Add a parse test in the existing `#[cfg(test)] mod tests`:
    ```rust
    assert!(Cli::try_parse_from(["cita", "completions", "zsh"]).is_ok());
    assert!(Cli::try_parse_from(["cita", "completions", "nonsense"]).is_err());
    ```
- `README.md`: add a completions install line under Quick start / Commands, e.g.
  `cita completions zsh > ~/.zfunc/_cita`.

No changes needed to `commands/mod.rs` — this stays entirely in `main.rs` since it
touches only the clap `Command`, not the manifest.

---

## Part 2 — Automated publishing (release-plz, lockstep, token auth)

**New file `release-plz.toml`** (repo root) — lockstep via shared `version_group`:
```toml
[workspace]
# release-plz reads Conventional Commits for bumps and writes per-crate CHANGELOGs.

[[package]]
name = "cita-core"
version_group = "cita"

[[package]]
name = "cita-bibliography"
version_group = "cita"

# ...one [[package]] block per crate: cita-documents, cita-inspire-client,
# cita-manifest, cita — all with version_group = "cita".
```
All six sharing `version_group = "cita"` makes release-plz assign them the single
highest computed next version, preserving the current "all crates share one
version" contract. Default per-crate git tags (`{{ package }}-v{{ version }}`) and
`changelog_update = true` are kept.

**New file `.github/workflows/release-plz.yml`** — two jobs on push to `main`,
mirroring release-plz's documented quickstart:
- `release-plz-release`: permissions `contents: write`, `pull-requests: write`;
  runs `release-plz release`; env `CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}`.
- `release-plz-pr`: permissions `contents: write`, `pull-requests: write`;
  runs `release-plz release-pr`; `concurrency` guard so overlapping pushes don't
  race the release PR.
- Both: `actions/checkout@v4` with `fetch-depth: 0` (release-plz needs full git
  history) and `dtolnay/rust-toolchain@stable`, plus
  `env.GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}`.

Leave the existing `ci.yml` untouched — it stays the gate; release-plz is separate.

**`README.md` Publishing section:** replace the hand-run numbered steps with the
release-plz flow — merge conventional-commit PRs to `main` → release-plz opens a
"release PR" bumping the shared version and updating changelogs → merging that PR
publishes all crates in dependency order and tags them. Keep one short line noting
the manual `cargo publish` order as an emergency fallback.

**Prerequisites (user does these manually — document in the PR description, not code):**
1. Create a crates.io API token; add it as GitHub Actions secret `CARGO_REGISTRY_TOKEN`.
2. Settings → Actions → General → Workflow permissions → allow GitHub Actions to
   create and approve pull requests (so `release-plz-pr` can open the PR).
3. (Optional) The default `GITHUB_TOKEN` will not trigger `ci.yml` on the release
   PR release-plz opens; if CI-on-release-PR is wanted, swap in a PAT or GitHub App
   token later. Not required for the first release.

---

## Verification

Completions:
```bash
cargo build -p cita
cargo run -p cita -- completions zsh | head          # emits a _cita zsh script
cargo run -p cita -- completions bash | head
cargo test -p cita                                    # new parse tests pass
cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings
```

release-plz config (local, no publish — validates the TOML + version grouping):
```bash
# optional, if release-plz is installed locally:
release-plz update --dry-run     # shows the single shared bump across all six crates
```
YAML: confirm the workflow parses (e.g. `actionlint .github/workflows/release-plz.yml`
if available, otherwise visual review). Full end-to-end publish is only exercisable
once `CARGO_REGISTRY_TOKEN` exists and a commit lands on `main`; that is the user's
first real release, intentionally out of scope for this PR.

## Out of scope (from prior discussion, not requested here)
Man pages (`clap_mangen`), `homepage`/`documentation` manifest fields, the `fetch`
mode-flag `ArgGroup` refactor, and `.claude/` + `E2E_TEST.md` repo hygiene remain
available as follow-ups.
