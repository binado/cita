# bibi-manifest

Typed schema-1 persistence for independent bibi projects and
`cita-library.toml` registries. It validates shelf identities and coordinated
`cita.toml`/`references.bib` writes, plus deterministic library serialization,
safe relative shelf paths (rejecting escapes, overlaps, and symlink aliases),
and non-overlapping registrations.

Schema-1 manifest storage, identity validation, deterministic bibliography
generation, and coordinated mutations for
[bibi](https://github.com/binado/bibi).

This crate is published for reuse by the bibi workspace. Its public API is
unstable while the version is 0.x and may change between minor releases.
