# bibi-documents

Validated arXiv PDF URLs and downloads, safe source-package extraction, and
atomic local artifact caching for [bibi](https://github.com/binado/bibi).

`DocumentStore::fetch` remains the PDF-compatible entry point.
`DocumentStore::fetch_artifact` additionally supports gzip-compressed arXiv
source packages, which are validated and extracted into a temporary sibling
directory before publication.

This crate is published for reuse by the bibi workspace. Its public API is
unstable while the version is 0.x and may change between minor releases.
