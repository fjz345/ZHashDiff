# ZDiff-GUI

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
    - P4 Integration
    - Diff Options
        + Ignore whitespace
        + Highlight rows that differ
        + Inline ghost tokens
        + Syntax highlight (only hardcoded keywords for now)

## Known bugs
* lost_focus not called correctly on paths when holding down mouse: https://github.com/emilk/egui/issues/2142

## Todo
* Fix .p4config, .env code

* Add platform image for .exe

* Batch DiffRow performance optimzation
* Show diffs only Diff Option
* Quick Diff /w multiple p4 repositories

* Syncronized scroll bar (like p4)
* user defined comparison per file format
* better horizontal scrolling
* show time it took to compute the diff