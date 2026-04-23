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
returns "cost" between source/target comparing each entry
*/
pub fn myers_diff<T, F>(source: &[T], target: &[T], cmp: F) -> i32
where
    F: FnMut(&T, &T) -> bool,
{
    let trace = myers_diff_trace(source, target, cmp);
    (trace.len() as i32) - 1 // D = trace.len() - 1
}

/*
returns the path of lowest cost (edits) to get from source to target
*/
pub fn myers_diff_trace<T, F>(source: &[T], target: &[T], mut cmp: F) -> MyersTrace
where
    F: FnMut(&T, &T) -> bool,
{
    let source_len = source.len() as i32;
    let target_len = target.len() as i32;
    let max_possible_edits = source_len + target_len;

    // We use +2 to allow safe boundary checks for (diagonal + 1) and (diagonal - 1)
    // even at the extreme edges of the search space.
    let mut furthest_x_on_diagonal = vec![0; (2 * max_possible_edits + 2) as usize];
    let mut trace = MyersTrace::new(max_possible_edits as usize + 1);
    let diagonal_offset = max_possible_edits as usize;

    // Initialization: the virtual starting point for the D=0 iteration
    furthest_x_on_diagonal[diagonal_offset + 1] = 0;

    for edit_distance in 0..=max_possible_edits {
        for diagonal in (-edit_distance..=edit_distance).step_by(2) {
            let v_index = (diagonal + max_possible_edits) as usize;

            // Determine move direction based on the furthest reaching previous diagonals
            let mut current_x = if diagonal == -edit_distance
                || (diagonal != edit_distance
                    && furthest_x_on_diagonal[v_index - 1] < furthest_x_on_diagonal[v_index + 1])
            {
                furthest_x_on_diagonal[v_index + 1] // Move Down
            } else {
                furthest_x_on_diagonal[v_index - 1] + 1 // Move Right
            };

            let mut current_y = current_x - diagonal;

            // Slide down the "snake" (diagonal matches)
            while current_x < source_len
                && current_y < target_len
                && cmp(&source[current_x as usize], &target[current_y as usize])
            {
                current_x += 1;
                current_y += 1;
            }

            furthest_x_on_diagonal[v_index] = current_x;

            // If we've reached the end of both sequences, we're done
            if current_x >= source_len && current_y >= target_len {
                trace.push(
                    &furthest_x_on_diagonal[(diagonal_offset - edit_distance as usize)
                        ..=(diagonal_offset + edit_distance as usize)],
                );
                return trace;
            }
        }

        // Save the furthest X for every diagonal at this edit distance
        trace.push(
            &furthest_x_on_diagonal[(diagonal_offset - edit_distance as usize)
                ..=(diagonal_offset + edit_distance as usize)],
        );
    }
    trace
}

pub fn myers_backtrack(trace: MyersTrace, source_len: i32, target_len: i32) -> Vec<(i32, i32)> {
    let mut path = Vec::with_capacity((source_len + target_len) as usize + 1);
    let mut current_x = source_len;
    let mut current_y = target_len;

    // Start from the final depth and work backwards to D=1
    for edit_distance in (1..trace.len()).rev() {
        let d_idx = edit_distance as i32;
        let diagonal = current_x - current_y;
        let prev_v_slice = &trace[edit_distance - 1];
        let prev_d_idx = d_idx - 1;

        // Logic check: Did we come from the diagonal above (Down) or the diagonal to the left (Right)?
        let came_from_above = if diagonal == -d_idx
            || (diagonal != d_idx
                && prev_v_slice[(diagonal + 1 + prev_d_idx) as usize]
                    > prev_v_slice[(diagonal - 1 + prev_d_idx) as usize])
        {
            true
        } else {
            false
        };

        let k_prev = if came_from_above {
            diagonal + 1
        } else {
            diagonal - 1
        };
        let x_before_snake = if came_from_above {
            prev_v_slice[(k_prev + prev_d_idx) as usize]
        } else {
            prev_v_slice[(k_prev + prev_d_idx) as usize] + 1
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
    path
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

        let dist = myers_diff(&a, &b, cmp);
        let trace = myers_diff_trace(&a, &b, cmp);
        let path = myers_backtrack(trace, a.len() as i32, b.len() as i32);

        assert_eq!(dist, 0);
        assert_eq!(distance_from_path(&path), 0);
        assert_eq!(path.len(), 4); // (0,0) -> (1,1) -> (2,2) -> (3,3)
    }

    #[test]
    fn test_completely_different() {
        let a = vec!["a", "b"];
        let b = vec!["c", "d"];
        let cmp = |t1: &&str, t2: &&str| t1 == t2;

        let dist = myers_diff(&a, &b, cmp);
        assert_eq!(dist, 4); // 2 deletes, 2 inserts
    }

    #[test]
    fn test_empty_sequences() {
        let a: Vec<&str> = vec![];
        let b: Vec<&str> = vec!["a", "b"];
        let cmp = |t1: &&str, t2: &&str| t1 == t2;

        assert_eq!(myers_diff(&a, &b, cmp), 2);
        assert_eq!(myers_diff(&b, &a, cmp), 2);
        assert_eq!(myers_diff(&a, &a, cmp), 0);
    }

    #[test]
    fn test_complex_interleaving() {
        let a: Vec<char> = "ABCABBA".chars().collect();
        let b: Vec<char> = "CBABAC".chars().collect();
        let cmp = |t1: &char, t2: &char| t1 == t2;

        let dist = myers_diff(&a, &b, cmp);
        let trace = myers_diff_trace(&a, &b, cmp);
        let path = myers_backtrack(trace, a.len() as i32, b.len() as i32);

        assert_eq!(dist, 5);
        assert_eq!(distance_from_path(&path), 5);
    }

    #[test]
    fn test_rust_token_edit_logic() {
        let a = vec!["fn", "main", "(", ")", "{", "}"];
        let b = vec!["fn", "main2", "(", ")", "{", "}"];
        let cmp = |t1: &&str, t2: &&str| t1 == t2;

        let trace = myers_diff_trace(&a, &b, cmp);
        let path = myers_backtrack(trace, a.len() as i32, b.len() as i32);

        // Distance should be 2 (Delete main, Insert main2)
        assert_eq!(distance_from_path(&path), 2);

        // Path should include (3,3) which is the match for '('
        assert!(path.contains(&(3, 3)));
    }

    #[test]
    fn test_path_continuity() {
        let a = vec!["A", "B", "C"];
        let b = vec!["A", "X", "C"];
        let cmp = |t1: &&str, t2: &&str| t1 == t2;

        let trace = myers_diff_trace(&a, &b, cmp);
        let path = myers_backtrack(trace, a.len() as i32, b.len() as i32);

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
