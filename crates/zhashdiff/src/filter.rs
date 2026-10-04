/// Name filter for folder trees. Patterns match an entry's name only, never its path: `*` is any
/// run of characters, `?` one character, case-insensitive, and a trailing `/` limits a pattern to
/// folders. Pruning subtrees and keeping the root is up to the caller, which walks the tree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PathFilter {
    #[cfg_attr(feature = "serde", serde(default))]
    pub blacklist: PatternList,
    #[cfg_attr(feature = "serde", serde(default))]
    pub whitelist: PatternList,
}

impl PathFilter {
    /// True when the filter can hide anything.
    pub fn is_active(&self) -> bool {
        !self.blacklist.is_empty() || !self.whitelist.is_empty()
    }

    pub fn is_blacklisted(&self, name: &str, is_dir: bool) -> bool {
        self.blacklist.matches(name, is_dir)
    }

    /// True when the entry survives on its own: not blacklisted, and for a file, whitelisted or
    /// no whitelist set. The whitelist applies to files only, so a folders-only pattern in it
    /// keeps nothing. A kept folder is still hidden when a whitelist is set and nothing below it
    /// survives; that is up to the caller too.
    pub fn keeps(&self, name: &str, is_dir: bool) -> bool {
        !self.is_blacklisted(name, is_dir)
            && (is_dir || self.whitelist.is_empty() || self.whitelist.matches(name, false))
    }
}

/// Patterns separated by commas or newlines, parsed once at construction. Equality and
/// persistence go by `text` alone.
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(from = "String", into = "String"))]
pub struct PatternList {
    text: String,
    patterns: Vec<Pattern>,
}

#[derive(Debug, Clone)]
struct Pattern {
    chars: Vec<char>,
    dir_only: bool,
}

impl PatternList {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let patterns = text
            .split([',', '\n'])
            .filter_map(|entry| {
                let entry = entry.trim();
                let (entry, dir_only) = match entry.strip_suffix('/') {
                    Some(entry) => (entry.trim_end(), true),
                    None => (entry, false),
                };
                (!entry.is_empty()).then(|| Pattern {
                    chars: entry.chars().collect(),
                    dir_only,
                })
            })
            .collect();
        Self { text, patterns }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// True when there is no pattern, so nothing matches.
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    pub fn matches(&self, name: &str, is_dir: bool) -> bool {
        if self.is_empty() {
            return false;
        }
        let name: Vec<char> = name.chars().collect();
        self.patterns
            .iter()
            .any(|p| (is_dir || !p.dir_only) && wildcard_match(&p.chars, &name))
    }
}

impl PartialEq for PatternList {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
    }
}

impl Eq for PatternList {}

impl From<String> for PatternList {
    fn from(text: String) -> Self {
        Self::new(text)
    }
}

impl From<PatternList> for String {
    fn from(list: PatternList) -> Self {
        list.text
    }
}

/// Greedy match that only ever backtracks to the last `*`: O(pattern x name), so no pattern can
/// hang it.
fn wildcard_match(pattern: &[char], name: &[char]) -> bool {
    let (mut p, mut n) = (0, 0);
    // Pattern index after the last `*`, and the name index that `*` currently extends to.
    let mut last_star: Option<(usize, usize)> = None;
    while n < name.len() {
        match pattern.get(p) {
            Some('*') => {
                last_star = Some((p + 1, n));
                p += 1;
            }
            Some(&c) if c == '?' || eq_ignore_case(c, name[n]) => {
                p += 1;
                n += 1;
            }
            _ => match last_star {
                Some((after_star, star_end)) => {
                    last_star = Some((after_star, star_end + 1));
                    p = after_star;
                    n = star_end + 1;
                }
                None => return false,
            },
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}

// Per character, so `?` still consumes exactly one character of the name.
fn eq_ignore_case(a: char, b: char) -> bool {
    a == b || a.to_lowercase().eq(b.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::{PathFilter, PatternList};
    use std::time::{Duration, Instant};

    fn blacklist(text: &str) -> PathFilter {
        PathFilter {
            blacklist: PatternList::new(text),
            ..Default::default()
        }
    }

    #[test]
    fn star_matches_any_run_and_question_mark_one_character() {
        let filter = blacklist("*.obj, ?.rs, a*b*c");
        for name in [
            "x.obj", ".obj", "a.b.obj", "a.rs", "é.rs", "abc", "aXbYc", "abbc",
        ] {
            assert!(filter.is_blacklisted(name, false), "{name}");
        }
        for name in ["x.objx", "obj", "ab.rs", ".rs", "ab", "acb", "abcd"] {
            assert!(!filter.is_blacklisted(name, false), "{name}");
        }
    }

    #[test]
    fn a_pattern_without_wildcards_matches_the_whole_name_only() {
        let filter = blacklist("target");
        assert!(filter.is_blacklisted("target", true));
        assert!(!filter.is_blacklisted("target2", true));
        assert!(!filter.is_blacklisted("my_target", true));
    }

    #[test]
    fn a_trailing_slash_matches_folders_only() {
        let filter = blacklist("bin/");
        assert!(filter.is_blacklisted("bin", true));
        assert!(!filter.is_blacklisted("bin", false));

        let filter = blacklist("bin");
        assert!(filter.is_blacklisted("bin", true));
        assert!(filter.is_blacklisted("bin", false));
    }

    #[test]
    fn matching_ignores_case() {
        let filter = blacklist("*.PNG, Target, ÉCOLE");
        assert!(filter.is_blacklisted("a.png", false));
        assert!(filter.is_blacklisted("A.Png", false));
        assert!(filter.is_blacklisted("TARGET", true));
        assert!(filter.is_blacklisted("école", true));
    }

    #[test]
    fn patterns_split_on_commas_and_newlines_trimmed_with_blanks_dropped() {
        let filter = blacklist(" a.txt ,b.txt\n\n  c.txt\r\n , ,\nbin /");
        for name in ["a.txt", "b.txt", "c.txt"] {
            assert!(filter.is_blacklisted(name, false), "{name}");
        }
        assert!(filter.is_blacklisted("bin", true));
        assert!(!filter.is_blacklisted("bin", false));
        assert!(!filter.is_blacklisted("", false));
        assert!(!filter.is_blacklisted(" ", false));

        let blanks = blacklist(" , ,\n\r\n /,");
        assert!(!blanks.is_active());
        assert!(!blanks.is_blacklisted("", true));
        assert!(!blanks.is_blacklisted("a", true));
    }

    #[test]
    fn an_empty_filter_hides_nothing() {
        let filter = PathFilter::default();
        assert!(!filter.is_active());
        for (name, is_dir) in [("a.txt", false), (".git", true), ("*", false)] {
            assert!(!filter.is_blacklisted(name, is_dir), "{name}");
        }
        assert!(blacklist("*.obj").is_active());
    }

    fn whitelist(white: &str, black: &str) -> PathFilter {
        PathFilter {
            blacklist: PatternList::new(black),
            whitelist: PatternList::new(white),
        }
    }

    #[test]
    fn a_whitelisted_file_is_kept_and_a_non_matching_file_is_not() {
        let filter = whitelist("*.rs, *.toml, bin/", "");
        assert!(filter.is_active());
        assert!(filter.keeps("main.rs", false));
        assert!(filter.keeps("Cargo.TOML", false));
        assert!(!filter.keeps("readme.md", false));
        // Files only: a folders-only pattern keeps no file, and folders are kept on their own.
        assert!(!filter.keeps("bin", false));
        assert!(filter.keeps("src", true));
        assert!(filter.keeps("docs", true));
    }

    #[test]
    fn the_blacklist_beats_the_whitelist() {
        let filter = whitelist("*.rs", "gen_*");
        assert!(!filter.keeps("gen_a.rs", false));
        assert!(!filter.keeps("gen_dir", true));
        assert!(filter.keeps("a.rs", false));
    }

    #[test]
    fn an_empty_whitelist_keeps_everything_not_blacklisted() {
        let filter = whitelist(" , \n", "*.obj");
        assert!(filter.keeps("readme.md", false));
        assert!(filter.keeps("src", true));
        assert!(!filter.keeps("a.obj", false));
        assert!(!whitelist(" , \n", "").is_active());
        assert!(PathFilter::default().keeps("anything", false));
    }

    #[test]
    fn many_stars_against_a_long_name_finish_quickly() {
        // A backtracking matcher tries every split of the name between the stars here.
        let pattern = format!("{}b", "*a".repeat(50));
        let filter = blacklist(&pattern);
        let name = "a".repeat(2000);
        let start = Instant::now();
        assert!(!filter.is_blacklisted(&name, false));
        assert!(blacklist(&"*".repeat(50)).is_blacklisted(&name, false));
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "{:?}",
            start.elapsed()
        );
    }
}
