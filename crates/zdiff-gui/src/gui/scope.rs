//! Brace-depth scope detection for expanding a collapsed block up to the scope a change lives in.
//!
//! Not language aware: braces inside strings, chars and comments count like any other brace.

/// Index into `hidden` of the line that opens the nearest `{` scope enclosing the change below a
/// collapsed block. `hidden` is the block's lines; `below` is the shown lines between the block
/// and the first changed line, which itself isn't included.
///
/// The walk goes upward from the line above the change. Scopes opened in `below` are already
/// visible, so the walk steps past them to the next enclosing one. `None` when no unmatched `{`
/// is hidden: the enclosing scope opens above the block, or the braces don't balance.
pub fn scope_opening<S: AsRef<str>>(hidden: &[S], below: &[S]) -> Option<usize> {
    // Closing braces seen so far that still wait for their opening brace.
    let mut depth = 0usize;
    // Right to left, so `} else {` opens a scope rather than closing one.
    let mut opens_scope = |line: &str| {
        let mut opens = false;
        for c in line.chars().rev() {
            match c {
                '}' => depth += 1,
                '{' if depth == 0 => opens = true,
                '{' => depth -= 1,
                _ => {}
            }
        }
        opens
    };
    for line in below.iter().rev() {
        opens_scope(line.as_ref());
    }
    (0..hidden.len())
        .rev()
        .find(|&i| opens_scope(hidden[i].as_ref()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_opening_line_of_the_enclosing_scope_is_found() {
        let hidden = ["use x;", "", "fn changed() {", "    let a = 1;"];
        assert_eq!(scope_opening(&hidden, &["    let b = 2;"]), Some(2));
    }

    #[test]
    fn nested_scopes_closed_before_the_change_are_skipped() {
        let hidden = [
            "fn outer() {",
            "    if a {",
            "        for x in y { z(); }",
            "        b();",
            "    }",
            "    {",
            "    }",
        ];
        assert_eq!(scope_opening(&hidden, &["    c();"]), Some(0));
    }

    #[test]
    fn the_nearest_unmatched_opening_wins_over_outer_ones() {
        let hidden = ["impl A {", "    fn b() {", "        c();"];
        assert_eq!(scope_opening(&hidden, &["        d();"]), Some(1));
    }

    #[test]
    fn an_opening_on_a_line_that_also_closes_counts() {
        let hidden = ["    if a {", "        b();", "    } else {", "        c();"];
        assert_eq!(scope_opening(&hidden, &[] as &[&str]), Some(2));
    }

    #[test]
    fn scopes_opened_in_the_shown_lines_below_are_stepped_past() {
        let hidden = ["fn f() {", "    a();"];
        let below = ["    if b {", "        c();"];
        assert_eq!(scope_opening(&hidden, &below), Some(0));
    }

    #[test]
    fn braces_closed_in_the_shown_lines_below_are_matched_in_the_block() {
        let hidden = ["fn f() {", "    if b {", "        c();"];
        let below = ["    }", "    d();"];
        assert_eq!(scope_opening(&hidden, &below), Some(0));
    }

    #[test]
    fn no_braces_means_no_scope() {
        let hidden = ["keep_6", "keep_7"];
        assert_eq!(scope_opening(&hidden, &["keep_16"]), None);
    }

    #[test]
    fn a_scope_opening_above_the_block_is_not_found() {
        // The block sits inside a function whose signature is above it.
        let hidden = ["    a();", "    { b(); }", "    c();"];
        assert_eq!(scope_opening(&hidden, &["    d();"]), None);
    }

    #[test]
    fn unbalanced_closing_braces_hide_the_scope() {
        // Two closes, one open: the opening line is matched by a stray `}`.
        let hidden = ["fn f() {", "    a();", "}", "}"];
        assert_eq!(scope_opening(&hidden, &["b();"]), None);
    }

    #[test]
    fn an_unbalanced_opening_brace_is_taken_as_the_scope() {
        let hidden = ["fn f() {", "    let v = vec![{", "    a();"];
        assert_eq!(scope_opening(&hidden, &["b();"]), Some(1));
    }

    /// Known limitation, pinned: braces in strings and comments are counted.
    #[test]
    fn braces_in_strings_and_comments_count_as_scopes() {
        let hidden = ["fn f() {", "    let s = \"{\";", "    a();"];
        assert_eq!(scope_opening(&hidden, &["b();"]), Some(1));

        let hidden = ["fn f() {", "    // }", "    a();"];
        assert_eq!(scope_opening(&hidden, &["b();"]), None);
    }
}
