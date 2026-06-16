//! Drives the [`Nester`] continuously and publishes results to the UI.
//!
//! Native builds run the optimizer on a background thread using a rayon pool
//! sized to the requested core count. Wasm builds (no threads) advance the
//! optimizer incrementally from the UI loop via [`Engine::tick`].
//!
//! Starting and stopping never block the UI thread. Each run is tagged with an
//! `epoch`; the worker checks it once per generation and exits on its own when
//! the epoch changes (i.e. on stop or restart), so we never `join()` from the
//! UI. Stale workers detach and wind down within at most one generation.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::model::{NestConfig, NestResult, Part, SheetSlot};
use crate::nest::Nester;

#[derive(Default)]
struct Shared {
    running: AtomicBool,
    generation: AtomicU64,
    /// Monotonic id of the currently-desired run. Bumped on every start/stop.
    /// Native-only: it's how a detached worker learns it's been superseded.
    #[cfg(not(target_arch = "wasm32"))]
    epoch: AtomicU64,
    best: Mutex<Option<NestResult>>,
    slots: Mutex<Vec<SheetSlot>>,
    /// Set when a started run had nothing to nest.
    invalid: AtomicBool,
}

pub struct Engine {
    shared: Arc<Shared>,
    #[cfg(target_arch = "wasm32")]
    nester: Option<Nester>,
}

impl Default for Engine {
    fn default() -> Self {
        Self {
            shared: Arc::new(Shared::default()),
            #[cfg(target_arch = "wasm32")]
            nester: None,
        }
    }
}

impl Engine {
    pub fn is_running(&self) -> bool {
        self.shared.running.load(Ordering::Relaxed)
    }

    pub fn generation(&self) -> u64 {
        self.shared.generation.load(Ordering::Relaxed)
    }

    pub fn invalid(&self) -> bool {
        self.shared.invalid.load(Ordering::Relaxed)
    }

    /// Snapshot the current best result (cloned).
    pub fn best(&self) -> Option<NestResult> {
        self.shared.best.lock().unwrap().clone()
    }

    pub fn slots(&self) -> Vec<SheetSlot> {
        self.shared.slots.lock().unwrap().clone()
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn start(&mut self, parts: Vec<Part>, config: NestConfig) {
        // Invalidate any running worker (it will see the new epoch and exit),
        // then claim this epoch for the run we're about to spawn.
        let my_epoch = self.shared.epoch.fetch_add(1, Ordering::SeqCst) + 1;
        let shared = self.shared.clone();
        shared.invalid.store(false, Ordering::SeqCst);
        shared.running.store(true, Ordering::SeqCst);
        shared.generation.store(0, Ordering::SeqCst);
        *shared.best.lock().unwrap() = None;

        let threads = config.threads;
        // Detached: we never join from the UI thread.
        std::thread::spawn(move || {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads) // 0 = all cores
                .build()
                .ok();

            let still_current = || shared.epoch.load(Ordering::SeqCst) == my_epoch;

            let run = || {
                let mut nester = Nester::new(parts, config);
                // A newer run may have started while we built the population.
                if !still_current() {
                    return;
                }
                *shared.slots.lock().unwrap() = nester.sheet_slots().to_vec();
                if !nester.ready {
                    shared.invalid.store(true, Ordering::SeqCst);
                    if still_current() {
                        shared.running.store(false, Ordering::SeqCst);
                    }
                    return;
                }
                *shared.best.lock().unwrap() = Some(nester.best.clone());
                shared.generation.store(nester.generation, Ordering::SeqCst);
                while still_current() {
                    nester.step();
                    if !still_current() {
                        break;
                    }
                    *shared.best.lock().unwrap() = Some(nester.best.clone());
                    shared.generation.store(nester.generation, Ordering::SeqCst);
                }
            };

            match pool {
                Some(p) => p.install(run),
                None => run(),
            }
            // `pool` drops here (on this worker thread, not the UI thread).
        });
    }

    /// Non-blocking: bump the epoch so the worker exits at its next generation
    /// boundary, and mark the engine stopped immediately for the UI.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn stop(&mut self) {
        self.shared.epoch.fetch_add(1, Ordering::SeqCst);
        self.shared.running.store(false, Ordering::SeqCst);
    }

    /// No-op on native; the UI calls this each frame on wasm.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn tick(&mut self) {}

    #[cfg(target_arch = "wasm32")]
    pub fn start(&mut self, parts: Vec<Part>, config: NestConfig) {
        self.stop();
        self.shared.invalid.store(false, Ordering::Relaxed);
        *self.shared.best.lock().unwrap() = None;
        let nester = Nester::new(parts, config);
        *self.shared.slots.lock().unwrap() = nester.sheet_slots().to_vec();
        if !nester.ready {
            self.shared.invalid.store(true, Ordering::Relaxed);
            return;
        }
        *self.shared.best.lock().unwrap() = Some(nester.best.clone());
        self.shared.generation.store(nester.generation, Ordering::Relaxed);
        self.nester = Some(nester);
        self.shared.running.store(true, Ordering::Relaxed);
    }

    #[cfg(target_arch = "wasm32")]
    pub fn stop(&mut self) {
        self.shared.running.store(false, Ordering::Relaxed);
        self.nester = None;
    }

    /// Advance the optimizer a few generations (wasm, single-threaded).
    #[cfg(target_arch = "wasm32")]
    pub fn tick(&mut self) {
        if !self.is_running() {
            return;
        }
        if let Some(nester) = self.nester.as_mut() {
            // A small batch per frame keeps the browser responsive.
            for _ in 0..2 {
                nester.step();
            }
            *self.shared.best.lock().unwrap() = Some(nester.best.clone());
            self.shared.generation.store(nester.generation, Ordering::Relaxed);
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop();
    }
}
