/*
returns "cost" between source/target comparing each entry
*/
pub fn myers_diff<T, F>(source: &[T], target: &[T], cmp: F) -> i32 
where 
    F: FnMut(&T, &T) -> bool 
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
    let source_len = source.len() as i32;            // N
    let target_len = target.len() as i32;            // M
    let max_edit_distance = source_len + target_len; // MAX = N + M
    
    if max_edit_distance == 0 { return vec![vec![0]]; }

    let mut furthest_x = vec![0; (2 * max_edit_distance + 1) as usize]; // V array
    let mut trace = Vec::with_capacity(max_edit_distance as usize + 1);

    for edit_distance in 0..=max_edit_distance {
        trace.push(furthest_x.clone());
        
        for diagonal in (-edit_distance..=edit_distance).step_by(2) {
            let v_idx = (diagonal + max_edit_distance) as usize; 
            
            let mut x = if diagonal == -edit_distance || (diagonal != edit_distance && furthest_x[v_idx - 1] < furthest_x[v_idx + 1]) {
                furthest_x[v_idx + 1]     // Move Down (Insertion)
            } else {
                furthest_x[v_idx - 1] + 1 // Move Right (Deletion)
            };

            let mut y = x - diagonal; // y = x - k
            while x < source_len && y < target_len && cmp(&source[x as usize], &target[y as usize]) {
                x += 1; // Greedy snake (Match)
                y += 1;
            }
            furthest_x[v_idx] = x;

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
    let max_edit_distance = source_len + target_len; // MAX offset

    for (edit_distance, furthest_x) in trace.into_iter().enumerate().rev() {
        let d = edit_distance as i32; // D
        let diagonal = x - y;         // k
        let v_idx = (diagonal + max_edit_distance) as usize;

        let prev_diagonal = if diagonal == -d || (diagonal != d && furthest_x[v_idx - 1] < furthest_x[v_idx + 1]) {
            diagonal + 1 // Came from k + 1
        } else {
            diagonal - 1 // Came from k - 1
        };

        let prev_x = furthest_x[(prev_diagonal + max_edit_distance) as usize];
        let prev_y = prev_x - prev_diagonal; // prev_y = prev_x - prev_k

        while x > prev_x && y > prev_y {
            path.push((x, y));  // Step back through snake
            x -= 1;
            y -= 1;
        }

        path.push((x, y));      // Step back through edit
        x = prev_x;
        y = prev_y;
    }

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
            adds += 1;    // Vertical = Target consumed = Addition
        }
    }
    (adds, deletes)
}