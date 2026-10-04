use std::{
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::{
    ignore::IgnoreMask,
    lexer::{RawTokenTrait, TokenKind},
};

pub type MyersPath = Vec<(i32, i32)>;
pub type MyersNumAddDelete = (u32, u32);

#[derive(Debug, Clone, Copy, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum MyersDiffAlgorithm {
    Trace, // N+M^2 memory
    #[default]
    Linear, // N+M memory
    LinearMT, // N+M memory with multi-threading
}

/// Token-level edit path of `source` -> `target`: `line_diff`, then `token_diff`. A unit-step
/// path from (0, 0) to (source.len(), target.len()). None when cancelled.
pub fn myers_diff_path<T, F>(
    algorithm: MyersDiffAlgorithm,
    source: &[T],
    target: &[T],
    cmp: F,
    ignore: &IgnoreMask,
    cancel_flag: Arc<AtomicBool>,
) -> Option<MyersPath>
where
    T: RawTokenTrait,
    F: Fn(&T, &T) -> bool + Sync,
{
    let hunks = line_diff(algorithm, source, target, &cmp, ignore, cancel_flag.clone())?;
    token_diff(algorithm, source, target, &hunks, &cmp, ignore, cancel_flag)
}

/// A run of non-equal lines, as token ranges. Either range may be empty (pure insert/delete).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineHunk {
    pub source: Range<usize>,
    pub target: Range<usize>,
}

/// Line phase: Myers over lines, where two lines are equal when their tokens are pairwise equal
/// under `cmp`. Returns the hunks in order; tokens outside them pair up one to one.
pub fn line_diff<T, F>(
    algorithm: MyersDiffAlgorithm,
    source: &[T],
    target: &[T],
    cmp: F,
    ignore: &IgnoreMask,
    cancel_flag: Arc<AtomicBool>,
) -> Option<Vec<LineHunk>>
where
    T: RawTokenTrait,
    F: Fn(&T, &T) -> bool + Sync,
{
    if cancel_flag.load(Ordering::Relaxed) {
        return None;
    }
    assert_mask_fits(ignore, source, target);
    if source.len() == target.len() && source.iter().zip(target).all(|(a, b)| cmp(a, b)) {
        return Some(Vec::new());
    }

    let (source_lines, target_lines) = (split_lines(source), split_lines(target));
    let line_eq = |a: &Range<usize>, b: &Range<usize>| {
        if ignore.is_empty() {
            return a.len() == b.len()
                && source[a.clone()]
                    .iter()
                    .zip(&target[b.clone()])
                    .all(|(a, b)| cmp(a, b));
        }
        // The line key: the tokens that aren't ignored.
        let mut a_key = a.clone().filter(|&i| !ignore.source[i]);
        let mut b_key = b.clone().filter(|&i| !ignore.target[i]);
        loop {
            match (a_key.next(), b_key.next()) {
                (None, None) => return true,
                (Some(i), Some(j)) if cmp(&source[i], &target[j]) => {}
                _ => return false,
            }
        }
    };
    let path = raw_diff_path(
        algorithm,
        &source_lines,
        &target_lines,
        line_eq,
        cancel_flag.clone(),
    )?;
    // The inner algorithms don't check the flag on every path (Trace's backtrack skips it for a
    // one-row trace, Linear's midpoint search gives up silently), so check it here.
    if cancel_flag.load(Ordering::Relaxed) {
        return None;
    }

    let token_start = |lines: &[Range<usize>], line: usize, num_tokens: usize| {
        lines.get(line).map_or(num_tokens, |l| l.start)
    };
    let mut hunks = Vec::new();
    let (mut x, mut y) = (0, 0);
    let mut open: Option<(usize, usize)> = None;
    for step in path_steps(&path).into_iter().chain([Step::Equal]) {
        if step != Step::Equal {
            open.get_or_insert((x, y));
        } else if let Some((x0, y0)) = open.take() {
            hunks.push(LineHunk {
                source: token_start(&source_lines, x0, source.len())
                    ..token_start(&source_lines, x, source.len()),
                target: token_start(&target_lines, y0, target.len())
                    ..token_start(&target_lines, y, target.len()),
            });
        }
        match step {
            Step::Equal => (x, y) = (x + 1, y + 1),
            Step::Delete => x += 1,
            Step::Insert => y += 1,
        }
    }
    Some(hunks)
}

/// Token phase: Myers over each hunk's tokens, Equal for every token outside the hunks.
/// Returns a unit-step path over the whole files.
pub fn token_diff<T, F>(
    algorithm: MyersDiffAlgorithm,
    source: &[T],
    target: &[T],
    hunks: &[LineHunk],
    cmp: F,
    ignore: &IgnoreMask,
    cancel_flag: Arc<AtomicBool>,
) -> Option<MyersPath>
where
    T: RawTokenTrait,
    F: Fn(&T, &T) -> bool + Sync,
{
    if cancel_flag.load(Ordering::Relaxed) {
        return None;
    }
    assert_mask_fits(ignore, source, target);
    let mut path = Vec::with_capacity(source.len() + target.len() + 1);
    path.push((0, 0));
    let (mut x, mut y) = (0, 0);

    for hunk in hunks {
        if cancel_flag.load(Ordering::Relaxed) {
            return None;
        }
        let to = (hunk.source.start, hunk.target.start);
        push_equal_lines(&mut path, source, target, (x, y), to, &cmp, ignore);
        (x, y) = (hunk.source.start, hunk.target.start);

        let (s, t) = (&source[hunk.source.clone()], &target[hunk.target.clone()]);
        if s.is_empty() || t.is_empty() {
            path.extend((1..=s.len()).map(|i| ((x + i) as i32, y as i32)));
            path.extend((1..=t.len()).map(|i| (x as i32, (y + i) as i32)));
        } else {
            let local = raw_diff_path(algorithm, s, t, &cmp, cancel_flag.clone())?;
            if cancel_flag.load(Ordering::Relaxed) {
                return None;
            }
            let is_line_end = |t: &T| t.as_ref().kind == TokenKind::Newline;
            let local = align_runs_to_line_ends(&local, s, t, &cmp, is_line_end);
            path.extend(
                local[1..]
                    .iter()
                    .map(|&(lx, ly)| (x as i32 + lx, y as i32 + ly)),
            );
        }
        (x, y) = (hunk.source.end, hunk.target.end);
    }
    let to = (source.len(), target.len());
    push_equal_lines(&mut path, source, target, (x, y), to, &cmp, ignore);
    Some(path)
}

fn assert_mask_fits<T>(ignore: &IgnoreMask, source: &[T], target: &[T]) {
    if !ignore.is_empty() {
        assert_eq!(
            ignore.source.len(),
            source.len(),
            "one flag per source token"
        );
        assert_eq!(
            ignore.target.len(),
            target.len(),
            "one flag per target token"
        );
    }
}

/// The tokens of equal lines between hunks, from `from` to `to`. With nothing ignored they pair
/// one to one. Otherwise equal lines can differ in ignored tokens, so each line pair is aligned
/// on its key tokens instead, and only ignored tokens become edits.
fn push_equal_lines<T, F>(
    path: &mut MyersPath,
    source: &[T],
    target: &[T],
    from: (usize, usize),
    to: (usize, usize),
    cmp: &F,
    ignore: &IgnoreMask,
) where
    T: RawTokenTrait,
    F: Fn(&T, &T) -> bool,
{
    if ignore.is_empty() {
        push_equal_run(path, from, to);
        return;
    }
    let source_lines = split_lines(&source[from.0..to.0]);
    let target_lines = split_lines(&target[from.1..to.1]);
    assert_eq!(
        source_lines.len(),
        target_lines.len(),
        "equal lines pair one to one"
    );
    for (s, t) in source_lines.into_iter().zip(target_lines) {
        let (mut x, mut y) = (from.0 + s.start, from.1 + t.start);
        let (x_end, y_end) = (from.0 + s.end, from.1 + t.end);
        loop {
            let key_x = (x..x_end).find(|&i| !ignore.source[i]).unwrap_or(x_end);
            let key_y = (y..y_end).find(|&i| !ignore.target[i]).unwrap_or(y_end);
            push_ignored_gap(path, source, target, (x, y), (key_x, key_y), cmp);
            if key_x == x_end || key_y == y_end {
                assert!(
                    key_x == x_end && key_y == y_end,
                    "equal lines must have the same key"
                );
                break;
            }
            assert!(
                cmp(&source[key_x], &target[key_y]),
                "equal lines must have the same key"
            );
            path.push(((key_x + 1) as i32, (key_y + 1) as i32));
            (x, y) = (key_x + 1, key_y + 1);
        }
    }
}

/// Ignored tokens between two key tokens: an equal prefix, deletes, inserts, an equal suffix.
/// Every edit here is hidden in the rows, so a minimal diff buys nothing; deletes come first
/// like everywhere else. The suffix pairs a line's break when both sides end with the same one.
fn push_ignored_gap<T, F>(
    path: &mut MyersPath,
    source: &[T],
    target: &[T],
    from: (usize, usize),
    to: (usize, usize),
    cmp: &F,
) where
    F: Fn(&T, &T) -> bool,
{
    let (s, t) = (&source[from.0..to.0], &target[from.1..to.1]);
    let prefix = s.iter().zip(t).take_while(|(a, b)| cmp(a, b)).count();
    let suffix = s[prefix..]
        .iter()
        .rev()
        .zip(t[prefix..].iter().rev())
        .take_while(|(a, b)| cmp(a, b))
        .count();
    let (x, y) = (from.0 + prefix, from.1 + prefix);
    let (x_end, y_end) = (to.0 - suffix, to.1 - suffix);
    push_equal_run(path, from, (x, y));
    path.extend((x + 1..=x_end).map(|i| (i as i32, y as i32)));
    path.extend((y + 1..=y_end).map(|j| (x_end as i32, j as i32)));
    push_equal_run(path, (x_end, y_end), to);
}

/// Unit diagonal steps from `from` to `to`: the tokens of equal lines between hunks.
fn push_equal_run(path: &mut MyersPath, from: (usize, usize), to: (usize, usize)) {
    assert_eq!(
        to.0 - from.0,
        to.1 - from.1,
        "equal lines must have the same number of tokens on both sides"
    );
    path.extend((1..=to.0 - from.0).map(|i| ((from.0 + i) as i32, (from.1 + i) as i32)));
}

/// Each line's token range, including its line break token. A last line without a line break
/// is included; an empty file has no lines.
fn split_lines<T: RawTokenTrait>(tokens: &[T]) -> Vec<Range<usize>> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (i, token) in tokens.iter().enumerate() {
        if token.as_ref().kind == TokenKind::Newline {
            lines.push(start..i + 1);
            start = i + 1;
        }
    }
    if start < tokens.len() {
        lines.push(start..tokens.len());
    }
    lines
}

fn raw_diff_path<T, F>(
    algorithm: MyersDiffAlgorithm,
    source: &[T],
    target: &[T],
    cmp: F,
    cancel_flag: Arc<AtomicBool>,
) -> Option<MyersPath>
where
    T: Sync,
    F: Fn(&T, &T) -> bool + Sync,
{
    match algorithm {
        MyersDiffAlgorithm::Trace => {
            let trace = myers_diff_trace(source, target, &cmp);
            myers_backtrack(trace, source.len() as i32, target.len() as i32, cancel_flag)
        }
        MyersDiffAlgorithm::Linear => myers_diff_linear(source, target, &cmp, cancel_flag),
        MyersDiffAlgorithm::LinearMT => myers_diff_linear_mt(source, target, &cmp, cancel_flag),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Equal,
    Delete,
    Insert,
}

/// Unit steps of a path, with the same window semantics as DiffIR::generate_ir: edits first,
/// then the snake.
fn path_steps(path: &[(i32, i32)]) -> Vec<Step> {
    let mut steps = Vec::with_capacity(path.len());
    for w in path.windows(2) {
        let (dx, dy) = (w[1].0 - w[0].0, w[1].1 - w[0].1);
        let edit = if dx > dy { Step::Delete } else { Step::Insert };
        steps.extend(std::iter::repeat_n(edit, (dx - dy).unsigned_abs() as usize));
        steps.extend(std::iter::repeat_n(Step::Equal, dx.min(dy) as usize));
    }
    steps
}

/// How far a run may slide; bounds the cost on long runs of identical tokens.
const MAX_SLIDE: usize = 256;

/// Moves each pure insert or delete run to the equivalent position (same cost) where its last
/// token ends a line. Token-level Myers often places a whole-line edit across a line break
/// (`a [+\n +x] \n` instead of `a \n [+x +\n]`), which pairs the wrong lines in the rows.
/// Runs with no such position stay put. Returns a unit-step path.
pub fn align_runs_to_line_ends<T, F, L>(
    path: &[(i32, i32)],
    source: &[T],
    target: &[T],
    cmp: F,
    is_line_end: L,
) -> MyersPath
where
    F: Fn(&T, &T) -> bool,
    L: Fn(&T) -> bool,
{
    let Some(&start) = path.first() else {
        return Vec::new();
    };

    let mut steps = path_steps(path);
    // Interleaved edits (`+a -b +c`) leave no run next to the following equals, so put each
    // block's deletes first. Same cost and the same equal pairs.
    for block in steps.split_mut(|s| *s == Step::Equal) {
        let deletes = block.iter().filter(|s| **s == Step::Delete).count();
        block[..deletes].fill(Step::Delete);
        block[deletes..].fill(Step::Insert);
    }

    let (mut x, mut y) = (start.0 as usize, start.1 as usize);
    let mut i = 0;
    while i < steps.len() {
        let edit = steps[i];
        if edit == Step::Equal {
            (x, y) = (x + 1, y + 1);
            i += 1;
            continue;
        }
        let end = i + steps[i..].iter().take_while(|s| **s == edit).count();
        let n = end - i;

        // Sliding the run's edits from `split` on forward by one pairs the token at the run's
        // other-side cursor with the token at `split` for either run kind.
        let forward_from = |split: usize| {
            (0..MAX_SLIDE)
                .take_while(|&j| {
                    steps.get(end + j) == Some(&Step::Equal)
                        && match edit {
                            Step::Insert => cmp(&source[x + j], &target[y + split + j]),
                            _ => cmp(&source[x + split + j], &target[y + j]),
                        }
                })
                .count()
        };
        let backward = (1..=MAX_SLIDE.min(i))
            .take_while(|&j| {
                steps[i - j] == Step::Equal
                    && match edit {
                        Step::Insert => cmp(&source[x - j], &target[y + n - j]),
                        _ => cmp(&source[x + n - j], &target[y - j]),
                    }
            })
            .count();
        let ends_line = |d: isize| {
            let last = ((if edit == Step::Insert { y } else { x }) + n) as isize + d - 1;
            let tokens = if edit == Step::Insert { target } else { source };
            is_line_end(&tokens[last as usize])
        };
        let mut split = 0;
        let mut shift = (-(backward as isize)..=forward_from(0) as isize)
            .rev()
            .find(|&d| ends_line(d));
        if shift.is_none() {
            // The run can't move as a whole, but its tail may: in `-2 +20 +; +\n +new +( +) =; =\n`
            // keeping `+20` in place and sliding the rest gives `-2 +20 =; =\n +new +( +) +; +\n`.
            (split, shift) = (1..n.min(MAX_SLIDE))
                .find_map(|k| {
                    let d = (1..=forward_from(k) as isize)
                        .rev()
                        .find(|&d| ends_line(d))?;
                    Some((k, Some(d)))
                })
                .unwrap_or((0, None));
        }
        let shift = shift.unwrap_or(0);

        // The shifted region holds the same steps, so the cursors past it are unchanged.
        if shift > 0 {
            let d = shift as usize;
            steps[i + split..i + split + d].fill(Step::Equal);
            steps[i + split + d..end + d].fill(edit);
        } else if shift < 0 {
            let d = (-shift) as usize;
            steps[i - d..end - d].fill(edit);
            steps[end - d..end].fill(Step::Equal);
        }
        let equals = shift.max(0) as usize;
        match edit {
            Step::Insert => (x, y) = (x + equals, y + n + equals),
            _ => (x, y) = (x + n + equals, y + equals),
        }
        i = end + equals;
    }

    let mut out = Vec::with_capacity(steps.len() + 1);
    let (mut x, mut y) = start;
    out.push((x, y));
    for step in steps {
        match step {
            Step::Equal => (x, y) = (x + 1, y + 1),
            Step::Delete => x += 1,
            Step::Insert => y += 1,
        }
        out.push((x, y));
    }
    out
}

pub struct MyersTrace {
    data: Vec<i32>,
    num_rows: usize,
}

impl MyersTrace {
    fn new(edit_capacity: usize) -> Self {
        Self {
            data: Vec::with_capacity(edit_capacity * 64),
            num_rows: 0,
        }
    }

    fn push(&mut self, row: &[i32]) {
        self.data.extend_from_slice(row);
        self.num_rows += 1;
    }

    pub fn len(&self) -> usize {
        self.num_rows
    }

    pub fn shortest_edit(&self) -> usize {
        if self.num_rows == 0 {
            0
        } else {
            self.num_rows - 1
        }
    }
}

impl std::ops::Index<usize> for MyersTrace {
    type Output = [i32];

    fn index(&self, d: usize) -> &Self::Output {
        let start = d * d;
        let end = (d + 1) * (d + 1);
        &self.data[start..end]
    }
}

/*
returns the path of lowest cost (edits) to get from source to target
Uses (N+M)^2 memory
*/
pub fn myers_diff_trace<T, F>(source: &[T], target: &[T], mut cmp: F) -> MyersTrace
where
    F: FnMut(&T, &T) -> bool,
{
    let source_len = source.len() as i32;
    let target_len = target.len() as i32;
    let max = source_len + target_len;

    let mut furthest_x_for_k = vec![0; (2 * max + 2) as usize];
    let mut trace = MyersTrace::new(max as usize + 1);

    let offset = max as usize;
    furthest_x_for_k[offset] = 0;

    for depth in 0..=max {
        for k in (-depth..=depth).step_by(2) {
            let v_index = (k + max) as usize;

            let mut x = if k == -depth
                || (k != depth && furthest_x_for_k[v_index - 1] < furthest_x_for_k[v_index + 1])
            {
                furthest_x_for_k[v_index + 1]
            } else {
                furthest_x_for_k[v_index - 1] + 1
            };

            let mut y = x - k;

            // Move diagonally
            while x < source_len && y < target_len && cmp(&source[x as usize], &target[y as usize])
            {
                x += 1;
                y += 1;
            }

            furthest_x_for_k[v_index] = x;

            if x >= source_len && y >= target_len {
                let start = offset - depth as usize;
                let end = offset + depth as usize;
                trace.push(&furthest_x_for_k[start..=end]);
                return trace;
            }
        }

        let start = offset - depth as usize;
        let end = offset + depth as usize;
        trace.push(&furthest_x_for_k[start..=end]);
    }

    trace
}

pub fn myers_backtrack(
    trace: MyersTrace,
    source_len: i32,
    target_len: i32,
    cancel_flag: Arc<AtomicBool>,
) -> Option<MyersPath> {
    let mut path = Vec::with_capacity((source_len + target_len) as usize + 1);
    let mut current_x = source_len;
    let mut current_y = target_len;

    // Start from the final depth and work backwards to D=1
    for (i, depth) in (1..trace.len()).rev().enumerate() {
        if i % 1000 == 0 && cancel_flag.load(Ordering::Relaxed) {
            return None;
        }

        let d_idx = depth as i32;
        let k = current_x - current_y;
        let prev_v_slice = &trace[depth - 1];
        let prev_d_idx = d_idx - 1;

        // Logic check: Did we come from the diagonal above (Down) or the diagonal to the left (Right)?
        let came_from_above = if k == -d_idx
            || (k != d_idx
                && prev_v_slice[(k + 1 + prev_d_idx) as usize]
                    > prev_v_slice[(k - 1 + prev_d_idx) as usize])
        {
            true
        } else {
            false
        };

        let k_prev = if came_from_above { k + 1 } else { k - 1 };
        let prev_v_idx = (k_prev + prev_d_idx) as usize;

        let x_before_snake = if came_from_above {
            prev_v_slice[prev_v_idx]
        } else {
            prev_v_slice[prev_v_idx] + 1
        };

        // 1. Backtrack the diagonal snake (matches)
        while current_x > x_before_snake {
            path.push((current_x, current_y));
            current_x -= 1;
            current_y -= 1;
        }

        // 2. Backtrack the single edit (Right or Down move)
        path.push((current_x, current_y));

        // 3. Update coordinates to the point *before* the edit
        current_x = prev_v_slice[(k_prev + prev_d_idx) as usize];
        current_y = current_x - k_prev;
    }

    // 4. Final step: handle the potential diagonal snake leading back to (0,0) at D=0
    while current_x > 0 || current_y > 0 {
        path.push((current_x, current_y));
        current_x -= 1;
        current_y -= 1;
    }
    path.push((0, 0));

    path.reverse();
    Some(path)
}

pub fn myers_count_add_deletes(diff_path: &[(i32, i32)]) -> MyersNumAddDelete {
    let mut adds = 0;
    let mut deletes = 0;

    for window in diff_path.windows(2) {
        let dx = window[1].0 - window[0].0;
        let dy = window[1].1 - window[0].1;

        if dx > 0 && dy == 0 {
            deletes += 1; // Horizontal = Source consumed = Deletion
        } else if dy > 0 && dx == 0 {
            adds += 1; // Vertical = Target consumed = Addition
        }
    }
    (adds, deletes)
}

#[derive(Clone, Copy, Debug)]
struct BoxRegion {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}
impl BoxRegion {
    #[inline(always)]
    fn width(&self) -> i32 {
        self.right - self.left
    }
    #[inline(always)]
    fn height(&self) -> i32 {
        self.bottom - self.top
    }
    #[inline(always)]
    fn size(&self) -> i32 {
        self.width() + self.height()
    }
    #[inline(always)]
    fn delta(&self) -> i32 {
        self.width() - self.height()
    }
}

struct SearchBuffers {
    vf: Vec<i32>,
    vb: Vec<i32>,
    offset: usize,
}

impl SearchBuffers {
    fn new(max_size: usize) -> Self {
        Self {
            vf: vec![0; 2 * max_size + 2],
            vb: vec![0; 2 * max_size + 2],
            offset: max_size,
        }
    }

    #[inline(always)]
    fn get_f(&self, k: i32) -> i32 {
        self.vf[(k as usize).wrapping_add(self.offset)]
    }
    #[inline(always)]
    fn set_f(&mut self, k: i32, val: i32) {
        self.vf[(k as usize).wrapping_add(self.offset)] = val;
    }

    #[inline(always)]
    fn get_b(&self, c: i32) -> i32 {
        self.vb[(c as usize).wrapping_add(self.offset)]
    }
    #[inline(always)]
    fn set_b(&mut self, c: i32, val: i32) {
        self.vb[(c as usize).wrapping_add(self.offset)] = val;
    }
}

fn find_midpoint<T, F>(
    box_reg: BoxRegion,
    source: &[T],
    target: &[T],
    cmp: &mut F,
    bufs: &mut SearchBuffers,
    cancel_flag: Arc<AtomicBool>,
) -> Option<((i32, i32), (i32, i32))>
where
    F: FnMut(&T, &T) -> bool,
{
    let box_size = box_reg.size();
    if box_size == 0 {
        return None;
    }

    let delta = box_reg.delta();
    bufs.set_f(1, box_reg.left);
    bufs.set_b(1, box_reg.bottom);

    let max_d = (box_size + 1) / 2;

    for d in 0..=max_d {
        if d % 1000 == 0 && cancel_flag.load(Ordering::Relaxed) {
            return None;
        }
        for k in (-d..=d).step_by(2) {
            let c = k - delta;

            let (prev_x, x) = if k == -d || (k != d && bufs.get_f(k - 1) < bufs.get_f(k + 1)) {
                let px = bufs.get_f(k + 1);
                (px, px)
            } else {
                let px = bufs.get_f(k - 1);
                (px, px + 1)
            };

            let mut current_x = x;
            let mut current_y = current_x - box_reg.left - k + box_reg.top;

            let prev_y = if d == 0 || current_x != prev_x {
                current_y
            } else {
                current_y - 1
            };

            while current_x < box_reg.right
                && current_y < box_reg.bottom
                && cmp(&source[current_x as usize], &target[current_y as usize])
            {
                current_x += 1;
                current_y += 1;
            }

            bufs.set_f(k, current_x);

            if (delta & 1) != 0 && c >= -(d - 1) && c <= d - 1 {
                if current_y >= bufs.get_b(c) {
                    return Some(((prev_x, prev_y), (current_x, current_y)));
                }
            }
        }

        for c in (-d..=d).step_by(2) {
            let k = c + delta;

            let (prev_y, y) = if c == -d || (c != d && bufs.get_b(c - 1) > bufs.get_b(c + 1)) {
                let py = bufs.get_b(c + 1);
                (py, py)
            } else {
                let py = bufs.get_b(c - 1);
                (py, py - 1)
            };

            let mut current_y = y;
            let mut current_x = current_y - box_reg.top + k + box_reg.left;

            let prev_x = if d == 0 || current_y != prev_y {
                current_x
            } else {
                current_x + 1
            };

            while current_x > box_reg.left
                && current_y > box_reg.top
                && cmp(
                    &source[(current_x - 1) as usize],
                    &target[(current_y - 1) as usize],
                )
            {
                current_x -= 1;
                current_y -= 1;
            }

            bufs.set_b(c, current_y);

            if (delta & 1) == 0 && k >= -d && k <= d {
                if current_x <= bufs.get_f(k) {
                    return Some(((current_x, current_y), (prev_x, prev_y)));
                }
            }
        }
    }

    None
}

fn find_path<T, F>(
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
    source: &[T],
    target: &[T],
    cmp: &mut F,
    bufs: &mut SearchBuffers,
    path: &mut MyersPath,
    cancel_flag: Arc<AtomicBool>,
) where
    F: FnMut(&T, &T) -> bool,
{
    let box_reg = BoxRegion {
        left,
        top,
        right,
        bottom,
    };
    if box_reg.size() == 0 {
        return;
    }

    if let Some((snake_start, snake_end)) =
        find_midpoint(box_reg, source, target, cmp, bufs, cancel_flag.clone())
    {
        find_path(
            box_reg.left,
            box_reg.top,
            snake_start.0,
            snake_start.1,
            source,
            target,
            cmp,
            bufs,
            path,
            cancel_flag.clone(),
        );

        if path.last() != Some(&snake_start) {
            path.push(snake_start);
        }
        if path.last() != Some(&snake_end) {
            path.push(snake_end);
        }

        find_path(
            snake_end.0,
            snake_end.1,
            box_reg.right,
            box_reg.bottom,
            source,
            target,
            cmp,
            bufs,
            path,
            cancel_flag,
        );
    }
}

pub fn myers_diff_linear<T, F>(
    source: &[T],
    target: &[T],
    mut cmp: F,
    cancel_flag: Arc<AtomicBool>,
) -> Option<MyersPath>
where
    F: FnMut(&T, &T) -> bool,
{
    let source_len = source.len() as i32;
    let target_len = target.len() as i32;

    if source_len == 0 && target_len == 0 {
        return Some(vec![(0, 0)]);
    }

    let mut bufs = SearchBuffers::new((source_len + target_len) as usize + 1);
    let mut points = Vec::with_capacity(((source_len + target_len) / 8) as usize);

    points.push((0, 0));
    find_path(
        0,
        0,
        source_len,
        target_len,
        source,
        target,
        &mut cmp,
        &mut bufs,
        &mut points,
        cancel_flag.clone(),
    );

    if points.last() != Some(&(source_len, target_len)) {
        points.push((source_len, target_len));
    }

    let mut path = Vec::with_capacity(points.len() * 2);
    path.push((0, 0));

    for i in 0..points.len() - 1 {
        if i % 1000 == 0 && cancel_flag.load(Ordering::Relaxed) {
            return None;
        }

        let mut x = points[i].0;
        let mut y = points[i].1;
        let next_point = points[i + 1];

        while x < next_point.0 || y < next_point.1 {
            if x < next_point.0 && y < next_point.1 && cmp(&source[x as usize], &target[y as usize])
            {
                x += 1;
                y += 1;
            } else if next_point.0 - x > next_point.1 - y {
                x += 1;
            } else {
                y += 1;
            }
            if path.last() != Some(&(x, y)) {
                path.push((x, y));
            }
        }
    }

    Some(path)
}

fn find_path_mt<T, F>(
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
    source: &[T],
    target: &[T],
    cmp: &F,
    path: &mut MyersPath,
    cancel_flag: Arc<AtomicBool>,
) where
    T: Sync,
    F: Fn(&T, &T) -> bool + Sync,
{
    let box_reg = BoxRegion {
        left,
        top,
        right,
        bottom,
    };
    let size = box_reg.size();
    if size == 0 {
        return;
    }

    // Allocate a scratch buffer local to this thread's scope frame
    let mut bufs = SearchBuffers::new(size as usize + 1);

    if let Some((snake_start, snake_end)) =
        find_midpoint_mt(box_reg, source, target, cmp, &mut bufs, cancel_flag.clone())
    {
        // Threshold optimization: Do not pay scheduling costs for tiny sub-problems
        if size > 2048 {
            let mut left_path = Vec::new();
            let mut right_path = Vec::new();

            // Execute the independent left and right bounding boxes on Rayon's thread pool
            rayon::join(
                || {
                    find_path_mt(
                        box_reg.left,
                        box_reg.top,
                        snake_start.0,
                        snake_start.1,
                        source,
                        target,
                        cmp,
                        &mut left_path,
                        cancel_flag.clone(),
                    )
                },
                || {
                    find_path_mt(
                        snake_end.0,
                        snake_end.1,
                        box_reg.right,
                        box_reg.bottom,
                        source,
                        target,
                        cmp,
                        &mut right_path,
                        cancel_flag.clone(),
                    )
                },
            );

            path.extend(left_path);
            if path.last() != Some(&snake_start) {
                path.push(snake_start);
            }
            if path.last() != Some(&snake_end) {
                path.push(snake_end);
            }
            path.extend(right_path);
        } else {
            // Fall back to sequential execution on the current thread for small segments
            find_path_mt(
                box_reg.left,
                box_reg.top,
                snake_start.0,
                snake_start.1,
                source,
                target,
                cmp,
                path,
                cancel_flag.clone(),
            );
            if path.last() != Some(&snake_start) {
                path.push(snake_start);
            }
            if path.last() != Some(&snake_end) {
                path.push(snake_end);
            }
            find_path_mt(
                snake_end.0,
                snake_end.1,
                box_reg.right,
                box_reg.bottom,
                source,
                target,
                cmp,
                path,
                cancel_flag,
            );
        }
    }
}

// Internal logic remains identical to your linear midpoint execution, adapted to immutable closure matching
fn find_midpoint_mt<T, F>(
    box_reg: BoxRegion,
    source: &[T],
    target: &[T],
    cmp: &F,
    bufs: &mut SearchBuffers,
    cancel_flag: Arc<AtomicBool>,
) -> Option<((i32, i32), (i32, i32))>
where
    F: Fn(&T, &T) -> bool,
{
    let box_size = box_reg.size();
    if box_size == 0 {
        return None;
    }

    let delta = box_reg.delta();
    bufs.set_f(1, box_reg.left);
    bufs.set_b(1, box_reg.bottom);

    let max_d = (box_size + 1) / 2;

    for d in 0..=max_d {
        if d % 1000 == 0 && cancel_flag.load(Ordering::Relaxed) {
            return None;
        }
        for k in (-d..=d).step_by(2) {
            let c = k - delta;
            let (prev_x, x) = if k == -d || (k != d && bufs.get_f(k - 1) < bufs.get_f(k + 1)) {
                let px = bufs.get_f(k + 1);
                (px, px)
            } else {
                let px = bufs.get_f(k - 1);
                (px, px + 1)
            };

            let mut current_x = x;
            let mut current_y = current_x - box_reg.left - k + box_reg.top;
            let prev_y = if d == 0 || current_x != prev_x {
                current_y
            } else {
                current_y - 1
            };

            while current_x < box_reg.right
                && current_y < box_reg.bottom
                && cmp(&source[current_x as usize], &target[current_y as usize])
            {
                current_x += 1;
                current_y += 1;
            }
            bufs.set_f(k, current_x);

            if (delta & 1) != 0 && c >= -(d - 1) && c <= d - 1 {
                if current_y >= bufs.get_b(c) {
                    return Some(((prev_x, prev_y), (current_x, current_y)));
                }
            }
        }

        for c in (-d..=d).step_by(2) {
            let k = c + delta;
            let (prev_y, y) = if c == -d || (c != d && bufs.get_b(c - 1) > bufs.get_b(c + 1)) {
                let py = bufs.get_b(c + 1);
                (py, py)
            } else {
                let py = bufs.get_b(c - 1);
                (py, py - 1)
            };

            let mut current_y = y;
            let mut current_x = current_y - box_reg.top + k + box_reg.left;
            let prev_x = if d == 0 || current_y != prev_y {
                current_x
            } else {
                current_x + 1
            };

            while current_x > box_reg.left
                && current_y > box_reg.top
                && cmp(
                    &source[(current_x - 1) as usize],
                    &target[(current_y - 1) as usize],
                )
            {
                current_x -= 1;
                current_y -= 1;
            }
            bufs.set_b(c, current_y);

            if (delta & 1) == 0 && k >= -d && k <= d {
                if current_x <= bufs.get_f(k) {
                    return Some(((current_x, current_y), (prev_x, prev_y)));
                }
            }
        }
    }
    None
}

// Requires Fn + Sync instead of FnMut so it can be safely referenced across threads
pub fn myers_diff_linear_mt<T, F>(
    source: &[T],
    target: &[T],
    cmp: F,
    cancel_flag: Arc<AtomicBool>,
) -> Option<MyersPath>
where
    T: Sync,
    F: Fn(&T, &T) -> bool + Sync,
{
    let source_len = source.len() as i32;
    let target_len = target.len() as i32;

    if source_len == 0 && target_len == 0 {
        return Some(vec![(0, 0)]);
    }

    let mut points = Vec::with_capacity(((source_len + target_len) / 8) as usize);
    points.push((0, 0));

    find_path_mt(
        0,
        0,
        source_len,
        target_len,
        source,
        target,
        &cmp,
        &mut points,
        cancel_flag.clone(),
    );

    if points.last() != Some(&(source_len, target_len)) {
        points.push((source_len, target_len));
    }

    let mut path = Vec::with_capacity(points.len() * 2);
    path.push((0, 0));

    for i in 0..points.len() - 1 {
        if i % 1000 == 0 && cancel_flag.load(Ordering::Relaxed) {
            return None;
        }

        let mut x = points[i].0;
        let mut y = points[i].1;
        let next_point = points[i + 1];

        while x < next_point.0 || y < next_point.1 {
            if x < next_point.0 && y < next_point.1 && cmp(&source[x as usize], &target[y as usize])
            {
                x += 1;
                y += 1;
            } else if next_point.0 - x > next_point.1 - y {
                x += 1;
            } else {
                y += 1;
            }
            if path.last() != Some(&(x, y)) {
                path.push((x, y));
            }
        }
    }

    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn distance_from_path(path: &[(i32, i32)]) -> usize {
        if path.is_empty() {
            return 0;
        }
        path.windows(2)
            .filter(|w| {
                let (x1, y1) = w[0];
                let (x2, y2) = w[1];
                (x1 == x2 && y1 != y2) || (x1 != x2 && y1 == y2)
            })
            .count()
    }

    #[test]
    fn test_identical_sequences() {
        let a = vec!["a", "b", "c"];
        let b = vec!["a", "b", "c"];
        let cmp = |t1: &&str, t2: &&str| t1 == t2;

        let trace = myers_diff_trace(&a, &b, cmp);
        let dist = trace.shortest_edit();
        let path = myers_backtrack(
            trace,
            a.len() as i32,
            b.len() as i32,
            Arc::new(AtomicBool::new(false)),
        )
        .expect("myers backtrack failed");

        assert_eq!(dist, 0);
        assert_eq!(distance_from_path(&path), 0);
        assert_eq!(path.len(), 4); // (0,0) -> (1,1) -> (2,2) -> (3,3)
    }

    #[test]
    fn test_completely_different() {
        let a = vec!["a", "b"];
        let b = vec!["c", "d"];
        let cmp = |t1: &&str, t2: &&str| t1 == t2;

        let trace = myers_diff_trace(&a, &b, cmp);
        let dist = trace.shortest_edit();
        assert_eq!(dist, 4); // 2 deletes, 2 inserts
    }

    #[test]
    fn test_empty_sequences() {
        let a: Vec<&str> = vec![];
        let b: Vec<&str> = vec!["a", "b"];
        let cmp = |t1: &&str, t2: &&str| t1 == t2;

        let trace = myers_diff_trace(&a, &b, cmp);
        assert_eq!(trace.shortest_edit(), 2);
        let trace = myers_diff_trace(&b, &a, cmp);
        assert_eq!(trace.shortest_edit(), 2);
        let trace = myers_diff_trace(&a, &a, cmp);
        assert_eq!(trace.shortest_edit(), 0);
    }

    #[test]
    fn test_complex_interleaving() {
        let a: Vec<char> = "ABCABBA".chars().collect();
        let b: Vec<char> = "CBABAC".chars().collect();
        let cmp = |t1: &char, t2: &char| t1 == t2;

        let trace = myers_diff_trace(&a, &b, cmp);
        let dist = trace.shortest_edit();
        let path = myers_backtrack(
            trace,
            a.len() as i32,
            b.len() as i32,
            Arc::new(AtomicBool::new(false)),
        )
        .expect("myers backtrack failed");

        assert_eq!(dist, 5);
        assert_eq!(distance_from_path(&path), 5);
    }

    #[test]
    fn test_path_continuity() {
        let a = vec!["A", "B", "C"];
        let b = vec!["A", "X", "C"];
        let cmp = |t1: &&str, t2: &&str| t1 == t2;

        let trace = myers_diff_trace(&a, &b, cmp);
        let path = myers_backtrack(
            trace,
            a.len() as i32,
            b.len() as i32,
            Arc::new(AtomicBool::new(false)),
        )
        .expect("myers backtrack failed");

        // Verify every step in the path is valid (Right, Down, or Diagonal)
        for w in path.windows(2) {
            let (x1, y1) = w[0];
            let (x2, y2) = w[1];
            let dx = x2 - x1;
            let dy = y2 - y1;

            // Valid moves: (1,0), (0,1), or (1,1)
            assert!(
                (dx == 1 && dy == 0) || (dx == 0 && dy == 1) || (dx == 1 && dy == 1),
                "Invalid path jump from ({},{}) to ({},{})",
                x1,
                y1,
                x2,
                y2
            );
        }
    }

    // LINEAR
    #[test]
    fn test_linear_identical_sequences() {
        let a = vec!["a", "b", "c"];
        let b = vec!["a", "b", "c"];
        let cmp = |t1: &&str, t2: &&str| t1 == t2;

        let path = myers_diff_linear(&a, &b, cmp, Arc::new(AtomicBool::new(false)))
            .expect("myers diff failed");

        assert_eq!(distance_from_path(&path), 0);
        assert_eq!(path.len(), 4); // (0,0) -> (1,1) -> (2,2) -> (3,3)
    }

    #[test]
    fn test_linear_completely_different() {
        let a = vec!["a", "b"];
        let b = vec!["c", "d"];
        let cmp = |t1: &&str, t2: &&str| t1 == t2;

        let path = myers_diff_linear(&a, &b, cmp, Arc::new(AtomicBool::new(false)))
            .expect("myers diff failed");
        assert_eq!(distance_from_path(&path), 4); // 2 deletes, 2 inserts
    }

    #[test]
    fn test_linear_empty_sequences() {
        let a: Vec<&str> = vec![];
        let b: Vec<&str> = vec!["a", "b"];
        let cmp = |t1: &&str, t2: &&str| t1 == t2;

        assert_eq!(
            myers_diff_linear(&a, &b, cmp, Arc::new(AtomicBool::new(false)))
                .expect("myers diff failed")
                .len()
                - 1,
            2
        );
        assert_eq!(
            myers_diff_linear(&b, &a, cmp, Arc::new(AtomicBool::new(false)))
                .expect("myers diff failed")
                .len()
                - 1,
            2
        );
        assert_eq!(
            myers_diff_linear(&a, &a, cmp, Arc::new(AtomicBool::new(false)))
                .expect("myers diff failed")
                .len()
                - 1,
            0
        );
    }

    #[test]
    fn test_linear_complex_interleaving() {
        let a: Vec<char> = "ABCABBA".chars().collect();
        let b: Vec<char> = "CBABAC".chars().collect();
        let cmp = |t1: &char, t2: &char| t1 == t2;

        let path = myers_diff_linear(&a, &b, cmp, Arc::new(AtomicBool::new(false)))
            .expect("myers diff failed");

        // assert_eq!(dist, 5);
        assert_eq!(distance_from_path(&path), 5);
    }

    #[test]
    fn test_linear_path_continuity() {
        let a = vec!["A", "B", "C"];
        let b = vec!["A", "X", "C"];
        let cmp = |t1: &&str, t2: &&str| t1 == t2;

        let path = myers_diff_linear(&a, &b, cmp, Arc::new(AtomicBool::new(false)))
            .expect("myers diff failed");

        // Verify every step in the path is valid (Right, Down, or Diagonal)
        for w in path.windows(2) {
            let (x1, y1) = w[0];
            let (x2, y2) = w[1];
            let dx = x2 - x1;
            let dy = y2 - y1;

            // Valid moves: (1,0), (0,1), or (1,1)
            assert!(
                (dx == 1 && dy == 0) || (dx == 0 && dy == 1) || (dx == 1 && dy == 1),
                "Invalid path jump from ({},{}) to ({},{})",
                x1,
                y1,
                x2,
                y2
            );
        }
    }

    // LINEAR MT
    #[test]
    fn test_linear_mt_identical_sequences() {
        let a = vec!["a", "b", "c"];
        let b = vec!["a", "b", "c"];
        let cmp = |t1: &&str, t2: &&str| t1 == t2;

        let path = myers_diff_linear_mt(&a, &b, cmp, Arc::new(AtomicBool::new(false)))
            .expect("myers diff failed");

        assert_eq!(distance_from_path(&path), 0);
        assert_eq!(path.len(), 4); // (0,0) -> (1,1) -> (2,2) -> (3,3)
    }

    #[test]
    fn test_linear_mt_completely_different() {
        let a = vec!["a", "b"];
        let b = vec!["c", "d"];
        let cmp = |t1: &&str, t2: &&str| t1 == t2;

        let path = myers_diff_linear_mt(&a, &b, cmp, Arc::new(AtomicBool::new(false)))
            .expect("myers diff failed");
        assert_eq!(distance_from_path(&path), 4); // 2 deletes, 2 inserts
    }

    #[test]
    fn test_linear_mt_empty_sequences() {
        let a: Vec<&str> = vec![];
        let b: Vec<&str> = vec!["a", "b"];
        let cmp = |t1: &&str, t2: &&str| t1 == t2;

        assert_eq!(
            myers_diff_linear_mt(&a, &b, cmp, Arc::new(AtomicBool::new(false)))
                .expect("myers diff failed")
                .len()
                - 1,
            2
        );
        assert_eq!(
            myers_diff_linear_mt(&b, &a, cmp, Arc::new(AtomicBool::new(false)))
                .expect("myers diff failed")
                .len()
                - 1,
            2
        );
        assert_eq!(
            myers_diff_linear_mt(&a, &a, cmp, Arc::new(AtomicBool::new(false)))
                .expect("myers diff failed")
                .len()
                - 1,
            0
        );
    }

    #[test]
    fn test_linear_mt_complex_interleaving() {
        let a: Vec<char> = "ABCABBA".chars().collect();
        let b: Vec<char> = "CBABAC".chars().collect();
        let cmp = |t1: &char, t2: &char| t1 == t2;

        let path = myers_diff_linear_mt(&a, &b, cmp, Arc::new(AtomicBool::new(false)))
            .expect("myers diff failed");

        // assert_eq!(dist, 5);
        assert_eq!(distance_from_path(&path), 5);
    }

    #[test]
    fn test_linear_mt_path_continuity() {
        let a = vec!["A", "B", "C"];
        let b = vec!["A", "X", "C"];
        let cmp = |t1: &&str, t2: &&str| t1 == t2;

        let path = myers_diff_linear_mt(&a, &b, cmp, Arc::new(AtomicBool::new(false)))
            .expect("myers diff failed");

        // Verify every step in the path is valid (Right, Down, or Diagonal)
        for w in path.windows(2) {
            let (x1, y1) = w[0];
            let (x2, y2) = w[1];
            let dx = x2 - x1;
            let dy = y2 - y1;

            // Valid moves: (1,0), (0,1), or (1,1)
            assert!(
                (dx == 1 && dy == 0) || (dx == 0 && dy == 1) || (dx == 1 && dy == 1),
                "Invalid path jump from ({},{}) to ({},{})",
                x1,
                y1,
                x2,
                y2
            );
        }
    }

    /// Edit script of a unit-step path: `=c` equal, `-c` delete, `+c` insert.
    fn script(path: &[(i32, i32)], source: &[char], target: &[char]) -> String {
        path.windows(2)
            .map(|w| {
                let ((x, y), (x2, y2)) = (w[0], w[1]);
                match (x2 - x, y2 - y) {
                    (1, 1) => format!("={}", source[x as usize]),
                    (1, 0) => format!("-{}", source[x as usize]),
                    (0, 1) => format!("+{}", target[y as usize]),
                    step => panic!("not a unit step: {step:?}"),
                }
            })
            .collect()
    }

    fn align(path: &[(i32, i32)], source: &str, target: &str) -> String {
        let (s, t): (Vec<char>, Vec<char>) = (source.chars().collect(), target.chars().collect());
        let aligned = align_runs_to_line_ends(path, &s, &t, |a, b| a == b, |c| *c == '\n');
        script(&aligned, &s, &t)
    }

    #[test]
    fn insert_run_straddling_a_line_break_slides_to_end_at_it() {
        // a [+\n +x] \n b \n
        let path = [(0, 0), (1, 1), (1, 2), (1, 3), (2, 4), (3, 5), (4, 6)];
        assert_eq!(align(&path, "a\nb\n", "a\nx\nb\n"), "=a=\n+x+\n=b=\n");
    }

    #[test]
    fn delete_run_straddling_a_line_break_slides_to_end_at_it() {
        // k e e p [-\n -o -l -d] \n e n d \n
        let mut path = vec![(0, 0), (1, 1), (2, 2), (3, 3), (4, 4)];
        path.extend((5..=8).map(|x| (x, 4)));
        path.extend((9..=13).map(|x| (x, x - 4)));
        assert_eq!(
            align(&path, "keep\nold\nend\n", "keep\nend\n"),
            "=k=e=e=p=\n-o-l-d-\n=e=n=d=\n"
        );
    }

    #[test]
    fn run_that_cannot_end_at_a_line_break_stays_put() {
        let path = [(0, 0), (1, 1), (1, 2), (2, 3), (3, 4)];
        assert_eq!(align(&path, "ab\n", "axb\n"), "=a+x=b=\n");
    }

    #[test]
    fn run_slides_back_when_it_cannot_slide_forward() {
        // a \n b [+\n +b] at the end of the file
        let path = [(0, 0), (1, 1), (2, 2), (3, 3), (3, 4), (3, 5)];
        assert_eq!(align(&path, "a\nb", "a\nb\nb"), "=a=\n+b+\n=b");
    }

    #[test]
    fn already_aligned_run_stays_put() {
        let path = [(0, 0), (1, 1), (2, 2), (3, 3), (4, 4), (4, 5), (4, 6)];
        assert_eq!(align(&path, "a\nx\n", "a\nx\nx\n"), "=a=\n=x=\n+x+\n");
    }

    mod line_then_token {
        use super::*;
        use crate::{
            ignore::IgnoreOptions,
            lexer::{LexerDefault, RawToken},
        };

        const ALGORITHMS: [MyersDiffAlgorithm; 3] = [
            MyersDiffAlgorithm::Trace,
            MyersDiffAlgorithm::Linear,
            MyersDiffAlgorithm::LinearMT,
        ];

        #[derive(Debug, Clone, Copy, PartialEq)]
        enum Op {
            Equal,
            Delete,
            Insert,
        }

        /// One entry per token: the op, the source and target line of the token (None on the
        /// side that doesn't have it).
        type Script = Vec<(Op, Option<usize>, Option<usize>)>;

        fn lex(text: &str) -> Vec<RawToken> {
            LexerDefault::<RawToken>::new(text).parse()
        }

        fn line_of(text: &str, token: &RawToken) -> usize {
            text[..token.span.start].matches('\n').count()
        }

        fn diff(
            algorithm: MyersDiffAlgorithm,
            source: &str,
            target: &str,
            cancel: bool,
        ) -> Option<Script> {
            diff_with(algorithm, source, target, &IgnoreOptions::default(), cancel)
        }

        fn diff_with(
            algorithm: MyersDiffAlgorithm,
            source: &str,
            target: &str,
            ignore: &IgnoreOptions,
            cancel: bool,
        ) -> Option<Script> {
            let (ts, tt) = (lex(source), lex(target));
            let cmp = |a: &RawToken, b: &RawToken| {
                a.kind == b.kind && source[a.span.clone()] == target[b.span.clone()]
            };
            let path = myers_diff_path(
                algorithm,
                &ts,
                &tt,
                cmp,
                &ignore.mask(&ts, &tt),
                Arc::new(AtomicBool::new(cancel)),
            )?;

            assert_eq!(path.first(), Some(&(0, 0)), "{algorithm:?}");
            assert_eq!(
                path.last(),
                Some(&(ts.len() as i32, tt.len() as i32)),
                "{algorithm:?}"
            );
            let mut script = Script::new();
            let (mut rebuilt_source, mut rebuilt_target) = (String::new(), String::new());
            for w in path.windows(2) {
                let ((x, y), (x2, y2)) = (w[0], w[1]);
                let (s, t) = (ts.get(x as usize), tt.get(y as usize));
                let entry = match (x2 - x, y2 - y) {
                    (1, 1) => {
                        let (s, t) = (s.unwrap(), t.unwrap());
                        assert!(cmp(s, t), "{algorithm:?}: Equal pairs unequal tokens");
                        rebuilt_source.push_str(&source[s.span.clone()]);
                        rebuilt_target.push_str(&target[t.span.clone()]);
                        (
                            Op::Equal,
                            Some(line_of(source, s)),
                            Some(line_of(target, t)),
                        )
                    }
                    (1, 0) => {
                        rebuilt_source.push_str(&source[s.unwrap().span.clone()]);
                        (Op::Delete, Some(line_of(source, s.unwrap())), None)
                    }
                    (0, 1) => {
                        rebuilt_target.push_str(&target[t.unwrap().span.clone()]);
                        (Op::Insert, None, Some(line_of(target, t.unwrap())))
                    }
                    step => panic!("{algorithm:?}: not a unit step: {step:?}"),
                };
                script.push(entry);
            }
            assert_eq!(
                rebuilt_source, source,
                "{algorithm:?}: source reconstruction"
            );
            assert_eq!(
                rebuilt_target, target,
                "{algorithm:?}: target reconstruction"
            );
            Some(script)
        }

        /// The concatenated text of the deleted and of the inserted tokens.
        fn changed_text(source: &str, target: &str, script: &Script) -> (String, String) {
            let (ts, tt) = (lex(source), lex(target));
            let (mut si, mut ti) = (0, 0);
            let (mut deleted, mut inserted) = (String::new(), String::new());
            for (op, _, _) in script {
                match op {
                    Op::Equal => (si, ti) = (si + 1, ti + 1),
                    Op::Delete => {
                        deleted.push_str(&source[ts[si].span.clone()]);
                        si += 1;
                    }
                    Op::Insert => {
                        inserted.push_str(&target[tt[ti].span.clone()]);
                        ti += 1;
                    }
                }
            }
            (deleted, inserted)
        }

        #[test]
        fn edit_script_reconstructs_both_inputs() {
            let pairs = [
                (
                    "fn main() {\n    let x = 10;\n}\n",
                    "fn main() {\n    let x = 20;\n    let y = 30;\n}\n",
                ),
                ("a\r\nb\r\nc\r\n", "a\r\nB\r\nc\r\nd"),
                ("no newline at end", "no newline\nat end\n"),
                ("line\n", "line\n\n\n"),
                ("\t\tindent\n    spaces\n", "\t\tindent;\n    spaces;\n"),
                ("/* a\n b */ x\n", "/* a\n c */ x\n// y\n"),
                ("x\ny\nx\ny\n", "y\nx\ny\nx\n"),
            ];
            for algorithm in ALGORITHMS {
                for (source, target) in pairs {
                    diff(algorithm, source, target, false).expect("not cancelled");
                    diff(algorithm, target, source, false).expect("not cancelled");
                }
            }
        }

        #[test]
        fn insert_delete_and_replace_hunks_change_only_their_lines() {
            let base = "fn f() {\n    let a = 1;\n    let b = 2;\n}\n";
            let inserted = "fn f() {\n    let a = 1;\n    call(a);\n    let b = 2;\n}\n";
            let replaced = "fn f() {\n    let a = 7;\n    let b = 2;\n}\n";
            for algorithm in ALGORITHMS {
                let script = diff(algorithm, base, inserted, false).unwrap();
                assert_eq!(
                    changed_text(base, inserted, &script),
                    (String::new(), "    call(a);\n".into()),
                    "{algorithm:?}: insert"
                );

                let script = diff(algorithm, inserted, base, false).unwrap();
                assert_eq!(
                    changed_text(inserted, base, &script),
                    ("    call(a);\n".into(), String::new()),
                    "{algorithm:?}: delete"
                );

                let script = diff(algorithm, base, replaced, false).unwrap();
                assert_eq!(
                    changed_text(base, replaced, &script),
                    ("1".into(), "7".into()),
                    "{algorithm:?}: replace"
                );
            }
        }

        #[test]
        fn line_inserted_mid_hunk_does_not_shift_later_pairings() {
            // Every line changes, so the line phase sees one hunk; the new line sits in the middle.
            let source = "let a = 1;\nlet b = 2;\nlet c = 3;\nlet d = 4;\n";
            let target = "let a = 10;\nlet b = 20;\nnew();\nlet c = 30;\nlet d = 40;\n";
            for algorithm in ALGORITHMS {
                let script = diff(algorithm, source, target, false).unwrap();
                let pairs: Vec<(usize, usize)> = script
                    .iter()
                    .filter(|(op, _, _)| *op == Op::Equal)
                    .map(|&(_, s, t)| (s.unwrap(), t.unwrap()))
                    .collect();
                assert!(
                    pairs
                        .iter()
                        .all(|&(s, t)| t == if s < 2 { s } else { s + 1 }),
                    "{algorithm:?}: {pairs:?}"
                );
                for line in 0..4 {
                    assert!(
                        pairs.iter().any(|&(s, _)| s == line),
                        "{algorithm:?}: {line}"
                    );
                }
                assert_eq!(
                    changed_text(source, target, &script),
                    ("1234".into(), "1020new();\n3040".into()),
                    "{algorithm:?}"
                );
            }
        }

        #[test]
        fn identical_files_produce_no_hunks() {
            let text = "fn main() {\r\n    // comment\r\n    let x = \"s\";\r\n}";
            for algorithm in ALGORITHMS {
                let script = diff(algorithm, text, text, false).unwrap();
                assert!(
                    script.iter().all(|(op, _, _)| *op == Op::Equal),
                    "{algorithm:?}"
                );
                assert_eq!(script.len(), lex(text).len());
                assert_eq!(diff(algorithm, "", "", false).unwrap(), Script::new());
            }
        }

        #[test]
        fn one_empty_file() {
            let text = "a\nb c\n\nd";
            for algorithm in ALGORITHMS {
                let script = diff(algorithm, "", text, false).unwrap();
                assert!(
                    script.iter().all(|(op, _, _)| *op == Op::Insert),
                    "{algorithm:?}"
                );
                let script = diff(algorithm, text, "", false).unwrap();
                assert!(
                    script.iter().all(|(op, _, _)| *op == Op::Delete),
                    "{algorithm:?}"
                );
            }
        }

        /// Deterministic generator (xorshift64*): no rand dependency.
        struct Rng(u64);

        impl Rng {
            fn below(&mut self, n: u64) -> u64 {
                self.0 ^= self.0 >> 12;
                self.0 ^= self.0 << 25;
                self.0 ^= self.0 >> 27;
                self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) % n
            }

            /// Words from a small vocabulary plus a unique id, so lines never repeat but their
            /// tokens do.
            fn line(&mut self, id: &mut usize) -> String {
                let words = ["let", "x", "y", "=", "+", "1", "2", "(", ")", ";", "{", "}"];
                let len = 1 + self.below(8);
                *id += 1;
                (0..len)
                    .map(|_| words[self.below(words.len() as u64) as usize])
                    .chain([format!("id{id}").as_str()])
                    .collect::<Vec<_>>()
                    .join(" ")
            }
        }

        #[test]
        fn large_generated_pair_marks_only_edited_lines() {
            // Tokens repeat across lines, so a global token diff may match tokens of unrelated
            // lines around an edit; the line phase must keep unedited lines equal.
            let (mut rng, mut id) = (Rng(7), 0);
            let (mut source, mut target) = (Vec::new(), Vec::new());
            let (mut edited_source, mut edited_target) = (Vec::new(), Vec::new());
            for _ in 0..1500 {
                let line = rng.line(&mut id);
                match rng.below(20) {
                    0 => {
                        edited_source.push(source.len());
                        source.push(line);
                    }
                    1 => {
                        edited_target.push(target.len());
                        target.push(rng.line(&mut id));
                        source.push(line.clone());
                        target.push(line);
                    }
                    2 => {
                        edited_source.push(source.len());
                        edited_target.push(target.len());
                        source.push(line);
                        target.push(rng.line(&mut id));
                    }
                    _ => {
                        source.push(line.clone());
                        target.push(line);
                    }
                }
            }
            let (source, target) = (source.join("\n") + "\n", target.join("\n") + "\n");

            for algorithm in ALGORITHMS {
                let script = diff(algorithm, &source, &target, false).unwrap();
                for (op, s, t) in &script {
                    match op {
                        Op::Delete => assert!(
                            edited_source.contains(&s.unwrap()),
                            "{algorithm:?}: delete on unedited source line {s:?}"
                        ),
                        Op::Insert => assert!(
                            edited_target.contains(&t.unwrap()),
                            "{algorithm:?}: insert on unedited target line {t:?}"
                        ),
                        Op::Equal => {}
                    }
                }
            }
        }

        #[test]
        fn cancellation_returns_none() {
            let pairs = [
                ("a\nb\nc\n", "a\nx\nc\nd\n"),
                ("same\ntext\n", "same\ntext\n"),
                ("", "only target\n"),
                ("", ""),
            ];
            for algorithm in ALGORITHMS {
                for (source, target) in pairs {
                    assert_eq!(
                        diff(algorithm, source, target, true),
                        None,
                        "{algorithm:?}: {source:?} -> {target:?}"
                    );
                }
            }
        }

        mod ignore_whitespace {
            use super::*;

            fn on() -> IgnoreOptions {
                IgnoreOptions {
                    whitespace: true,
                    ..Default::default()
                }
            }

            /// The line phase's hunks, as (source text, target text).
            pub(super) fn hunks(
                algorithm: MyersDiffAlgorithm,
                source: &str,
                target: &str,
                ignore: &IgnoreOptions,
            ) -> Vec<(String, String)> {
                let (ts, tt) = (lex(source), lex(target));
                let cmp = |a: &RawToken, b: &RawToken| {
                    a.kind == b.kind && source[a.span.clone()] == target[b.span.clone()]
                };
                let text = |text: &str, tokens: &[RawToken], range: Range<usize>| -> String {
                    tokens[range]
                        .iter()
                        .map(|t| &text[t.span.clone()])
                        .collect()
                };
                line_diff(
                    algorithm,
                    &ts,
                    &tt,
                    cmp,
                    &ignore.mask(&ts, &tt),
                    Arc::new(AtomicBool::new(false)),
                )
                .expect("not cancelled")
                .into_iter()
                .map(|h| (text(source, &ts, h.source), text(target, &tt, h.target)))
                .collect()
            }

            /// Lines that differ only in whitespace: indentation, inner and trailing blanks,
            /// line endings, a missing final newline.
            const WHITESPACE_ONLY: [(&str, &str); 4] = [
                (
                    "fn f() {\n    let a = 1;\n\tlet b  =  2;   \n}\n",
                    "fn f() {\nlet a = 1;\n    let b = 2;\n}\n",
                ),
                // A plain token diff matches the blanks and edits `a` (see below).
                ("\t a\n", "a\t \n"),
                ("a\r\nb\r\n", "a\nb\n"),
                ("a\n b", "a\nb\n"),
            ];

            #[test]
            fn whitespace_only_line_changes_form_a_hunk_only_with_the_option_off() {
                for algorithm in ALGORITHMS {
                    for (source, target) in WHITESPACE_ONLY {
                        for (s, t) in [(source, target), (target, source)] {
                            assert_eq!(
                                hunks(algorithm, s, t, &on()),
                                vec![],
                                "{algorithm:?}: {s:?} -> {t:?}"
                            );
                            assert_ne!(
                                hunks(algorithm, s, t, &IgnoreOptions::default()),
                                vec![],
                                "{algorithm:?}: {s:?} -> {t:?}"
                            );
                        }
                    }
                }
            }

            #[test]
            fn whitespace_only_changes_reconstruct_and_edit_only_whitespace() {
                for algorithm in ALGORITHMS {
                    let script = diff(algorithm, "\t a\n", "a\t \n", false).unwrap();
                    assert!(
                        changed_text("\t a\n", "a\t \n", &script).0.contains('a'),
                        "{algorithm:?}: the pair no longer tests key alignment"
                    );

                    for (source, target) in WHITESPACE_ONLY {
                        for (s, t) in [(source, target), (target, source)] {
                            let script = diff_with(algorithm, s, t, &on(), false).unwrap();
                            let (deleted, inserted) = changed_text(s, t, &script);
                            assert!(
                                deleted.trim().is_empty() && inserted.trim().is_empty(),
                                "{algorithm:?}: {s:?} -> {t:?}: -{deleted:?} +{inserted:?}"
                            );
                        }
                    }
                }
            }

            #[test]
            fn a_real_change_among_reindented_lines_is_the_only_hunk() {
                let source = "fn f() {\n    let a = 1;\n    let b = 2;\n    let c = 3;\n}\n";
                let target = "fn f()  {\n\tlet a = 1;\n\tlet b = 20;\n\tlet c = 3;\n}";
                let non_blank = |s: &str| s.split_whitespace().collect::<String>();
                for algorithm in ALGORITHMS {
                    assert_eq!(
                        hunks(algorithm, source, target, &on()),
                        vec![("    let b = 2;\n".into(), "\tlet b = 20;\n".into())],
                        "{algorithm:?}"
                    );
                    for (s, t) in [(source, target), (target, source)] {
                        let script = diff_with(algorithm, s, t, &on(), false).unwrap();
                        let (deleted, inserted) = changed_text(s, t, &script);
                        let mut changed = [non_blank(&deleted), non_blank(&inserted)];
                        changed.sort();
                        assert_eq!(changed, ["2", "20"], "{algorithm:?}: {s:?} -> {t:?}");
                    }
                }
            }
        }

        mod ignore_comments {
            use super::{ignore_whitespace::hunks, *};
            use crate::lexer::TokenKind;

            fn on() -> IgnoreOptions {
                IgnoreOptions {
                    comments: true,
                    ..Default::default()
                }
            }

            fn with_whitespace() -> IgnoreOptions {
                IgnoreOptions {
                    whitespace: true,
                    comments: true,
                }
            }

            /// Lines that differ only in comments: a line comment, words added inside a block
            /// comment (its blanks are part of it), an inline block comment, and comments added
            /// after code (the blanks before them go with them).
            const COMMENT_ONLY: [(&str, &str); 4] = [
                ("let a = 1; // old\n", "let a = 1; // new text\n"),
                ("/*\n * foo bar\n */\nx();\n", "/*\n * foo\n */\nx();\n"),
                (
                    "call(a, /* first */ b);\n",
                    "call(a, /* the first one */ b);\n",
                ),
                ("x();\ny();\n", "x(); // note\ny();  /* more */\n"),
            ];

            /// The (kind, text) of every deleted and inserted token.
            fn edited(source: &str, target: &str, script: &Script) -> Vec<(TokenKind, String)> {
                let (ts, tt) = (lex(source), lex(target));
                let (mut si, mut ti) = (0, 0);
                let mut edited = Vec::new();
                for (op, _, _) in script {
                    match op {
                        Op::Equal => (si, ti) = (si + 1, ti + 1),
                        Op::Delete => {
                            edited.push((ts[si].kind, source[ts[si].span.clone()].to_string()));
                            si += 1;
                        }
                        Op::Insert => {
                            edited.push((tt[ti].kind, target[tt[ti].span.clone()].to_string()));
                            ti += 1;
                        }
                    }
                }
                edited
            }

            fn is_comment_or_blank(kind: TokenKind) -> bool {
                kind.is_comment() || matches!(kind, TokenKind::Whitespace | TokenKind::Tab)
            }

            #[test]
            fn comment_only_line_changes_form_a_hunk_only_with_the_option_off() {
                for algorithm in ALGORITHMS {
                    for (source, target) in COMMENT_ONLY {
                        for (s, t) in [(source, target), (target, source)] {
                            assert_eq!(
                                hunks(algorithm, s, t, &on()),
                                vec![],
                                "{algorithm:?}: {s:?} -> {t:?}"
                            );
                            assert_ne!(
                                hunks(algorithm, s, t, &IgnoreOptions::default()),
                                vec![],
                                "{algorithm:?}: {s:?} -> {t:?}"
                            );
                        }
                    }
                }
            }

            #[test]
            fn comment_only_changes_edit_only_comments_and_their_blanks() {
                for algorithm in ALGORITHMS {
                    for (source, target) in COMMENT_ONLY {
                        for (s, t) in [(source, target), (target, source)] {
                            let script = diff_with(algorithm, s, t, &on(), false).unwrap();
                            let edited = edited(s, t, &script);
                            assert!(
                                edited.iter().all(|(kind, _)| is_comment_or_blank(*kind)),
                                "{algorithm:?}: {s:?} -> {t:?}: {edited:?}"
                            );
                        }
                    }
                }
            }

            #[test]
            fn a_code_change_next_to_a_comment_change_is_still_a_hunk() {
                let source = "a();\nlet a = 1; // old\nb();\n";
                let target = "a();\nlet a = 2; // new\nb();\n";
                for algorithm in ALGORITHMS {
                    for ignore in [on(), with_whitespace()] {
                        assert_eq!(
                            hunks(algorithm, source, target, &ignore),
                            vec![("let a = 1; // old\n".into(), "let a = 2; // new\n".into())],
                            "{algorithm:?}, {ignore:?}"
                        );
                        let script = diff_with(algorithm, source, target, &ignore, false).unwrap();
                        let code: Vec<_> = edited(source, target, &script)
                            .into_iter()
                            .filter(|(kind, _)| !is_comment_or_blank(*kind))
                            .map(|(_, text)| text)
                            .collect();
                        assert_eq!(code, ["1", "2"], "{algorithm:?}, {ignore:?}");
                    }
                }
            }

            #[test]
            fn whitespace_and_comment_changes_need_both_options() {
                let source = "fn f() {\n    let a = 1; // old\n    let b = 2;\n}\n";
                let target = "fn f() {\n\tlet a = 1;   /* new */\n\tlet b = 2; // added\n}";
                for algorithm in ALGORITHMS {
                    for (s, t) in [(source, target), (target, source)] {
                        let whitespace_only = IgnoreOptions {
                            whitespace: true,
                            ..Default::default()
                        };
                        assert_ne!(hunks(algorithm, s, t, &on()), vec![], "{algorithm:?}");
                        assert_ne!(
                            hunks(algorithm, s, t, &whitespace_only),
                            vec![],
                            "{algorithm:?}"
                        );
                        assert_eq!(
                            hunks(algorithm, s, t, &with_whitespace()),
                            vec![],
                            "{algorithm:?}: {s:?} -> {t:?}"
                        );

                        let script = diff_with(algorithm, s, t, &with_whitespace(), false).unwrap();
                        let edited = edited(s, t, &script);
                        assert!(
                            edited
                                .iter()
                                .all(|(kind, _)| kind.is_comment() || kind.is_whitespace()),
                            "{algorithm:?}: {s:?} -> {t:?}: {edited:?}"
                        );
                    }
                }
            }
        }
    }
}
