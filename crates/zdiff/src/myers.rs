
pub fn myers_diff<T, F>(a: &[T], b: &[T], mut cmp: F) -> i32 
where 
    F: FnMut(&T, &T) -> bool 
{
    let n = a.len() as i32;
    let m = b.len() as i32;
    let max = n + m;
    
    if max == 0 { return 0; }

    // V array stores the furthest reaching x-coordinate for each diagonal k.
    // Index range is -max..=max, so we size it to 2*max + 1.
    let mut v = vec![0; (2 * max + 1) as usize];
    let offset = max as usize;

    for d in 0..=max {
        // k is the diagonal: k = x - y
        for k in (-d..=d).step_by(2) {
            let idx = (k + (max as i32)) as usize;
            
            let mut x = if k == -d || (k != d && v[idx - 1] < v[idx + 1]) {
                v[idx + 1] // Move down
            } else {
                v[idx - 1] + 1 // Move right
            };

            let mut y = x - k;

            // Greedily follow diagonals (matches)
            while x < n && y < m && cmp(&a[x as usize], &b[y as usize]) {
                x += 1;
                y += 1;
            }

            v[idx] = x;

            if x >= n && y >= m {
                return d;
            }
        }
    }
    max
}

pub fn myers_diff_trace<T, F>(a: &[T], b: &[T], mut cmp: F) -> Vec<Vec<i32>>
where
    F: FnMut(&T, &T) -> bool,
{
    let n = a.len() as i32;
    let m = b.len() as i32;
    let max = n + m;
    let mut v = vec![0; (2 * max + 1) as usize];
    let mut trace = Vec::new();

    for d in 0..=max {
        trace.push(v.clone());
        for k in (-d..=d).step_by(2) {
            let idx = (k + max) as usize;
            let mut x = if k == -d || (k != d && v[idx - 1] < v[idx + 1]) {
                v[idx + 1] // Move Down (Insert)
            } else {
                v[idx - 1] + 1 // Move Right (Delete)
            };

            let mut y = x - k;
            while x < n && y < m && cmp(&a[x as usize], &b[y as usize]) {
                x += 1;
                y += 1;
            }
            v[idx] = x;

            if x >= n && y >= m {
                return trace;
            }
        }
    }
    trace
}

pub fn backtrack(trace: Vec<Vec<i32>>, n: i32, m: i32) -> Vec<(i32, i32)> {
    let mut path = Vec::new();
    let mut x = n;
    let mut y = m;
    let max = n + m;

    // Iterate backwards from the last depth 'd' to 0
    for (d, v) in trace.into_iter().enumerate().rev() {
        let d = d as i32;
        let k = x - y;
        let idx = (k + max) as usize;

        // Determine if we got here via a move from k-1 (Right/Delete) or k+1 (Down/Insert)
        // We check the same logic used in the forward pass
        let prev_k = if k == -d || (k != d && v[idx - 1] < v[idx + 1]) {
            k + 1 // We came from above (Vertical/Insert)
        } else {
            k - 1 // We came from the left (Horizontal/Delete)
        };

        let prev_x = v[(prev_k + max) as usize];
        let prev_y = prev_x - prev_k;

        // The diagonal matches (snakes) happened AFTER the move to (x,y).
        // So in reverse, we process them BEFORE moving to the previous k.
        while x > prev_x && y > prev_y {
            path.push((x, y));
            x -= 1;
            y -= 1;
        }

        path.push((x, y));
        x = prev_x;
        y = prev_y;
    }

    // (0,0) is handled by the last iteration of the loop (d=0)
    path.reverse();
    path
}