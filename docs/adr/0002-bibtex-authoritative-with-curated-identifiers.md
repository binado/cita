# ADR 0002: BibTeX-authoritative sources with curated INSPIRE identifiers

- Status: accepted
- Date: 2026-07-18
- Supersedes: parts of ADR 0001 (the "persist selected typed JSON" decision)

## Context

ADR 0001 had INSPIRE sources persist a broad subset of typed JSON fields plus
authoritative BibTeX. In practice that JSON largely duplicated data the BibTeX
already carried (title, authors, year, journal, DOI, eprint), enlarged the
tracked `cita.toml`, and coupled the durable schema to INSPIRE's evolving JSON
shape. The only durably useful facts the BibTeX cannot express are the stable
provider identity and refresh key.

Cita is a niche HEP tool, so first-class HEP identifiers are worth curating even
if that narrows the general use case.

## Decision

`cita.toml` is schema 3. Each local key owns one `SourceSnapshot` tagged by the
source that owns its refresh lifecycle:

- `source = "inspire"`: authoritative BibTeX, the `record_id` refresh key, an
  `updated` timestamp, and a curated `identifiers` block of canonical normalized
  `arxiv`/`doi`.
- `source = "import"`: one exact standalone BibTeX entry, never refreshed.

Every source projects its `Reference` from its BibTeX. INSPIRE entries then
override the projected arXiv/DOI with their stored identifiers and add the
`inspire` provider id, so identity is canonical and independent of BibTeX
rendering while all bibliographic content stays derived from BibTeX. The broad
typed JSON subset is dropped.

The INSPIRE client returns a lean `InspireRecord` (record id, timestamp, texkey,
BibTeX, arXiv, DOI). It still fetches JSON to obtain identity, refresh
bookkeeping, and texkeys, and cross-checks the authoritative BibTeX against the
JSON identity at fetch and refresh time rather than persisting a JSON structure.
Transient `fetch`/`open` resolve JSON only; `--save` additionally fetches the
BibTeX and stores the record.

Schema 3 is a clean break: schema 1 and 2 are rejected with no automatic
migration, consistent with the 1→2 policy.

## Consequences

- `cita.toml` is smaller and its durable schema is owned by Cita, not INSPIRE.
- Adding INSPIRE fields no longer edits the persisted schema; upstream JSON
  drift is absorbed at the permissive wire layer.
- Projection quality now depends on INSPIRE's canonical BibTeX; curated
  publication selection, author-role filtering, and preprint-date year fallback
  from the old JSON subset are no longer available. Narrow optional scalar
  overrides can be added later if a real gap appears.
- The JSON↔BibTeX identity cross-check runs at fetch/refresh time instead of
  being a persisted, re-validated structure.
- The `identifiers` block is forward-compatible with future providers such as
  NASA ADS bibcodes.
