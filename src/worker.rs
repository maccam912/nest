//! Drives the [`Nester`] continuously and publishes results to the UI.
//!
//! Native builds run the optimizer on a background thread using a rayon pool
//! sized to the requested core count. Wasm builds (no threads) advance the
//! optimizer incrementally from the UI loop via [`Engine::tick`].

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::model::{NestConfig, NestResult, Part, SheetSlot};
use crate::nest::Nester;

#[derive(Default)]
struct Shared {
    running: AtomicBool,
    generation: AtomicU64,
    best: Mutex<Option<NestResult>>,
    slots: Mutex<Vec<SheetSlot>>,
    /// Set when a started run had nothing to nest.
    invalid: AtomicBool,
}

pub struct Engine {
    shared: Arc<Shared>,
    #[cfg(not(target_arch = "wasm32"))]
    handle: Option<std::thread::JoinHandle<()>>,
    #[cfg(target_arch = "wasm32")]
    nester: Option<Nester>,
}

impl Default for Engine {
    fn default() -> Self {
        Self {
            shared: Arc::new(Shared::default()),
            #[cfg(not(target_arch = "wasm32"))]
            handle: None,
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
        self.stop();
        let shared = self.shared.clone();
        shared.invalid.store(false, Ordering::Relaxed);
        shared.running.store(true, Ordering::Relaxed);
        *shared.best.lock().unwrap() = None;

        let threads = config.threads;
        self.handle = Some(std::thread::spawn(move || {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads) // 0 = all cores
                .build()
                .ok();

            let run = || {
                let mut nester = Nester::new(parts, config);
                *shared.slots.lock().unwrap() = nester.sheet_slots().to_vec();
                if !nester.ready {
                    shared.invalid.store(true, Ordering::Relaxed);
                    shared.running.store(false, Ordering::Relaxed);
                    return;
                }
                *shared.best.lock().unwrap() = Some(nester.best.clone());
                shared.generation.store(nester.generation, Ordering::Relaxed);
                while shared.running.load(Ordering::Relaxed) {
                    nester.step();
                    *shared.best.lock().unwrap() = Some(nester.best.clone());
                    shared.generation.store(nester.generation, Ordering::Relaxed);
                }
            };

            match pool {
                Some(p) => p.install(run),
                None => run(),
            }
        }));
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn stop(&mut self) {
        self.shared.running.store(false, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
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
