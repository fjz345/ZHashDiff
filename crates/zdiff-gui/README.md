# ZHashDiff-GUI

![alt text](img/showcase.png)

GUI for zdiff crate

# Featuers
* Myers diffing
* Keybindings
* Powerful diffing view
    - Next conflict
    - Search for text
    - Goto line
    - Line Pivot
    - Lexer modes
    - Diff Options
        + Ignore whitespace
        + Highlight rows that differ
        + Inline ghost tokens
        + Syntax highlight (only hardcoded keywords for now)

## Todo
* Fix memory consumption for big diffs ('binary search' and/or maybe start new diff when at the diagonal?)

* P4 integration for paths
* Quick diff tool

* user defined comparison per file format
* better horizontal scrolling
* fix update_diff_ctx interupt
* show time it took to compute the diff