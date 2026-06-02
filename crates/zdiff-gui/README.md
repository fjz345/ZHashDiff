# ZDiff-GUI

![alt text](img/showcase.png)

GUI for zdiff crate

# Featuers
* Myers diffing
    - Linear version
    - Linear MT version
* Keybindings
    - Customizable
    - P4 integrated commands
* Powerful diffing view
    - Next conflict
    - Search for text
    - Goto line
    - Line Pivot
    - Lexer modes
    - P4 depot paths
    - Diff Options
        + Ignore whitespace
        + Highlight rows that differ
        + Inline ghost tokens
        + Syntax highlight (only hardcoded keywords for now)
        + Diff only

## Known bugs
* lost_focus not called correctly on paths when holding down mouse: https://github.com/emilk/egui/issues/2142

## Todo
* Split logic to avoid invalidate whole diff_ctx when doing DiffRow manipulations

* Add platform image for .exe
* show time it took to compute the diff

* Batch DiffRow performance optimzation
* Quick Diff /w multiple p4 repositories

* Syncronized scroll bar (like p4)
* user defined comparison per file format
* better horizontal scrolling