# bibi

bibi maintains a bibliography as a manifest and renders a BibTeX file from it on
demand. Schema-1 `bibi.toml` is the only project state bibi maintains;
`references.bib` is explicit output, written by `bibi export` and never read
back.

Bibliographic metadata is owned by providers. You choose which references to
cite and what to call them; titles, authors, journals, identifiers, and the
BibTeX bytes are fetched and refreshed rather than authored. Where no provider
holds a work, you supply its BibTeX once and bibi stores it verbatim.

**Status: under construction.** bibi replaces `cita` as a clean break, with no
migration path from any cita schema. See [REDESIGN.md](REDESIGN.md) for the
design and [IMPLEMENTATION.md](IMPLEMENTATION.md) for the crate-level contract.
This file is rewritten with usage documentation once the command set lands.

## License

MIT. See [LICENSE](LICENSE).
