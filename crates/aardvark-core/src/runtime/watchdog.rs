use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};
use std::time::Duration;

use crate::engine::JsRuntime;

pub(super) struct WallClockGuard {
    fired: Arc<AtomicBool>,
    cancel_tx: Option<mpsc::Sender<()>>,
}

impl WallClockGuard {
    pub(super) fn new(handle: v8::IsolateHandle, requested_ms: u64) -> Self {
        let (tx, rx) = mpsc::channel();
        let fired = Arc::new(AtomicBool::new(false));
        let thread_handle = handle.clone();
        let fired_clone = fired.clone();
        std::thread::spawn(
            move || match rx.recv_timeout(Duration::from_millis(requested_ms)) {
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    fired_clone.store(true, Ordering::SeqCst);
                    thread_handle.terminate_execution();
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {}
            },
        );
        Self {
            fired,
            cancel_tx: Some(tx),
        }
    }

    pub(super) fn complete(mut self, runtime: &mut JsRuntime) -> bool {
        if let Some(tx) = self.cancel_tx.take() {
            let _ = tx.send(());
        }
        let fired = self.fired.load(Ordering::SeqCst);
        runtime.cancel_terminate_execution();
        fired
    }
}
