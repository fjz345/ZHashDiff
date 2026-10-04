//! Which viewer shows a diff. One module per viewer kind; Text is the existing diff pane.

pub mod hex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewerKind {
    Text,
    Hex,
}

/// Interim rule until the text-encoding-eol decoder lands: a NUL byte or invalid UTF-8 means
/// Hex. An empty file is Text.
pub fn sniff_viewer_kind(bytes: &[u8]) -> ViewerKind {
    if bytes.contains(&0) || std::str::from_utf8(bytes).is_err() {
        ViewerKind::Hex
    } else {
        ViewerKind::Text
    }
}

/// The viewer for a pair of sides, each `None` while not loaded. A pair that is not both Text
/// opens in Hex, which can show any content.
pub fn resolve_viewer_kind(
    file_1: Option<ViewerKind>,
    file_2: Option<ViewerKind>,
) -> Option<ViewerKind> {
    match (file_1, file_2) {
        (None, None) => None,
        (Some(ViewerKind::Hex), _) | (_, Some(ViewerKind::Hex)) => Some(ViewerKind::Hex),
        _ => Some(ViewerKind::Text),
    }
}

/// Stops for the shared conflict cursor: the shown viewer's. The other viewer's state is stale.
pub fn conflict_count(kind: Option<ViewerKind>, text: usize, hex: usize) -> usize {
    match kind {
        Some(ViewerKind::Hex) => hex,
        Some(ViewerKind::Text) | None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflict_navigation_counts_the_shown_viewers_stops() {
        // The conflict shortcuts drive one shared cursor; its max must come from the viewer on
        // screen, because the other viewer's state is stale.
        assert_eq!(conflict_count(Some(ViewerKind::Text), 7, 3), 7);
        assert_eq!(conflict_count(Some(ViewerKind::Hex), 7, 3), 3);
        assert_eq!(conflict_count(None, 7, 3), 7);
    }
}
