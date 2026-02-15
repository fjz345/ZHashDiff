use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, Receiver, Sender, TryRecvError},
    },
    thread,
    time::Duration,
};

pub fn hash_file(path: &Path) -> io::Result<String> {
    let mut hasher = blake3::Hasher::new();
    hasher.update_mmap_rayon(path)?;
    Ok(hasher.finalize().to_hex().to_string())
}

#[derive(Debug, Clone)]
pub struct HashServiceSnapshot {
    pub hashes: HashMap<PathBuf, Option<String>>,
    pub active_count: usize,
    pub queue_count: usize,
    pub num_workers: usize,
}

#[derive(Debug)]
pub struct HashService {
    hashes: Arc<RwLock<HashMap<PathBuf, Option<String>>>>,
    tx: Sender<PathBuf>,
    rx: Arc<Mutex<Receiver<PathBuf>>>,
    in_progress: Arc<AtomicUsize>,
    workers: Vec<WorkerHandle>,
}

#[derive(Debug)]
struct WorkerHandle {
    handle: thread::JoinHandle<()>,
    stop_flag: Arc<AtomicBool>,
}

impl Default for HashService {
    fn default() -> Self {
        HashService::new(4)
    }
}

impl HashService {
    pub fn new(worker_count: usize) -> Self {
        let (tx, rx) = mpsc::channel::<PathBuf>();
        let rx = Arc::new(Mutex::new(rx));

        let hashes = Arc::new(RwLock::new(HashMap::new()));
        let in_progress = Arc::new(AtomicUsize::new(0));

        let mut service = Self {
            hashes,
            tx,
            rx,
            in_progress,
            workers: Vec::new(),
        };

        service.resize_workers(worker_count);
        service
    }

    fn spawn_worker(&mut self, id: usize) {
        let rx = self.rx.clone();
        let hashes = self.hashes.clone();
        let in_progress = self.in_progress.clone();
        let stop_flag = Arc::new(AtomicBool::new(false));
        let thread_stop_flag = stop_flag.clone();

        let handle = thread::spawn(move || {
            log::info!("Worker {id} started");

            loop {
                if thread_stop_flag.load(Ordering::SeqCst) {
                    log::info!("Worker {id} stop signal");
                    break;
                }

                let msg = {
                    let rx_guard = rx.lock().unwrap();
                    rx_guard.try_recv()
                };

                match msg {
                    Ok(path) => {
                        in_progress.fetch_add(1, Ordering::SeqCst);
                        let hash = hash_file(&path).ok();
                        hashes.write().unwrap().insert(path, hash);
                        in_progress.fetch_sub(1, Ordering::SeqCst);
                    }
                    Err(TryRecvError::Empty) => {
                        thread::sleep(Duration::from_millis(50));
                        continue;
                    }
                    Err(TryRecvError::Disconnected) => {
                        log::info!("Worker {id} channel disconnected");
                        break;
                    }
                }
            }

            log::info!("Worker {id} exited");
        });

        self.workers.push(WorkerHandle { handle, stop_flag });
    }

    pub fn resize_workers(&mut self, new_count: usize) {
        let current = self.workers.len();

        log::info!("Resizing workers from {current} -> {new_count}");
        if new_count > current {
            for id in current..new_count {
                self.spawn_worker(id);
            }
        } else if new_count < current {
            let remove_count = current - new_count;

            for worker in self.workers.iter().rev().take(remove_count) {
                worker.stop_flag.store(true, Ordering::SeqCst);
            }

            for _ in 0..remove_count {
                if let Some(worker) = self.workers.pop() {
                    let _ = worker.handle.join();
                }
            }
        }
    }

    pub fn request(&self, path: PathBuf) {
        let mut hashes = self.hashes.write().unwrap();
        if hashes.contains_key(&path) {
            return;
        }
        hashes.insert(path.clone(), None);
        let _ = self.tx.send(path);
    }

    pub fn remove(&self, path: &PathBuf) {
        if let Ok(mut hashes) = self.hashes.write() {
            hashes.remove(path);
        }
    }

    pub fn get(&self, path: &PathBuf) -> Option<Option<String>> {
        self.hashes.read().unwrap().get(path).cloned()
    }

    pub fn clear(&self) {
        self.hashes.write().unwrap().clear();
    }

    pub fn count_threads(&self) -> usize {
        self.workers.len()
    }

    pub fn count_active_hashes(&self) -> usize {
        self.in_progress.load(Ordering::SeqCst)
    }

    pub fn count_hash_queue(&self) -> usize {
        let hashes = self.hashes.read().unwrap();
        let pending = hashes.values().filter(|v| v.is_none()).count();
        let active = self.in_progress.load(Ordering::SeqCst);
        pending.saturating_sub(active)
    }

    pub fn snapshot(&self) -> HashServiceSnapshot {
        HashServiceSnapshot {
            hashes: self.hashes.read().unwrap().clone(),
            active_count: self.count_active_hashes(),
            queue_count: self.count_hash_queue(),
            num_workers: self.count_threads(),
        }
    }
}

pub fn find_conflicts(
    hashes: &HashMap<PathBuf, Option<String>>,
    selected: &HashMap<PathBuf, bool>,
) -> HashMap<String, Vec<PathBuf>> {
    let mut groups: HashMap<String, Vec<PathBuf>> = HashMap::new();
    for (path, hash) in hashes {
        if selected.get(path).copied().unwrap_or(false) {
            if let Some(h) = hash {
                groups.entry(h.clone()).or_default().push(path.clone());
            }
        }
    }
    groups.retain(|_, v| v.len() > 1);
    groups
}

pub struct ResolveConflictsInput {
    pub conflict_map: HashMap<String, Vec<PathBuf>>,
    pub conflict_map_resolved: HashMap<String, PathBuf>,
}

pub struct ResolveConflictsOutput {
    pub removed_files: Vec<PathBuf>,
}

pub fn execute_resolution(input: &ResolveConflictsInput) -> ResolveConflictsOutput {
    let mut output = ResolveConflictsOutput {
        removed_files: Vec::new(),
    };

    log::info!("Starting file resolution process...");

    let conflicts = &input.conflict_map;
    let resolutions = &input.conflict_map_resolved;

    for (hash, paths) in conflicts {
        if let Some(path_to_keep) = resolutions.get(hash) {
            for path in paths {
                if path != path_to_keep {
                    match std::fs::remove_file(&path) {
                        Ok(_) => {
                            log::info!("Deleted duplicate: {:?}", path);
                            output.removed_files.push(path.clone());
                        }
                        Err(e) => {
                            log::error!("Failed to delete {:?}: {}", path, e);
                        }
                    }
                }
            }
        }
    }

    log::info!(
        "Resolution complete. Removed {} files.",
        output.removed_files.len()
    );
    output
}
