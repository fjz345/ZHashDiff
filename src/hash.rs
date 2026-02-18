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

pub fn hash_file(path: impl AsRef<Path>) -> io::Result<String> {
    let mut hasher = blake3::Hasher::new();
    hasher.update_mmap_rayon(path)?;
    Ok(hasher.finalize().to_hex().to_string())
}

pub type HashRepresentation = String;

#[derive(Debug, Clone)]
pub struct HashServiceSnapshot {
    pub hashes: HashMap<PathBuf, Option<HashRepresentation>>,
    pub active_count: usize,
    pub queue_count: usize,
    pub num_workers: usize,
}

#[derive(Debug)]
pub struct HashService {
    hashes: Arc<RwLock<HashMap<PathBuf, Option<HashRepresentation>>>>,
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

    pub fn request(&self, path: impl AsRef<Path>) {
        let mut hashes = self.hashes.write().unwrap();
        if hashes.contains_key(path.as_ref().into()) {
            return;
        }
        hashes.insert(path.as_ref().into(), None);
        let _ = self.tx.send(path.as_ref().into());
    }

    pub fn remove(&self, path: impl AsRef<Path>) {
        if let Ok(mut hashes) = self.hashes.write() {
            hashes.remove(path.as_ref());
        }
    }

    // TODO: fix to get reference instead of clone
    pub fn get_hash(&self, path: impl AsRef<Path>) -> Option<HashRepresentation> {
        let binding = self.hashes.read().unwrap();
        let hash = binding.get(path.as_ref().into()).and_then(|f| f.clone());
        hash
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
