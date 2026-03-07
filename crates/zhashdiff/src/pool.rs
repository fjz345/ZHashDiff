#![allow(dead_code)]
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

pub struct Pool<T, const N: usize> {
    data: [T; N],
    free_list: [AtomicBool; N],
    generation: AtomicUsize,
}

impl<T: Default, const N: usize> Default for Pool<T, N> {
    fn default() -> Self {
        Self {
            data: std::array::from_fn(|_| T::default()),
            free_list: std::array::from_fn(|_| AtomicBool::new(true)),
            generation: AtomicUsize::new(0),
        }
    }
}

impl<T: Default, const N: usize> Pool<T, N> {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            data: std::array::from_fn(|_| T::default()),
            free_list: std::array::from_fn(|_| AtomicBool::new(true)),
            generation: AtomicUsize::new(0),
        })
    }

    pub fn len(&self) -> usize {
        self.free_list
            .iter()
            .filter(|free| !free.load(Ordering::Relaxed))
            .count()
    }

    pub fn free_count(&self) -> usize {
        self.free_list
            .iter()
            .filter(|free| free.load(Ordering::Relaxed))
            .count()
    }

    pub const fn capacity(&self) -> usize {
        N
    }

    pub fn clear(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        for slot in &self.free_list {
            slot.store(true, Ordering::SeqCst);
        }
    }

    pub fn allocate(self: &Arc<Self>) -> Option<PoolHandle<T, N>> {
        let current_gen = self.generation.load(Ordering::SeqCst);
        for i in 0..N {
            if self.free_list[i]
                .compare_exchange(true, false, Ordering::SeqCst, Ordering::Relaxed)
                .is_ok()
            {
                return Some(PoolHandle {
                    pool: Arc::clone(self),
                    index: i,
                    generation: current_gen,
                });
            }
        }
        None
    }
}

pub struct PoolHandle<T, const N: usize> {
    pool: Arc<Pool<T, N>>,
    pub index: usize,
    generation: usize,
}

impl<T, const N: usize> Drop for PoolHandle<T, N> {
    fn drop(&mut self) {
        // ONLY release if the pool hasn't been cleared since this handle was created
        if self.pool.generation.load(Ordering::SeqCst) == self.generation {
            self.pool.free_list[self.index].store(true, Ordering::SeqCst);
        }
    }
}
