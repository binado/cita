# TODOS

1) using `add -p <path>` on a file that does not exist errors out instead of creating the file.
2) The output of `add` could be the just printing the generated bibtex of all the entries, separated by a newline. This is more useful than the current visual feedback. Warnings and errors can be printed to stderr. In this case, `skipped` entries can just print the bibtex again and post a warning to stderr
3) I think that the output of import could just print all bibtex entries (e.g. the new file) to stdout
4) sync command takes an extremely long time. We should benchmark what is going on
