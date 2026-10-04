# ZDiff

Library diffing file contents.

../zdiff-gui/README.md builds on this

# Featuers
* Myers diffing
* Powerful diffing view

## Todo
* Propper state based lexer for parsing scopes (// COMMENT & /* COMMENT\n*/)
* Line diff -> inner line diff

* Regex filter text contents
* image file diff

* hex diff/binary diff

## Benchmark
```
cargo bench -p zdiff --bench pipeline
```
Runs the whole pipeline on each fixture and prints one row per fixture: the median time in ms of each stage (lex, line, token, ir, rows; line and token are the two phases of `myers_diff_path`) over 7 iterations after 1 warm-up. `total` is the median of the per-iteration totals, not the sum of the stage medians. The bench profile inherits `[profile.release]` (`opt-level = "z"`, LTO), so the numbers are for the shipped size-optimized build.

* Fixtures are generated with fixed seeds: 20k C-like lines against a copy with ~2% each of deleted, inserted and modified lines; the same 20k lines against themselves; two unrelated 2k-line files (the Myers worst case). Pairs from the gitignored `test/` dir are added when present and skipped silently otherwise.
* Settings match zdiff-gui's defaults: greedy lexer, `Linear` Myers, default `DiffBuilderOptions`. `rows` is `build_diff_rows` only; zdiff-gui's IR clone and row finalization are not included.
* `-/+ tokens` and `n rows` describe the fixture. An identical pair must show `0/0`.
* `cargo test` doesn't build it, and with `--benches`/`--all-targets` the binary skips itself (cargo passes `--bench` only under `cargo bench`).

## Great blog post
https://blog.jcoglan.com/2017/02/12/the-myers-diff-algorithm-part-1/