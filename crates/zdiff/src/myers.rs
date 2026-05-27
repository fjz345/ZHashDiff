use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Debug, Clone, Copy, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum MyersDiffAlgorithm {
    Trace, // N+M^2 memory
    #[default]
    Linear, // N+M memory
    LinearMT, // N+M memory with multi-threading
}

pub fn myers_diff_path<T, F>(
    algorithm: MyersDiffAlgorithm,
    source: &[T],
    target: &[T],
    cmp: F,
    cancel_flag: Arc<AtomicBool>,
) -> Option<Vec<(i32, i32)>>
where
    T: Sync,
    F: Fn(&T, &T) -> bool + Sync,
{
    match algorithm {
        MyersDiffAlgorithm::Trace => {
            let trace = myers_diff_trace(source, target, cmp);
            myers_backtrack(trace, source.len() as i32, target.len() as i32, cancel_flag)
        }
        MyersDiffAlgorithm::Linear => myers_diff_linear(source, target, cmp, cancel_flag),
        MyersDiffAlgorithm::LinearMT => myers_diff_linear_mt(source, target, cmp, cancel_flag),
    }
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
) -> Option<Vec<(i32, i32)>> {
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

pub fn myers_count_add_deletes(diff_path: &[(i32, i32)]) -> (u32, u32) {
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
    path: &mut Vec<(i32, i32)>,
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
) -> Option<Vec<(i32, i32)>>
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
    path: &mut Vec<(i32, i32)>,
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
) -> Option<Vec<(i32, i32)>>
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
}
