# Layout proposal: superseded

Status: superseded by the global personal shelf store.

The earlier discussion proposed hiding shelf projects under a `.cita/` directory
inside each library root. The adopted design removes project and ancestor
discovery entirely: one global store owns every authoritative shelf, while
BibTeX files are materialized only through `cita export`.

See `docs/CONTEXT.md` for the authoritative model and `README.md` for the public
CLI and migration instructions.
