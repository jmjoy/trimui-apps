//! Priority thread pool for terrain generation, lighting and meshing.
//!
//! Threads are created for every core that may come online, but only
//! `active` of them run jobs at once, so the CPU option can change without a
//! restart. Lower priority values run first; equal priorities run FIFO.
use std::{
    cmp::Ordering,
    collections::BinaryHeap,
    sync::{Arc, Condvar, Mutex, MutexGuard},
    thread::JoinHandle,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// Generation and lighting jobs are scheduled once the streaming world lands.
#[allow(dead_code)]
pub enum Kind {
    Generate = 0,
    Light = 1,
    Mesh = 2,
    Other = 3,
}

struct Task {
    priority: i64,
    seq: u64,
    kind: Kind,
    run: Box<dyn FnOnce() + Send>,
}
impl PartialEq for Task {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority && self.seq == other.seq
    }
}
impl Eq for Task {}
impl PartialOrd for Task {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Task {
    // BinaryHeap is a max-heap; invert so the smallest priority pops first.
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .priority
            .cmp(&self.priority)
            .then_with(|| other.seq.cmp(&self.seq))
    }
}

#[derive(Default)]
struct State {
    queue: BinaryHeap<Task>,
    queued: [usize; 4],
    active: usize,
    running: usize,
    seq: u64,
    shutdown: bool,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}
impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        // A panicking job must not wedge the whole pool.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

pub struct Pool {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
}

impl Pool {
    pub fn new(threads: usize, active: usize) -> Self {
        let threads = threads.max(1);
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                active: active.clamp(1, threads),
                ..Default::default()
            }),
            wake: Condvar::new(),
        });
        let handles = (0..threads)
            .map(|i| {
                let shared = Arc::clone(&shared);
                std::thread::Builder::new()
                    .name(format!("voxel-worker-{i}"))
                    .spawn(move || worker(&shared))
                    .expect("spawning worker thread")
            })
            .collect();
        Self {
            shared,
            threads: handles,
        }
    }
    pub fn threads(&self) -> usize {
        self.threads.len()
    }
    pub fn submit(&self, kind: Kind, priority: i64, job: impl FnOnce() + Send + 'static) {
        let mut state = self.shared.lock();
        state.seq += 1;
        let seq = state.seq;
        state.queued[kind as usize] += 1;
        state.queue.push(Task {
            priority,
            seq,
            kind,
            run: Box::new(job),
        });
        drop(state);
        self.shared.wake.notify_one();
    }
    pub fn set_active(&self, active: usize) {
        let mut state = self.shared.lock();
        let active = active.clamp(1, self.threads.len());
        if state.active != active {
            log::info!("Worker threads active: {active}/{}", self.threads.len());
            state.active = active;
        }
        drop(state);
        self.shared.wake.notify_all();
    }
    pub fn active(&self) -> usize {
        self.shared.lock().active
    }
    /// Queued (not yet running) jobs per kind: generate, light, mesh, other.
    pub fn queued(&self) -> [usize; 4] {
        self.shared.lock().queued
    }
    pub fn running(&self) -> usize {
        self.shared.lock().running
    }
}

fn worker(shared: &Shared) {
    // Per-thread nice value on Linux: keep the frame thread responsive.
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, 0, 5);
    }
    loop {
        let task = {
            let mut state = shared.lock();
            loop {
                if state.shutdown {
                    return;
                }
                if state.running < state.active
                    && let Some(task) = state.queue.pop()
                {
                    state.queued[task.kind as usize] -= 1;
                    state.running += 1;
                    break task;
                }
                state = shared.wake.wait(state).unwrap_or_else(|e| e.into_inner());
            }
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(task.run));
        if result.is_err() {
            log::error!("Background {:?} job panicked", task.kind);
        }
        shared.lock().running -= 1;
        shared.wake.notify_all();
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        let mut state = self.shared.lock();
        state.shutdown = true;
        state.queue.clear();
        state.queued = [0; 4];
        drop(state);
        self.shared.wake.notify_all();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{
            atomic::{AtomicUsize, Ordering},
            mpsc,
        },
        time::Duration,
    };

    #[test]
    fn priority_order_and_active_limit() {
        let pool = Pool::new(3, 1);
        let (gate_tx, gate_rx) = mpsc::channel::<()>();
        let (tx, rx) = mpsc::channel();
        // Occupy the only active worker so the queue can fill up.
        pool.submit(Kind::Other, 0, move || {
            gate_rx.recv().unwrap();
        });
        std::thread::sleep(Duration::from_millis(50));
        for (priority, label) in [(5, "c"), (1, "a"), (3, "b"), (1, "a2")] {
            let tx = tx.clone();
            pool.submit(Kind::Mesh, priority, move || tx.send(label).unwrap());
        }
        assert_eq!(pool.queued()[Kind::Mesh as usize], 4);
        assert_eq!(pool.running(), 1);
        gate_tx.send(()).unwrap();
        let order: Vec<_> = (0..4)
            .map(|_| rx.recv_timeout(Duration::from_secs(2)).unwrap())
            .collect();
        assert_eq!(order, ["a", "a2", "b", "c"]);
    }

    #[test]
    fn concurrency_follows_active_count_and_survives_panics() {
        let pool = Pool::new(3, 1);
        let peak = Arc::new(AtomicUsize::new(0));
        let now = Arc::new(AtomicUsize::new(0));
        let run = |pool: &Pool, n: usize| {
            let (tx, rx) = mpsc::channel();
            for _ in 0..n {
                let (peak, now, tx) = (peak.clone(), now.clone(), tx.clone());
                pool.submit(Kind::Generate, 0, move || {
                    let c = now.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(c, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(20));
                    now.fetch_sub(1, Ordering::SeqCst);
                    tx.send(()).unwrap();
                });
            }
            for _ in 0..n {
                rx.recv_timeout(Duration::from_secs(5)).unwrap();
            }
        };
        run(&pool, 6);
        assert_eq!(peak.load(Ordering::SeqCst), 1);
        pool.set_active(3);
        run(&pool, 9);
        assert!(peak.load(Ordering::SeqCst) >= 2);
        pool.submit(Kind::Other, 0, || panic!("job failure is contained"));
        peak.store(0, Ordering::SeqCst);
        run(&pool, 3);
        assert!(peak.load(Ordering::SeqCst) >= 1);
        assert_eq!(pool.threads(), 3);
    }
}
