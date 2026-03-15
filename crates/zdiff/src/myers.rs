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
pub fn myers_diff_trace<T, F>(source: &[T], target: &[T], mut cmp: F) -> Vec<Vec<i32>>
where
    F: FnMut(&T, &T) -> bool,
{
    let source_len = source.len() as i32; // N
    let target_len = target.len() as i32; // M
    let max_edit_distance = source_len + target_len; // MAX = N + M

    if max_edit_distance == 0 {
        return vec![vec![0]];
    }

    let mut furthest_x = vec![0; (2 * max_edit_distance + 1) as usize]; // V array
    let mut trace = Vec::with_capacity(max_edit_distance as usize + 1);
    let offset = max_edit_distance as usize;

    for d in 0..=max_edit_distance {
        // Optimization: Only clone the active diagonal range [-d, d]
        let start = offset - d as usize;
        let end = offset + d as usize + 1;
        trace.push(furthest_x[start..end].to_vec());

        for k in (-d..=d).step_by(2) {
            // diagonal
            let idx = (k + max_edit_distance) as usize;

            let mut x = if k == -d || (k != d && furthest_x[idx - 1] < furthest_x[idx + 1]) {
                furthest_x[idx + 1] // Move Down
            } else {
                furthest_x[idx - 1] + 1 // Move Right
            };

            let mut y = x - k;
            while x < source_len && y < target_len && cmp(&source[x as usize], &target[y as usize])
            {
                x += 1;
                y += 1;
            }
            furthest_x[idx] = x;

            if x >= source_len && y >= target_len {
                return trace;
            }
        }
    }
    trace
}

pub fn myers_backtrack(trace: Vec<Vec<i32>>, source_len: i32, target_len: i32) -> Vec<(i32, i32)> {
    let mut path = Vec::new();
    let mut x = source_len;
    let mut y = target_len;

    for d in (1..trace.len()).rev() {
        let edit_distance = d as i32; // D
        let k = x - y; // diagonal
        let v_prev = &trace[d];
        let offset = d;

        let came_from_k_plus = if k == -edit_distance {
            true // Came from k + 1
        } else if k == edit_distance {
            false // Came from k - 1
        } else {
            v_prev[(k - 1 + offset as i32) as usize] < v_prev[(k + 1 + offset as i32) as usize]
        };

        let (prev_k, prev_x) = if came_from_k_plus {
            (k + 1, v_prev[(k + 1 + offset as i32) as usize])
        } else {
            (k - 1, v_prev[(k - 1 + offset as i32) as usize])
        };

        let prev_y = prev_x - prev_k;

        let (x_mid, y_mid) = if came_from_k_plus {
            (prev_x, prev_y + 1)
        } else {
            (prev_x + 1, prev_y)
        };

        while x > x_mid && y > y_mid {
            path.push((x, y));
            x -= 1;
            y -= 1;
        }

        path.push((x, y));
        x = prev_x;
        y = prev_y;
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
