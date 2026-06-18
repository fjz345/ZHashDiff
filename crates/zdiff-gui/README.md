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
* Better handling of temp paths (example: [p4] Zdiff.exe %s %s)
    - Need to be able to use QuickDiffs after opening a file via p4 diff
* Selecting word in diff highlights all usages of that text in both diffs
* Fix multi line text selection
* Fix MyersLinear to behave the same as MyersDebug, it uses the wrong path

* p4 feature to quick diff towards the current local file
* Feature to click "collapsed rows" to expand them
* Add platform image for .exe
* show time it took to compute the diff

* Batch DiffRow performance optimzation
* Quick Diff /w multiple p4 repositories

* Research row independant diffing if it is possible
* Color/style customization
* Syncronized scroll bar (like p4)
* user defined comparison per file format
* better horizontal scrolling
* word wrapping