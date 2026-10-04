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
* `build_diff_rows` builds large diffs in up to 8 parallel chunks on rayon's pool. Run with `RAYON_NUM_THREADS=1` for the single-core cost.
* `-/+ tokens` and `n rows` describe the fixture. An identical pair must show `0/0`.
* `cargo test` doesn't build it, and with `--benches`/`--all-targets` the binary skips itself (cargo passes `--bench` only under `cargo bench`).

## Design note: phases and hunk independence
Research only (diff-engine issue 09); it describes the engine as of row chunking (issue 08). Times are one `cargo bench` run (ms, median of 7) on a 32-thread 7950X3D.

### Phases
| Phase | Code | Hands to the next phase |
|---|---|---|
| Document | `CachedFile::new`, `Lexer::parse` | `CachedFile`: `contents`, `tokens` (kind + byte span), `metadata.line_starts`, blake3 `hash` |
| Ignore mask (side input, not a phase) | `IgnoreOptions::mask` | `IgnoreMask`: one ignore flag per token per side, plus the regex-matched (dimmed) flags. Built twice per diff: Myers thread and rows thread |
| Scope (not built) | - | would hand paired token ranges to the line phase |
| Line | `myers::line_diff` | `Vec<LineHunk>`: token ranges of each run of unequal lines. Everything outside them pairs line for line |
| Token | `myers::token_diff` | `MyersPath`: unit-step path over both whole files. Myers per hunk, `push_equal_lines` between hunks |
| IR | `DiffIR::new` | `DiffIR.entries`: one `DiffResult` per token, absolute `u32` token indices |
| Rows | `build_diff_rows` | `Vec<DiffRow>`: per-side token lists (copies of the `DiffResult`s) and absolute `line_num` |
| Finalize (zdiff-gui) | `finalize_diff_rows`, `precompute_file_rows` | pivot shift, `DiffSpan`s, diff-only collapse, line-to-row maps |

`myers_diff_path` is line then token. The scope phase would slot in between document and line: it pairs scopes of the two sides and hands each pair's token ranges to the line phase, with unpaired scopes as inserts and deletes. `line_diff` and `token_diff` work on slices and return local results (as `token_diff` already does per hunk), so the line phase can run per scope pair and the paths concatenate with offsets. Pairing scopes is its own matching problem (for example Myers over scope header keys), and the ignore options must apply to those keys too. The brace-depth scope detector of file-view-ux issue 03 is the obvious seed.

### Units
Three units, easy to mix up. Only the last is user-visible.
* `LineHunk`: engine-internal. It only decides where token Myers runs.
* Seam: an Equal entry whose source token is a Newline. `DiffBuilder::handle_match` flushes both sides there, so only the line counters cross it. Every hunk after an equal line starts right after a seam, and seams also occur inside hunks. Rows are independent per seam, not per hunk.
* `DiffSpan`: a run of rows with a visible edit, computed over the final rows (conflict navigation, revert). Not 1:1 with hunks: a hunk with only hidden (ignored) edits forms no span.

### 1. Partial invalidation when one side changes locally: no
There is no editor, so a side changes by reload: a revert, its undo/redo, or an external edit. Today the new `CachedFile` hash changes `MyersCtxInput` and every stage reruns from lexing. Per phase:
* Lex: conditional. Only `BlockComment` state crosses a line: string literals end at the line (a backslash doesn't escape a line break) and a line comment ends at it. Relexing from the edited line resyncs at the first line start where the new state equals the old one; after that the tokens are the same with shifted spans and indices. An unclosed `/*` pushes the resync to EOF. The comment mask's block tracking follows the same rule; whitespace and regex flags are per line.
* Line: conditional, and the real blocker. Myers is a global optimum. Re-diffing only the window between the nearest unchanged equal lines around the edit gives a valid script, but not always the one a full run gives (repeated lines can pair differently). Every verification so far compares against a full recompute, so this would have to accept a different valid result.
* Token: yes. Each hunk is diffed from its own slices (`align_runs_to_line_ends` too), and equal regions are local, so unchanged hunks keep their paths.
* IR and rows: conditional. `DiffResult` holds absolute token indices, and rows copy them and hold absolute line numbers, so everything after the edit needs an O(n) fix-up pass, or a format change to relative indices.
* zdiff-gui: stage inputs are whole values (`MyersCtxInput` by file hash, `DiffIRInput` by the whole path, `DiffRowsInput` by the whole IR). A partial update needs the old outputs plus the edited range, not just a new key.

Cost of the full rerun it would save: `generated/scattered_20k` (20k lines, ~2% changed) is lex 50.0, line 38.7, token 2.6, ir 2.6, rows 4.6, total 99.3. That is about 0.1 s on background threads, after an action that already writes or reads the file.

### 2. Parallel row building: yes (built in issue 08)
* Ghost carry-over: ghosts live in the builder's side buffers, which a seam flushes. A non-last chunk asserts it ends clean, so ghosts never cross a chunk.
* Line numbering: the only state across seams. Each chunk's line count is added to the next chunks' `line_num > 0`. It is also known without building: at a seam both sides are at a line start, so the offset is the line of the next token (`line_starts`).
* Constraint, no seam means no split: a pair whose line endings differ on every line (CRLF vs LF) has no Equal Newline. Without ignore-whitespace the whole file is one hunk; with it, every break is a hidden Delete + Insert. `chunk_ranges` returns one range and the rows build on one thread.
* The token phase is just as independent per hunk and could run on rayon with offset concatenation, with no format change. It doesn't pay on the fixtures: token is 2.6 ms on `scattered_20k`, and the 504 ms of `local/extreme_size_log` is a single hunk covering 97,648 of its 97,758 tokens. That case needs a faster diff of one large hunk, not hunk parallelism.

### 3. Lazy building of off-screen rows: conditional, not without a row-format change
* Row content is local (seams), but row indices aren't. A hunk's row count (ghost-only rows, one-sided Void rows) is only known after building it, so every later row index depends on every earlier hunk. Equal regions give one row per line only when their line breaks pair as Equal: with mismatched line endings and ignore-whitespace, each line pair takes two staggered rows.
* zdiff-gui consumes the whole list: the table's row count, `precompute_file_rows` (goto, find, pivot), `precompute_diff_spans` (conflict navigation, revert), the pivot shift of one side, diff-only collapse, and copy/selection across rows. Planned: file-view-ux issue 02 keeps collapsed rows for expansion, issue 05 keeps per-row wrap heights.
* Workable shape: build hunks eagerly (cost proportional to the change), keep equal regions as (source line, target line, count) ranges that materialize on demand, and get row indices from a prefix sum of region row counts. That is a new row representation, which the diff-engine PRD puts out of scope ("Changing the diff IR or row format").
* Gain: rows are 4.6 ms on `scattered_20k`, plus 2 to 4 ms to drop them. Lazy rows would mostly save memory: one token Vec per side per row, 16 MB on `scattered_20k`.

### Recommendation: don't pursue hunk independence further now
* Parallel rows: done.
* Partial invalidation: no. Global Myers (results differ from a full run), absolute indices in the IR and rows, and whole-value stage keys all block it, for a rare trigger whose full rerun takes about 0.1 s at 20k lines.
* Lazy rows: not now. They need a row-format change.

The time is in lexing and the line phase (50 + 39 of 99 ms on `scattered_20k`) and in one large hunk's token Myers (504 of 540 ms on `extreme_size_log`). Hunk independence doesn't split either. If performance work continues after issue 10 sets the targets, profile the lexer and the line phase first.

If lazy rows are wanted later (most likely together with file-view-ux 02's expansion or 05's wrap heights), the first step is a new PRD for a region-based row list: hunks as built rows, equal regions as line ranges, row index by prefix sum. Measure row memory and drop cost on the 20k fixtures first to justify it.

## Great blog post
https://blog.jcoglan.com/2017/02/12/the-myers-diff-algorithm-part-1/