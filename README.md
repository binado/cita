# Cita

Cita is a small, Git-friendly bibliography database. Version 0 resolves
literature through INSPIRE, stores citation metadata in `cita.toml`, and
exports deterministic BibTeX.

```console
cargo install --path crates/cita
cita init
cita add 1207.7214 doi:10.1016/j.physletb.2012.08.020
cita list
cita export --bibtex > references.bib
cita commit
```

The workspace also contains three library crates: `cita-core`
(provider-neutral models and locators), `cita-manifest` (the `cita.toml`
storage engine and BibTeX export), and `cita-inspire-client` (an INSPIRE
metadata provider built on a reusable async INSPIRE literature API client).
