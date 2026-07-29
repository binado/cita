# bibi — design document

Status: draft. Supersedes `cita`, which this design replaces under a new name.
This is a clean break: there is no compatibility or migration path from
`cita.toml`, `cita-library.toml`, or any previous cita schema. The package has no
users whose data must be carried forward.

Everything here is decided. §12 lists what has been deliberately deferred, and
§11 records what was rejected and why, so that neither is reopened by accident.

> **Partly superseded.** `BIBI_TWO_BINARIES.md` splits bibi into a project-local
> file manager and a deferred user-level library manager, and phase one of that
> document has landed. Wherever this document describes a global manifest and
> `-g/--global`, an `export` command, or a managed document cache with
> `cache clean`, it describes what bibi *was*: those are gone, `list --format
> bibtex` plus shell redirection is how a bibliography is written, and `fetch`
> downloads one file into the working directory. Everything else here still
> holds. Full reconciliation waits until the second binary exists.

---

## 1. What bibi is

A command-line tool that maintains a bibliography as a manifest, and renders a
BibTeX file from it on demand. The manifest is the only project-state file bibi
maintains; the `.bib` is output, produced by an explicit command and consumed
directly by LaTeX. A separate user-level document cache is derived and
disposable (§7).

The distinguishing bet: bibliographic metadata is **owned by providers**, not by
the user. The user chooses *which* references they cite and *what to call them*;
everything else — titles, authors, journals, identifiers, the BibTeX bytes — is
fetched and refreshed rather than authored.

Where no provider holds a work, the user supplies its BibTeX once and bibi stores
it verbatim (§4). That is an admission of a boundary, not an editing feature:
supplied metadata is still never rewritten, and the escape hatch is closed to
anything a provider could have resolved.

### Goals

- Adding, removing, and listing references is a fast CLI operation.
- Rendering a bibliography is reproducible: the same manifest and options always
  produce the same bytes.
- Metadata staleness is repaired by a single command, cheaply enough to run often.
- Downloaded PDFs and sources are shared across projects and are disposable.
- The core is provider-agnostic; INSPIRE is the first provider, not the only one.

### Non-goals

- A reference *editor*. Metadata can be supplied once for a work no provider
  holds, but nothing in bibi amends stored metadata afterwards.
- A reading/annotation manager. No notes, no highlights, no PDF metadata writing.
- A citation formatter. Rendering styles is the LaTeX toolchain's job.
- An aggregate view across projects. Each project's bibliography stands alone.
- Version control. The manifest is meant to be tracked and reviewed, but staging
  and committing it is the user's own workflow, not something bibi performs.

---

## 2. Core invariants

These are the load-bearing constraints. Everything below is downstream of them,
and a proposed feature that violates one of these should be rejected rather than
accommodated.

**I1. Every record in the manifest is provider-owned.** Each record names the
provider that owns its refresh lifecycle. There is no second class of record and
no variant to special-case; providers differ in *capability*, not in kind, and a
provider whose records never change is an ordinary provider (§4).

**I2. bibi never authors or reformats BibTeX.** Stored BibTeX is byte-identical
to what it was given, whether that came from a provider or from the user. The
only permitted transformation is rewriting the citation key token. Should
field-level policy ever be reintroduced (§10), it may splice in whole fields, and
nothing may round-trip an entry through a formatter.

**I3. bibi never interprets a rendering when the source is available.** A
provider's BibTeX is a lossy rendering of its structured record — author strings
requiring von/jr parsing, collaborations flattened into the author field, a year
chosen by someone's formatter. Where a structured record exists, a record's
fields are derived from it. Where the BibTeX *is* the original, as in
user-supplied records, deriving them from it violates nothing.

**I4. Local citation keys adopt the source key at ingestion and never change
without the user asking.** A provider-owned record takes the texkey from the
provider's BibTeX; an imported or local record takes the texkey supplied by the
user. `--key` overrides either default. A refresh, a re-resolution, or a
provider-side key change must never invalidate a `\cite{}` in the user's
document.

**I5. bibi record identity is surrogate and immutable.** Every record carries a
bibi-generated UUIDv4 id, minted once and never changed by rename, refresh,
re-resolution, or provider migration. Provider ids may change; the bibi id does
not. It is an internal identity rather than a normal CLI selector.

**I6. Rendering is reproducible.** The same manifest and the same rendering
options always produce byte-identical output. No network, no cache probing, no
machine-specific paths, no ambient state. Plain `export` and `check` are offline.
`export --provider`, explicitly, performs a sync mutation first and then invokes
the same pure renderer against the resulting manifest (§10). bibi maintains no
continuously-synchronized artifact; an output the user chooses to keep can be
verified explicitly with `check`.

**I7. Description never appears in a generated artifact.** Title, authors,
collaborations, and year exist for terminal output alone. Rendering uses only the
stored local key and BibTeX payload. A wrong description can produce a poor
listing but cannot change a deliverable.

**I8. Mutations are atomic.** A command validates a complete candidate state
before writing anything. An interrupted command leaves the previous state intact
and fully valid. Because the manifest is the only authoritative project-state
file bibi maintains, this concerns a single write rather than a coordinated pair.

---

## 3. Data model

**Decided.** A record consists of six groups of fields, distinguished by the role
they play rather than by where the data came from:

| Group | Fields | Role |
| --- | --- | --- |
| Identity | id | Bibi-generated UUIDv4. Immutable and unique within the manifest. |
| Naming | local key | The user-facing citation key. Unique within the manifest. Renameable. |
| Provenance | provider, provider id, revision | Which provider owns refresh, its stable handle, and its opaque change token. Only the provider name is always present: a local record has neither a provider id nor a revision. Providers that cannot supply a useful revision leave it absent. |
| Identifiers | doi, arxiv | Canonical normalized values. Optional. Operational, not descriptive. |
| Payload | bibtex | Verbatim BibTeX, as received. |
| Description | title, authors, collaborations, year | Advisory display data (I7). |

Identifiers and description are both derived from the provider's structured
record at fetch time and stored, so that listing, selection, and deduplication
need neither a parse nor a network call. They are nonetheless not the same kind
of thing, and are separated deliberately — see below.

There is deliberately no term covering "everything derived from the provider."
Each group is named for the role it plays, because the roles differ in what a
wrong value costs.

### Why derived fields come from the structured record

- **Author names.** BibTeX author fields are `and`-separated with von/jr
  conventions and brace-protected accents; recovering individual names means
  running a parser that makes judgment calls. Providers return structured name
  arrays.
- **Collaborations.** BibTeX cannot cleanly express "the ATLAS Collaboration" as
  distinct from an author; providers return it as its own field. This is a case
  where the structured record is strictly more expressive than the BibTeX.
- **Year.** Preprint year and publication year differ. BibTeX gives whichever
  the provider's renderer chose; the structured record lets bibi decide.
- **Identity.** Normalized DOI and arXiv identifiers are already best taken from
  the structured record. Deriving them from BibTeX requires a reconciliation
  step that exists only to compensate for the weaker source.
- **Provider-agnosticism.** BibTeX quality varies sharply between providers;
  some generate it mechanically. A BibTeX-first core pins metadata quality to
  the worst provider in the set.

### Cost accepted

Because the structured record is not retained, the derived fields are **trusted
on write and not re-derivable offline**. The manifest is no longer
self-verifying: a reviewer reading a diff cannot confirm the title from the
BibTeX beside it.

This is accepted, and it costs less than it first appears. By I7, description
never supplies rendered bibliographic metadata. A wrong value can print a wrong
line; it cannot alter the stored BibTeX or local key.

The provider and provider id also make the derived fields
recoverable *with* network access, and a refresh repairs them. The alternative —
storing the provider's structured record alongside the BibTeX — turns the
manifest into a database that encodes a provider's schema, and is rejected.

### No cross-check between derived fields and BibTeX

**Decided.** Derived fields are not verified against the provider's BibTeX. Both
representations are accepted as the provider gave them.

Such a check would contradict I3. Its justification is that BibTeX author fields
are a lossy rendering requiring judgment-call parsing — so verifying against them
means running the very parser the design declares unreliable, then adjudicating
disagreements that are legitimate and routine: `and others` against a full
structured author array, collaborations flattened into the author field,
brace-protected accents. The check would fire on exactly the fields it exists to
protect. Narrowed to scalar identifier fields it would be tractable, but by then
it protects almost nothing.

It would also be the wrong instrument for the one hazard nearby. Bulk BibTeX
retrieval (§5) does make the association between a record and its entry
*inferential*, and that is worth defending against — but the defense is the
verified bijection between texkeys and requested records, which is exact, cheap,
and fails loudly. Comparing a parsed title against a structured title is a fuzzy
proxy for the same question, and one that cannot distinguish a mispairing from
the routine disagreements above. Where the join holds, a remaining mismatch
requires a provider to serve inconsistent content for a record it identified by
its own key — a failure that would corrupt its structured responses equally, and
that no client-side comparison can defend against.

**Structural validation is separate and remains.** A provider response must be
exactly one well-formed BibTeX entry, parseable, with an extractable citation key
and no directives or trailing content. This is not verification of the provider's
claims; it protects bibi's own downstream invariants, since the entry will be
re-keyed (I2) and rendered deterministically (I6). Rejecting a malformed response
is input validation. Rejecting a well-formed one because a parse of it disagrees
with the structured record is the cross-check, and that is what is dropped.

Two consequences follow. Interpreting BibTeX as metadata leaves every provider
path and survives only inside the local provider (§4), so the core never
interprets provider BibTeX at all. And the provider's responsibility to keep its
two representations consistent (§4) becomes load-bearing rather than decorative:
where they genuinely disagree, a listing and the rendered bibliography can differ.
That is a display inconsistency rather than corruption. A forced refresh
refetches both representations, but only the provider can repair a disagreement
it continues to serve.

### Identifiers are not description

**Decided.** Normalized DOI and arXiv identifiers are a group of their own, not
description fields, even though both are derived from the same provider response
at the same moment. They are provider-neutral: there is no provider-specific
identifier block.

The distinction is role, not origin, and it rests on what a wrong value costs.

**Description is advisory.** By I7 it never supplies bibliographic metadata to a
generated artifact and has no non-display use.

**Identifiers are load-bearing.** They are operational inputs to four paths, none
of them display, and a wrong value in any of them reaches the deliverable:

- **Deduplication** keys off DOI, arXiv id, and provider id (§3).
  A wrong value cites the same work twice, or silently suppresses an addition.
- **Selection** resolves user-supplied identifiers to records (§10).
- **Document retrieval** addresses the cache by arXiv id (§7). A wrong
  value fetches the wrong document.
- **Import resolution** extracts them from user BibTeX to resolve records (§6).

A second axis separates them, running opposite to intuition. **Identifiers are
immutable once set** — an arXiv identifier never changes, and a DOI may be
*added* on publication but never changes value. **Description genuinely
changes**: referees demand title changes, author lists shift between preprint and
publication, and the year moves from preprint to publication. Of everything in
the record, description is the least stable.

Two consequences follow:

- **Refresh distinguishes them** (§5), and now on a principled basis rather than
  a judgment call. Description changes are routine and reported in aggregate.
  An identifier change is rare and significant — a preprint gaining a DOI may
  create a duplicate relationship with another record or change which cached
  document it addresses — and is reported individually.
- **Local records carry a correct trust gradient.** A software or dataset
  citation may have a real DOI while its title and authors are whatever the user
  pasted. The identifier is as authoritative as on any managed record — it is a
  fact about the work — and the description is not. One record, two trust levels,
  correctly marked.

Filtering does not disturb this. A listing filtered by author or title substring
is a convenience over description, and a filter that misses returns a poor search
result. Resolving an identifier to a record is a correctness operation, and one
that binds the wrong record corrupts data. Filtering is not resolving.

**Shape: canonical normalized singles, both optional.** Every operational use
wants exactly one value — one arXiv id addresses one cache entry, one selector
resolves to one record. Multiplicity such as erratum DOIs or cross-listings is
descriptive; if it is ever wanted it belongs with the description. Choosing the
canonical value is provider mapping policy, alongside the year-selection policy
assigned to providers in §4.

This sharpens the deferred integrity question (§7): with the arXiv identifier
addressing the blob store, the versioned-versus-versionless decision lands on a
record field rather than on a cache-naming convention.

### Unmapped provider fields are discarded

**Decided.** A provider response carries far more than the record model uses —
abstracts, keywords, affiliations, citation counts, curation flags, reference
lists. None of it is stored. Unknown and unmapped fields are tolerated on the way
in and dropped on the way to disk.

**A field enters the model when bibi operates on it, not when a provider offers
it.** That is the same rule the six groups are built on; there is no seventh
group for leftovers.

Nothing bibliographic is lost, because it is in the payload. Journal, volume,
pages, publisher, editors, report numbers, ISBN — whatever the provider
considered citation-relevant is already in the verbatim BibTeX (I2), and that is
what gets rendered. The record model does not duplicate it.

Retaining the unmapped remainder as an opaque blob is rejected. It is §11's
"store the provider's structured record" under another name, and it carries three
specific costs:

- **Diff churn.** Citation counts and curation timestamps change constantly.
  Storing them produces a diff on every record at every refresh even when nothing
  bibliographic changed, undoing the quiet-refresh property of §5.
- **Loss of byte-stability.** An opaque string inherits the provider's key
  ordering and whitespace, so two refreshes of an unchanged record can render
  differently. Repairing that means canonicalizing the JSON under a specification
  bibi would then own — interpretation, which is what opacity was meant to avoid.
- **N providers, N shapes.** Any consumer would need per-provider interpretation,
  at which point the data should have been a mapped field.

When a provider field does look worth keeping, four questions decide it:

1. **Does bibi operate on it?** If not, it is not stored.
2. **Is it already in the payload?** Document type is the BibTeX entry type;
   report numbers and experiment tags are entry fields. Do not duplicate.
3. **Is it volatile?** Citation counts change daily. Sorting a listing by
   citations is a reasonable feature, but as a query-time fetch, never as stored
   state in a tracked file.
4. **Otherwise, map it deliberately** as a named field in whichever group its
   role assigns it, with the mapping owned by the provider layer alongside the
   other per-provider policy in §4.

### Unknown fields: opposite policies in each direction

**Decided.** The tolerance above applies to provider responses only.

- **Provider responses tolerate unknown fields.** bibi consumes a foreign,
  evolving schema and wants a subset of it; a provider adding a field must never
  break a refresh.
- **The manifest rejects unknown fields.** An unrecognized field means the file
  was written by a newer version of bibi. Ignoring it would mean the next write
  silently drops data that was not understood — a lossy round-trip on the user's
  tracked file. Failing and reporting that the manifest requires a newer version
  is the correct behavior.

Same mechanism, opposite policy, because in one direction bibi is a reader of
someone else's format and in the other it is about to rewrite its own.

The manifest therefore carries a **schema version**, so that a file from a newer
bibi is diagnosed directly rather than inferred from an unrecognized key, and the
error names the actual problem.

### Bibi record identity

**Decided.** Every record carries a bibi-generated UUIDv4 id (I5), serialized as
a canonical lowercase hyphenated UUID. It is minted once, checked for collision
within the fully loaded candidate manifest, and preserved by every mutation that
updates the record.

The bibi id is the primary key, not a deduplication key and not an ordinary CLI
selector. Two collaborators can add the same work on separate branches and mint
different bibi ids; deduplication therefore continues to use normalized DOI,
arXiv id, and provider id. Provider ids belong to provenance and may be replaced
by explicit provider migration, while bibi identity remains fixed.

### Adopting the local key

**Decided.** When the user does not supply `--key`, bibi adopts the texkey from
the source BibTeX. Provider-owned records therefore use the provider's naming
convention — in particular, records resolved through INSPIRE receive INSPIRE
texkeys. Imported and local-provider records retain the texkeys the user supplied.

The texkey is obtained by the same syntactic read already required for structural
validation and re-keying (I2); it is not interpreted as bibliographic metadata
(I3).

Three constraints keep adoption safe:

- **Adopt once.** Sync never changes a stored local key, even if a provider later
  returns a different texkey. Provider migration also preserves the existing
  local key. Changing it always requires `rename`.
- **User choice wins.** `add --key <key>` overrides the source texkey at
  ingestion. During `add --overwrite`, naming an existing key can also identify
  the overwrite target (§10).
- **Collisions are errors, except where there is no `--key` to offer.** If the
  adopted or requested key belongs to a different record, the item is rejected
  and the diagnostic asks the user to choose `--key`. bibi never silently
  appends a suffix, because the resulting key would be neither the provider's
  nor the user's. A file import has no per-entry `--key`, so there a colliding
  entry is skipped rather than failed, and `--overwrite` makes the colliding key
  name the overwrite target (§10).

---

## 4. Providers

**Decided.** A provider is responsible for:

- Resolving locators (arXiv id, DOI, provider id) to records, **many at once**,
  batching however its API demands.
- Refreshing many records at once by stable provider id, likewise batched.
- Joining batched payloads back to the records that were asked for, and refusing
  a batch it cannot pair exactly (§5).
- Mapping its own structured response to the record's identifiers and description.
- **Keeping its structured record and its BibTeX consistent with each other.**
  bibi does not verify this (§3), so it is a genuine obligation of the provider
  rather than a courtesy: everything downstream assumes the two representations
  describe the same work.

The core defines the record shape and the provider contract; it contains no
provider-specific mapping. Providers are addressed by name, and `sync --provider
<name>` refreshes only records owned by that provider.

### What a provider returns

**Decided.** The contract follows directly from the six groups of §3: **a
provider produces provenance, identifiers, description, and payload.** The
manifest layer mints identity. The provider does not return naming as a separate
field, but the texkey in its structurally validated payload is adopted as the
default local key (I4). Nothing further needs deciding; the contract is a
consequence of the data model.

### Provider selection and locator qualification

**Decided.** A caller may select a provider either with `--provider <name>` or
with a qualified locator such as `inspire:12345` or `ads:2024ApJ...`. If both are
present they must name the same provider; disagreement is a usage error detected
before any network request.

DOI and arXiv locators are provider-neutral. Without an explicit provider, bibi
tries providers in the documented roster order (§4) and accepts the first
successful resolution. Provider-specific record ids must be qualified unless
their syntax identifies exactly one installed provider; an ambiguous bare id is
an error rather than a guess. `--provider` constrains every locator in one
invocation, including every entry resolved by `add -f`.

Provider migration is consequently explicit:
`add --overwrite --provider <name> <locator>` re-resolves through the named
provider and updates the uniquely matching existing record while preserving its
local key. Matching normally uses the deduplication identifiers. When a migration
has no identifier in common with the old record,
`--key <existing-local-key>` explicitly names the overwrite target; bibi never
guesses which record to replace.

Resolution has five outcomes:

- **Found** — a complete mapped provider result.
- **Not found** — the provider understands the locator but has no record.
- **Unsupported locator** — the provider cannot resolve that locator kind.
- **Retrieval error** — network, authentication, status, or exhausted retry.
- **Mapping error** — retrieved content is malformed or cannot satisfy the
  provider contract.

**Fallback occurs only after absence.** When resolution is unconstrained,
`not found` and `unsupported locator` advance to the next provider in roster
order. A retrieval or mapping error stops resolution of that locator immediately:
bibi returns nonzero, names the failed provider, and tells the user to retry or
manually choose another provider with `--provider`. A temporary outage must not
silently determine permanent BibTeX and texkey provenance. Errors must also never
be converted into local records (§6).

### Providers are built in two halves

**Decided.** A provider is composed of two steps, both available on their own,
with a wrapper that runs them in sequence:

1. **Retrieval** — obtain the provider's own response. Impure: network, HTTP
   status, authentication, rate limiting.
2. **Mapping** — turn that response into record fields. Pure: no I/O, no clock,
   no ambient state.

**The seam is the network boundary**, which is what makes the second step a pure
function of its input. That matters because mapping is where every per-provider
judgment call lives — year selection across errata and proceedings, author and
collaboration extraction, identifier canonicalization — and where a provider
changing its schema does its damage. Pure means those are testable against
captured responses, with no network, no rate limits, and no flakiness.

Two further consequences:

- **The two failure classes separate.** Retrieval fails with network, status, and
  rate-limit errors, which are retryable and where §5's bounded retry belongs.
  Mapping fails on malformed or unmappable content, which is not retryable —
  reattempting a record whose date will not parse merely fails again. Fusing the
  steps yields one error type where two are needed.
- **Diagnostics need no separate mechanism.** Emitting a provider's unabridged
  response — to inspect why a record mapped as it did, to capture fixtures, to
  script against a provider directly — is the first step called on its own.

**The contract, however, is over the composition.** Generic operations — add,
import, refresh — go through the wrapper selected by the provider-selection
layer. They may constrain that selection by provider name, but the provider's
response shape itself does not cross into the contract, for two reasons:

- Untyped, it would make the contract "providers return arbitrary data," coupling
  any caller that reads it to one provider's schema — provider-agnosticism in
  name only.
- Typed per provider, it would break the dynamic dispatch the design requires
  (below), since providers must coexist behind one contract.

Reaching for the halves directly is therefore provider-specific by construction,
which is acceptable for tests and diagnostics precisely because the coupling is
visible: a caller had to name a provider to obtain it. What is prohibited is that
coupling arriving invisibly, through an untyped value flowing across a generic
contract. Otherwise the four-question procedure of §3 acquires a back door,
fields get *used* without being *modeled*, and a command ends up depending on one
provider's key that no second provider can supply.

Note also that retrieval requests a **narrowed field set** on the refresh path
(§5), so what it returns there is already only the subset mapping needs. A
diagnostic that wants the provider's full record makes its own unnarrowed
request rather than reusing the refresh path's response.

The local provider (below) degenerates rather than breaking: its retrieval step
reads user-supplied BibTeX instead of calling a network, and its mapping step is
still the pure half where description is derived.

Consequences to accept:

- **The contract must support dynamic dispatch.** Multiple providers coexist in
  one manifest, so the core holds a heterogeneous set of them.
- **Per-provider mapping policy is real work.** Choosing "the year" from a
  provider's publication metadata has edge cases (errata, conference vs journal)
  that BibTeX previously decided implicitly. Each provider now owns that policy.
- **Credentials become a concern.** Some providers require API keys. Keys live
  in user configuration or the environment — never in the manifest, which is
  tracked in version control.

### The local provider

**Decided.** Not everything a bibliography cites exists in a provider database:
software and dataset releases, references outside the tool's subject area, and
genuinely unpublished material — lecture notes, internal notes, work in
preparation, private communication. These are admitted as records owned by a
**local provider**: the user supplies the BibTeX, its description is derived from
it (permitted by I3, since here the BibTeX is the original rather than a
rendering), and the provider supports no refresh.

The rule that keeps this from becoming a special case:

> **Nothing outside the provider layer knows that `local` is special.** Refresh
> does not test for it and skip; it asks every provider to refresh its records,
> and the local provider reports none. A `provider == "local"` test appearing in
> a command means the abstraction has failed.

Locality is therefore a *capability* — whether a provider supports refresh — and
I1 holds unchanged: every record is provider-owned, and this provider's ownership
happens to be that nothing ever changes.

Constraints:

- **The local path must not become the easy path.** Adding user-supplied BibTeX
  refuses, absent an explicit override, when the entry carries a DOI or arXiv
  identifier that a provider would resolve. Without this, records silently lose
  refresh because resolution failed once or because it was quicker to paste.
- **Refresh reports what it skipped.** Unrefreshable records are counted in the
  output so that a growing population of them is visible rather than quietly
  rotting.
- **Local records can be upgraded.** When a new provider covers a record that was
  previously local, re-resolution replaces its provider, provider id, and BibTeX
  while preserving its bibi id and local key. Provider migration is a first-class
  operation, not delete-and-re-add.
- **Input is BibTeX, not structured fields.** A structured input mode would force
  bibi to author the BibTeX, which is rejected (§11). Users have BibTeX already —
  from publishers, from Zenodo, from search tools.

### Provider roster

INSPIRE is the first provider. The local provider exists from the start, since
completeness is a hard requirement (§6).

Note that a record's absence from arXiv does not imply absence from a provider:
INSPIRE indexes pre-arXiv literature thoroughly, so old papers are among the
best-resolved records, by DOI or record id. The genuinely unresolvable set is
narrower than "not on arXiv" in that direction and wider in another — software
releases, datasets, and out-of-subject references.

**Provider choice is permanent in a way it looks like it is not.** Because stored
BibTeX is verbatim and never repaired (I2), whichever provider resolves a record
determines the quality of that entry in every bibliography rendered from it,
with no correction available short of re-resolving elsewhere. A provider whose
BibTeX omits journal, volume, and pages, or renders journal names in a form
publishers do not expect, produces permanently worse entries than one that does
not. Coverage is therefore not the only axis on which a provider is chosen, and
usually not the deciding one.

The intended order:

1. **INSPIRE** — HEP. Excellent BibTeX: the journal abbreviations HEP publishers
   expect, eprint fields, collaboration handling.
2. **ADS** — astronomy and instrumentation, where INSPIRE is thinnest, with
   BibTeX of comparable quality. It is also the provider that first exercises two
   parts of this design that INSPIRE never will: it requires an API token, making
   credential handling real rather than stipulated; and its record identifiers
   can *change* when a preprint becomes a published paper, making explicit
   re-resolution and provider migration real rather than stipulated.
3. **A DOI registry for software and datasets** — the residue that actually
   drives local records. Preferred over a general journal-article registry, whose
   coverage duplicates what INSPIRE and ADS already serve better. The usual
   BibTeX-quality objection barely applies here: a software release is a title,
   authors, a version, and a DOI, so there is little for a renderer to lose.

---

## 5. Refresh

**Decided.** Refresh is **conditional**:

1. Fetch structured records for all managed ids in batches, requesting only the
   fields the record's identifiers, description, and revision need.
2. Compare each provider's opaque revision token against the stored one.
3. For changed records only, fetch BibTeX in batches, and **join each returned
   entry to exactly one requested record or fail the batch**.

A revision is provider-defined and need not be a timestamp. A provider that
cannot supply a token with the required semantics leaves it absent; its records
are treated as changed on every sync rather than incorrectly assumed current.

`sync --force` bypasses revision equality and refetches the structured record and
BibTeX for every refreshable record. It exists both for recovery and for provider
changes that do not advance the record revision, such as a correction to the
provider's BibTeX renderer. It does not bypass structural validation, bounded
retries, or atomic candidate validation.

### Why this shape

Two costs are being traded against each other, and the resolution is different
for each.

**Transferring full records for everything on every sync** is simply waste, and
it is the known cause of slow refreshes today. Narrowing the requested field set
and gating BibTeX on a revision comparison removes it: a typical refresh changes
nothing and fetches no BibTeX at all.

**Pairing bulk BibTeX to records** is the real hazard. A bulk BibTeX response is
a flat concatenation of entries with no envelope — nothing in the format carries
the provider's record identifier — while the structured response is
self-identifying. Associating the two means splitting the stream, reading each
citation key, and matching it against the provider-declared keys of the records
in the same batch. Done carelessly, a mispairing silently attaches one paper's
BibTeX to another paper's record.

**Decided: batch, and verify the join.** Within one batch, bibi builds an index
from provider-declared texkey to requested record, and every returned entry must
match exactly one record through it.

Two outcomes are *ambiguity*, and they fail the batch whole: an entry whose
texkey matches no requested record, and a texkey claimed by more than one record.
In both, no pairing in that response can be trusted, so none is accepted.

A requested record that receives *no* entry is not ambiguity. Nothing is unclear
about absence: no entry claims that record. It is the missing record described
below — a no-op with a warning — and the rest of the batch proceeds.

The join is therefore verified rather than assumed, and the failure the hazard
describes — a *silent* mispairing — cannot occur: a batch is provably paired,
refused whole, or short by records that are individually reported.

The alternative was fetching BibTeX one record at a time, making the request URL
the join key so the ambiguity is never created rather than checked. It is the
simpler rule and it was the earlier decision here. It loses to arithmetic.
Providers publish request budgets — INSPIRE allows fifteen requests per five
seconds, and counts rejected requests against that — so per-record retrieval
puts a hard floor under every operation proportional to the number of records
touched. Importing three hundred entries or running `sync --force` over them
costs minutes of pacing per-record, against seconds when batched. That cost is
paid on exactly the operations a user runs when they are waiting for the tool,
and it buys a property that ninety lines of index-and-verify also buys.

Rate limiting is respected **proactively**: a provider paces its own traffic
under its documented request budget, with bounded retries behind that as a
fallback, and each retry reported. Batching reduces how often pacing binds; it
does not replace it, since a provider's budget still governs the batches
themselves.

### What refresh reports

Changes are not all equal, and the output distinguishes them (§3, identifiers):

- **Description changes** — a corrected title, a grown author list — are routine
  and reported only in aggregate.
- **Identifier changes** are reported individually. A preprint gaining a DOI on
  publication may create a duplicate relationship with an existing record or
  change which document belongs to it, and neither should pass silently. An
  identifier *added* is accepted; one *replaced* fails that record, because §3
  holds that these values do not change once set — so a replacement means the
  stored value, the new value, or the provider is wrong, and repair is the
  explicit re-resolution below rather than a routine refresh.
- **Records that cannot be refreshed** — those owned by the local provider (§4) —
  are counted, so that a growing population of them stays visible.
- **Records owned by a provider this build does not carry** are counted
  separately and warned about once per provider name. A manifest may legitimately
  name a provider that is absent here; refusing to refresh anything on that
  account would make the manifest unmaintainable by the build that can still
  maintain most of it. Naming that provider explicitly is different, and is a
  usage error.

### When a provider id stops resolving

**Decided.** A lookup that returns nothing is a **no-op with a warning on
stderr**. The record is left exactly as it was, and the rest of the refresh
proceeds. This covers both a record absent from a structured batch and a record
that received no BibTeX entry from one — absence is per-record, whichever
representation went missing.

This is a normal event rather than an exception: ADS identifiers change when a
preprint becomes a published paper (§4), and providers occasionally merge or
withdraw records. Failing the whole refresh over one of them would make routine
maintenance impossible.

bibi does not attempt to re-resolve the record automatically by DOI or arXiv id.
That would be silent rebinding inside a bulk operation — the failure class §6
refuses for title matching, and for the same reason: a wrong binding is
discovered late and corrupts a deliverable. Recovery is the user's explicit
`add --overwrite --provider <name> <locator>` (or an equivalent qualified
locator), which is the same mechanism used to migrate a record between providers
(§4).

---

## 6. Import

**Decided.** Import **resolves**; it does not ingest.

An incoming BibTeX file is parsed far enough to extract each entry's identifiers
(arXiv id, DOI). These are resolved against a provider **in batches** — a file of
several hundred entries is the case that makes per-entry resolution painful — and
each result is stored as an ordinary provider-owned record; the parse result is
discarded. Entries that resolve are
therefore upgraded — they gain refresh, canonical identifiers, and provider
BibTeX. Every new record, resolved or local, receives a freshly minted bibi id.

Requirements:

- **The local key is taken from the imported entry, not from the provider.**
  Importing a collaborator's bibliography must not break existing `\cite{}`
  commands. This is invariant I4.
- **Resolution is by identifier only.** Falling back to fuzzy title search in a
  bulk operation reintroduces silent mis-binding — the same failure class §5
  eliminates. Title-based resolution is interactive-with-confirmation or absent.
- **Byte changes are reported.** The provider's BibTeX will differ from the
  user's (journal abbreviations, author formatting). Import summarizes what was
  resolved and what changed, rather than silently rewriting the bibliography.

### Entries no provider has

An entry becomes a **local-provider record** (§4), retaining the user's BibTeX
verbatim, only when every applicable provider reports either **not found** or
**unsupported locator**. Import is therefore lossless for records that are
authoritatively absent from the provider roster, and the summary distinguishes
what was resolved from what was retained locally.

A retrieval or mapping error is not absence. Authentication failures, timeouts,
rate limits after bounded retries, malformed responses, and other provider
failures leave that entry unresolved and are reported as failures. They never
silently convert a refreshable work into a local record. Other entries in the
same batch may still succeed, subject to the atomic-write rule in §10.

Rejecting such entries outright is not viable. A rendered bibliography is
consumed by LaTeX and must be *complete*; a completeness hole forces the user to
maintain a second file by hand, at which point bibi is no longer managing their
bibliography.

Keeping them **outside** the manifest — in a hand-authored file concatenated into
the rendered output — was considered and rejected. It breaks I6 outright:
rendering would depend on a third input that is neither the manifest nor an
option. It also admits two correctness failures the tool cannot see, because
entries it never reads cannot participate in its checks:

- A hand-written entry may define a citation key that collides with a managed
  one, producing a bibliography in which the LaTeX toolchain silently keeps one
  entry and discards the other.
- A hand-written entry duplicating a managed record escapes deduplication, and
  the same work is cited twice under two keys.

Admitting these records to the manifest puts them inside every check that
protects the rest.

---

## 7. Documents

**Decided.** V1 retrieves only arXiv-derived documents: a PDF or source package
fetched from the record's canonical arXiv identifier. User-provided attachments
are deferred (§12).

Downloaded PDFs and extracted sources live in a single user-level cache shared
by all projects, rather than inside each project. The cache is addressed by the
normalized arXiv identifier and artifact kind. Two projects citing the same
paper therefore share one copy, and a cached file is meaningful on its own —
inspectable, collectable, and reconstructible from its name without the manifest
that requested it.

There is no record-to-document mapping. `fetch` computes the requested cache key
from the selected record's arXiv identifier every time. Nothing in the manifest
refers to a cache path or cache entry, and no absolute path is ever written into
a tracked file. Rename, removal, and provider migration require no document
bookkeeping.

The cache is purely derived and disposable. Removing a record does not delete a
cache entry, because another project may use the same entry and there is no
global project registry or reference count. Cache eviction is an explicit global
operation (`cache clean`) independent of any manifest. Extraction of source
archives is bounded and refuses unsafe archives. Downloads are bounded while
they are received: PDFs at 256 MiB and compressed source packages at 64 MiB, so
a misleading or absent `Content-Length` cannot turn a fetch into unbounded
memory growth.

**Deferred — integrity tracking.** A "lockfile" was proposed. The manifest
already pins exact metadata, so there is no *resolution* to lock; the only thing
a second file would add is **integrity**: digests of downloaded artifacts, so
that a collaborator's fetch is verifiable and reproducible.

Not built for v1. If it returns it should be named for what it is and scoped to
documents, and it carries a coupling worth remembering: digests of arXiv
artifacts require pinning *versioned* identifiers, which contradicts storing
versionless ones (§3). Deferring the feature defers that conflict; taking it up
reopens the normalization rules.

---

## 8. Scope

**Decided: project-scoped, with a global manifest available on request.**

The alternative was a user-scoped default — one personal library, projects as
views over it. That is a different product, closer to a conventional reference
manager: rendering a bibliography becomes a query over the library, and
reproducing it from a repository alone stops being possible. Project scope is
what makes a bibliography reproducible from the repository that cites it, and is
the tool's differentiator against existing reference managers.

### Resolving the target

A manifest is named `bibi.toml`. Every command resolves exactly one target, by
the first rule that applies:

1. **`-p/--path <file>`** — an explicit manifest path.
2. **`-g/--global`** — the user-level manifest at a fixed location.
3. **`./bibi.toml`** in the working directory.

`-p` and `-g` are mutually exclusive. They are two spellings of one thing —
target selection — so the exclusion is structural rather than a special case.

**There is no upward search and no fallback.** The target is always evident from
where the command was run. This is a deliberate refusal of the parent-directory
walk that comparable tools perform, and it removes an entire class of questions
along with it: where a walk stops, which ancestor wins, and whether a command run
deep inside an unrelated repository quietly targets that repository's root.
Knowing which directory one is in is the user's responsibility, and the rule is
simple enough that it can be.

**A missing manifest is created by commands that write records.** Requiring an
`init` before a first `add` is the friction that motivates user-scoped designs,
and creating on demand removes it more directly than a fallback would: the record
lands in the directory the user is standing in, rather than in a global store
they cannot see. `init` remains available for creating a manifest without adding
anything, but nothing depends on it.

**Commands that only read do not create.** An empty listing conjured from a new
manifest hides the actual answer, which is that the working directory has no
project. Reads report the absence instead.

### The global manifest is an ordinary project

**Decided.** It is a manifest like any other, at a fixed path, and `-g` is sugar
for typing that path. It has no special relationship with project manifests, and
records do not flow between them.

Making it a *library* — one that projects draw records from, so that "I already
have this paper" avoids a second resolution — is a coherent extension,
deliberately left for later. A future copy operation can match records through
their canonical identifiers and preserve the bibi id and local key. The 128-bit
bibi id is large enough for copied records to retain identity across manifests
without a central allocator. Nothing here forecloses the extension.

### Two consequences worth stating

- **Paths resolve relative to the manifest's directory, not the working
  directory.** With `-p other/bibi.toml`, a rendered bibliography is written
  beside that manifest. Either rule is defensible; leaving it unstated means it
  gets decided by accident.
- **The document cache is global regardless of target** (§7). `-g` selects a
  manifest, never a cache. Every project and the global manifest share one blob
  store.

---

## 9. Grouping

**Decided by §8: there is no grouping feature.**

Under project scope a group is a directory, and a directory is addressed by
`-p`. Cross-membership is impossible by construction, so there is nothing to
decide and nothing to build. Tags on records were the user-scoped answer, and
user scope was not taken.

The previous design carried a registry mapping stable names to project
directories, with validation preventing those paths from escaping a root,
overlapping, nesting, or aliasing through symlinks. That machinery bought the
ability to say `--shelf paper` instead of naming a path. With `-p` accepting a
manifest path directly, it is not worth its weight.

Declining grouping also removes the storage question it would have raised. Tags
imply one large store to partition, and a large store invites a database or
dataframe backend — which §11 rejects, and which project-scoped manifests never
need in the first place.

**If naming another project's path proves annoying in practice**, the cheap
remedy is a name-to-path alias in user configuration, so that `-p @thesis`
resolves to a manifest path. That is deliberately not the registry: no ownership
semantics, no validation, no per-group commands, and nothing in the data model.
It is shell shorthand living in configuration, which is why it cannot grow into
a scope system.

---

## 10. Command-line contract

### Output

**Decided. stdout carries the command's result in its most pipeable form; stderr
carries everything meant for a human.**

For record-oriented commands, the result is BibTeX — `add` appends, `rename`
shows the re-keyed entry, and `remove` emits what it deleted, which makes the
removed entry available as recovery input. Re-adding it may resolve the provider
again and is not promised to reproduce the previous record byte-for-byte. An
`export --provider` owns its composed operation's output contract: preliminary
sync changes are described on stderr rather than emitted as separate BibTeX on
stdout. For `fetch` the result is a path, so
`open $(bibi fetch <selector>)` works. Warnings, counts, progress, and retry
notices are stderr. A skipped or duplicate entry still emits its BibTeX, with the
explanation on stderr.

### Selection

**Decided.** Records are selected by local key, DOI, arXiv identifier, or
qualified provider identifier (§3). The bibi id is internal and is not an
ordinary selector. Filters on listings are a separate, best-effort facility and
are not selection: a filter that misses returns a poor result, whereas a selector
that binds the wrong record corrupts data.

### Commands

| Command | Purpose | Principal flags |
| --- | --- | --- |
| `add <locator>…` | Resolve and store | `--key`, `--provider`, `--overwrite`, `--dry-run` |
| `add -f <file>` | Resolve each entry; definitively absent entries become local (§6) | `--provider`, `--force-local`, `--overwrite` |
| `remove <selector>…` | Delete records, emitting them | `--dry-run` |
| `rename <selector> <key>` | Change a local citation key (I4) | |
| `list` | Filtered listing | `--format {table,bibtex,json}`, `--fields <field>…`, `--provider`, `--author`, `--title`, `--year`, `--local` |
| `show <selector>` | Emit the stored record as locally keyed BibTeX | |
| `sync` | Conditional or forced refresh (§5) | `--provider <name>`, `--force`, `--dry-run` |
| `export` | Optionally sync one provider, then render | `--provider <name>`, `--force`, `--output <path>`, filters |
| `check <bibfile>` | Verify a rendered bibliography byte-for-byte | export filters and rendering options |
| `fetch <selector>` | Retrieve PDF or source (§7) | `--source`, `--url`, `-o`, `--force` |
| `cache clean` | Delete the global derived document cache | `--dry-run`, `--all` |
| `init` | Create a project | |

Every project command accepts `-p/--path` or `-g/--global` to select its target
manifest (§8). `cache clean` is global derived-state maintenance and selects no
manifest. Scope is an argument, not a command level: there is no parallel command
tree to keep in step, and new project commands are scope-aware by construction.

### Notes on individual commands

**`add` is the only ingestion verb.** Importing a `.bib` file is `add -f`: each
entry resolves to a managed record where possible and becomes a local record
where not (§6). A single hand-written entry is a one-entry file, so there is no
separate import path and no second rule for when the local escape hatch applies.

**`list --format json` is the query projection.** It loads and validates the
selected manifest, applies the ordinary list filters, and emits a JSON array to
stdout without writing any file. Records are ordered by local key, optional
values are present as `null`, and the output ends with one newline. Each record
contains:

- `id`, `key`, `provider`, `provider_id`, and `revision`;
- `doi` and `arxiv`;
- `title`, `authors`, `collaborations`, and `year`.

`id`, `key`, `provider`, and `title` are strings. `provider_id`, `revision`,
`doi`, and `arxiv` are strings or `null`; opaque provider values are serialized
as strings rather than inferred as numbers. `authors` and `collaborations` are
arrays of strings, and `year` is an integer or `null`.

The stored BibTeX payload is deliberately absent. `show`, `list --format bibtex`,
and `export` are the BibTeX views; the JSON view exists for metadata filtering,
shell tooling, and future query engines.

The JSON is generated fresh from the in-memory manifest on every invocation. It
is never stored, cached, read back, or included in an atomic mutation. A user who
wants a file can redirect stdout, but that file is ordinary output and bibi does
not maintain it. The JSON field set and types form a stable schema-1 CLI
contract.

**`rename` exists because I4 requires it.** Local keys are the user's and are
renameable. No other bibi-managed state refers to a record (I5), so renaming
requires no internal bookkeeping; updating `\cite{}` uses is the reason rename is
always explicit. `add --key` sets the key at ingest instead.

**Bulk operations are partial; writes are not.** Resolving fifty of two hundred
entries and then hitting a network error must not discard the forty-nine that
succeeded. I8 constrains the *write*, not the batch: a command resolves
everything it was asked to, reports each failure on stderr, and then commits the
successes in a single atomic write. Nothing partially written, nothing needlessly
thrown away. If any requested item fails, the command exits nonzero after
committing the validated successes.

A *skipped* item is not a failure. A duplicate that was refused, or an entry
skipped for a key collision, leaves the requested state already true, so it is
reported and exits zero.

**A duplicate is refused unless `--overwrite` is given.** Deduplication keys off
DOI, arXiv id, and provider id (§3). A record matching an existing one is skipped
with its BibTeX on stdout and the explanation on stderr.

`--overwrite` replaces the existing record's provenance, identifiers,
description, and payload while preserving its bibi id (I5) and local key (I4).
That is precisely the operation §4 calls provider migration and §5 names as the
recovery path for a provider id that stopped resolving — one mechanism with three
uses, not three features. The fetched candidate must match exactly one existing
record by a deduplication identifier, or `--key` must name the existing local key
explicitly; zero or multiple possible targets are errors.

Citing a work twice in a document is not a reason to store it twice; that is the
LaTeX toolchain's concern, and one record serves any number of `\cite{}` calls.

**`show` emits BibTeX.** It structurally re-keys the stored payload to the local
key and writes that one standalone entry to stdout. It performs no network
request. Live provider responses are diagnostics for provider-specific tooling,
not an alternate meaning of `show`.

**`export` is the only way a bibliography is written.** bibi does not keep a
`.bib` continuously in step with the manifest. Mutations write the manifest and
nothing else; the bibliography is rendered when the user asks for it, to
`references.bib` by default or to `--output`.

Plain `export` is offline. `export --provider <name>` first runs the equivalent
of `sync --provider <name>` for records already owned by that provider, then
renders the complete selected bibliography from the resulting manifest.
`--provider` does not filter the export. With `--force`, the preliminary sync is
forced exactly as if `sync --provider <name> --force` had been invoked; `--force`
without `--provider` is a usage error.

If the preliminary sync reports any failure, its validated successful updates
are committed under the ordinary partial-batch rule, the command exits nonzero,
and no bibliography is written. If sync succeeds, the manifest is committed
atomically before the bibliography is rendered and atomically written. Rendering
itself remains the pure operation described by I6.

The rendered file is not continuously maintained and is never read as input.
If the user commits it for collaborators who do not have bibi, it can nonetheless
become stale relative to the manifest. `check <bibfile>` covers that boundary
without turning the file into maintained state: it renders the expected bytes
with the same options as `export`, compares them byte-for-byte, exits zero on a
match and nonzero on drift, and never writes either file. A bibliography that
does not exist is reported as missing rather than as drift. Diagnostics and an
optional diff go to stderr. `check` accepts only rendering and filtering options;
it never accepts `--provider` or `--force` and never performs a sync.

It also removes the reason the previous design split rendering across two
commands. That split protected a canonical file which had to be a function of the
manifest and nothing else, so the command writing it could not be allowed
options. A bibliography the user explicitly asks for, at a path they choose, may
be a function of the manifest *and* its options — it is output, not a maintained
artifact. Field-level policy for a journal's submission requirements can
therefore be added to `export` whenever a concrete need appears, with no safety
argument standing against it.

**`export` and `list --format bibtex` are one renderer with two entry points.**
`export` is the build step: a conventional default path, no shell redirection.
`list` is for inspection and filtering, and pipes. `check` uses the same renderer
and the same rendering-options type; it does not carry a second implementation
of the rules.

`export` refuses any output path that resolves to the selected `bibi.toml`.
Options cannot overwrite the authoritative input, including through a symlink or
an alternate spelling of the same path.

**Document cache refresh is explicit.** `fetch` returns a cache hit by default;
`fetch --force` redownloads the selected PDF or source and atomically replaces
that cache entry. Sync, including `sync --force`, never mutates the document
cache. `cache clean --dry-run` previews global eviction, while
`cache clean --all` performs it.

**What happens to the rendered file afterwards is not bibi's concern.**
bibi manages the manifest. Whether the output is committed, ignored, or deleted
after every build is entirely the user's business. bibi neither tracks it nor
rewrites it except through an explicit `export`; `check` only verifies it. There
is a real trade — committing it lets a coauthor without bibi compile the
document, ignoring it makes bibi a build dependency — but it is the user's trade
to make, not a decision this design takes. bibi performs no version-control
operations of its own (§1).

---

## 11. Rejected alternatives

Recorded so they are not reopened without new information.

| Alternative | Why rejected |
| --- | --- |
| Generate BibTeX from the structured record instead of storing the provider's | Provider BibTeX encodes journal abbreviation conventions, collaboration author formatting, and eprint layout that publishers expect. Authoring it is worse than the manipulation the design already forbids. This also rules out a structured input mode for local records, which would leave bibi with no BibTeX to store but its own. |
| Never read provider BibTeX at all | A syntactic read is unavoidable while re-keying entries and while extracting identifiers on import. The achievable and sufficient rule is that BibTeX is never *interpreted* as a metadata source (I3). |
| Reject unresolvable entries outright | A rendered bibliography must be complete for LaTeX; a completeness hole forces the user to maintain a second file and defeats the tool. Solved by the local provider (§4). |
| Keep unresolvable entries in a passthrough file concatenated into the output | Breaks I6 — rendering would depend on a third input that is neither the manifest nor an option. Entries the tool never reads also escape its checks: key collisions silently truncate the bibliography, and duplicates of managed records evade deduplication. See §6. |
| Store the derived fields *and* the provider's structured record | Restores self-verification at the cost of encoding a provider's schema in the manifest and doubling its size. The manifest is a manifest, not a database. |
| Retain unmapped provider fields as an opaque blob | The same alternative under another name. Adds diff churn from volatile fields, forfeits byte-stability unless bibi owns a canonicalization spec, and has a different shape per provider. Bibliographic content is already in the payload. See §3. |
| Expose a provider's own response shape through the provider contract | Untyped, it couples every caller to one provider's schema; typed per provider, it breaks the dynamic dispatch multiple providers require. Providers are still split into a retrieval half and a pure mapping half, both callable — the contract is simply over their composition. See §4. |
| Using provider id as bibi record identity | Provider ids may change or be replaced during provider migration, and no provider id is total across local records. A bibi-generated immutable 128-bit id gives the record a fixed identity independent of provenance. See §3. |
| Addressing the document cache by record identity | Fragments a cache shared across projects — the same paper is stored once per manifest — and makes a cached file unreadable without the manifest that named it. The cache is addressed by arXiv identifier and artifact kind. See §7. |
| Reference counting cached documents across projects | A count or registry of active project paths goes stale whenever a project is moved, deleted, or unmounted. Removing a record does not evict its derived cache entry; explicit global cache cleanup needs no project bookkeeping. See §7. |
| Generating a uniform local citation key | Users commonly want a provider's established texkeys, especially INSPIRE's. bibi therefore adopts the source BibTeX key at ingestion and preserves it thereafter; `--key` and `rename` remain explicit overrides. See §3. |
| UUIDs as ordinary citation keys | UUIDs are suitable for invisible bibi record identity, but unpleasant to type, read in source, and review in diffs. The user-facing local key comes from the source BibTeX or an explicit `--key`. See §3. |
| Automatically re-resolving a record whose provider id stopped resolving | Silent rebinding inside a bulk operation, the same failure class §6 refuses for title matching: wrong bindings surface late and corrupt a deliverable. The refresh is a no-op with a warning, and recovery explicitly names a provider through `--provider` or a qualified locator. See §5. |
| Guessing a provider for an ambiguous bare provider id | A wrong guess permanently selects that provider's BibTeX. Provider-specific ids are qualified unless their syntax identifies exactly one provider; `--provider` is the explicit alternative. See §4. |
| Falling back after a provider error | A temporary timeout, authentication failure, rate-limit exhaustion, or mapping error must not silently select another provider's BibTeX and texkey. bibi stops, reports the provider, and asks the user to retry or select another provider manually. See §4. |
| Treating provider failure as absence during import | A timeout, authentication failure, exhausted rate limit, or malformed response says nothing about whether a work exists. Only not-found and unsupported-locator outcomes permit local fallback. See §6. |
| Requiring a separate sync before every fresh export | It is easy to forget and leaves a stale tracked bibliography. Plain export remains offline, while explicit `export --provider` composes provider sync and deterministic rendering, and refuses to write output if sync reports a failure. See §10. |
| JSON as the authoritative manifest solely for querying | Escaped multiline BibTeX produces poor diffs, while moving BibTeX to a global UUID-keyed store would make export depend on ambient machine state and let branches overwrite one another's payload. The self-contained Git-friendly TOML manifest remains authoritative; `list --format json` generates a metadata-only projection on demand. |
| Migrating cita manifests | bibi is a clean break with no users to carry forward. It starts with its own schema 1 and does not read `cita.toml` or `cita-library.toml`. |
| Upward search for a manifest in parent directories | Buys the ability to run commands from a subdirectory, at the cost of an entire class of questions — where the walk stops, which ancestor wins, whether a command deep inside an unrelated repository targets that repository's root. The accepted trade is that a manifest is created in the working directory instead, and knowing which directory one is in is the user's responsibility. See §8. |
| A database or dataframe backend for the manifest | Would contradict §8's reviewability argument, which is the whole case for project scope: a binary store has no diffs and no hand-resolvable merge conflicts, and is not byte-stable for the same logical content. It also answers a problem that has not been measured at this scale — a few hundred records per project, ten thousand at the outside for a personal library, where lookup is an in-memory index built at load. Query-engine integration is deferred rather than built speculatively. |
| A registry mapping stable names to project directories | Existed to allow `--shelf paper` in place of a path, and required validation against escaping a root, overlapping, nesting, and symlink aliasing. `-p` accepting a manifest path directly makes it unnecessary. See §9. |
| Bulk BibTeX fetch with *unverified* key-based pairing | The savings are real and are taken (§5), but pairing entries to records by texkey without checking the join admits a silent, late-discovered mispairing. bibi batches and then verifies: an unmatched entry or a texkey claimed twice fails the batch whole, while a record that received no entry is reported as missing. |
| Fetching BibTeX one record at a time | The simpler rule — the request URL is the join key, so ambiguity is never created rather than checked — and it was the earlier decision here. Under a published request budget it puts a floor under every operation proportional to records touched: minutes for a large import or `sync --force`, against seconds when batched, on exactly the operations where a user is waiting. See §5. |
| Cross-checking derived fields against the provider's BibTeX | Requires parsing the rendering that I3 declares unreliable, and adjudicating disagreements that are legitimate and routine. Also guards a mispairing hazard that fetching BibTeX by record id has already eliminated. Structural validation of the response is retained and is a different thing. See §3. |

---

## 12. Open questions, collected

None blocking. Every question this document opened has been resolved or
deliberately deferred:

- **Document integrity tracking** (§7) — deferred from v1. Taking it up
  reopens versioned-versus-versionless identifier normalization (§3), so it
  should be decided before that is frozen, not after.
- **User-provided attachments** (§7) — deferred from v1. Adding them creates the
  first record-to-external-state relation. The fixed bibi id is available as its
  record key, but association storage, copying, lifetime, and deletion semantics
  remain to be designed.
- **Providers beyond the first three** (§4) — the roster is intended, not fixed.
  Each addition is weighed on BibTeX quality first and coverage second.
- **DuckDB or another query engine** (§10) — deferred from v1. V1 exposes the
  stable, metadata-only `list --format json` projection and carries no DuckDB
  dependency, SQL command, persisted JSON cache, or derived database. A future
  query engine should consume that projection and be justified by measured need.
- **A user-level library that projects draw records from** (§8) — coherent,
  additive, and left for later. Records can be copied by canonical identifier
  while preserving their fixed bibi ids.
