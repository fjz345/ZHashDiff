use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct Pool<T, const N: usize> {
    data: [T; N],
    // Using AtomicBool for lock-free slot management
    free_list: [AtomicBool; N],
}

impl<T: Default, const N: usize> Pool<T, N> {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            data: std::array::from_fn(|_| T::default()),
            free_list: std::array::from_fn(|_| AtomicBool::new(true)),
        })
    }

    pub fn allocate(self: &Arc<Self>) -> Option<PoolHandle<T, N>> {
        for i in 0..N {
            // "Compare and Swap": If true (free), set to false (occupied)
            if self.free_list[i]
                .compare_exchange(true, false, Ordering::SeqCst, Ordering::Relaxed)
                .is_ok()
            {
                return Some(PoolHandle {
                    pool: Arc::clone(self),
                    index: i,
                });
            }
        }
        None
    }
}

pub struct PoolHandle<T, const N: usize> {
    pool: Arc<Pool<T, N>>,
    pub index: usize,
}

impl<T, const N: usize> PoolHandle<T, N> {
    pub fn get(&self) -> &T {
        &self.pool.data[self.index]
    }
}

impl<T, const N: usize> Drop for PoolHandle<T, N> {
    fn drop(&mut self) {
        // Release the slot back to the pool
        self.pool.free_list[self.index].store(true, Ordering::SeqCst);
    }
}
