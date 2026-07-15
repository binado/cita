# PaperDB

PaperDB is a small, Git-friendly bibliography database. Version 0 resolves
literature through INSPIRE, stores citation metadata in `paperdb.toml`, and
exports deterministic BibTeX.

```console
cargo install --path crates/paperdb
paperdb init
paperdb add 1207.7214 doi:10.1016/j.physletb.2012.08.020
paperdb list
paperdb export --bibtex > references.bib
paperdb commit
```

The workspace also contains `paperdb-core` and the independently reusable
`paperdb-inspire-client` library.
