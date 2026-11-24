//! A counting `ArrayBuffer` allocator with a per-call budget.
//!
//! ArrayBuffer backing stores live outside the V8 heap, so neither the JS
//! heap limit nor the WebAssembly memory guard sees them: a guest doing
//! `js.ArrayBuffer.new(2**30)` in a loop could otherwise take the whole host.
//! Every isolate gets its own allocator that counts live bytes. While a call
//! runs, live bytes may grow at most `allowance` past the level at the start
//! of the call (long-lived buffers Pyodide already holds don't count against
//! the guest). An allocation past that fails, which JS reports as a
//! `RangeError`, and the refusal is recorded so the call can be failed.

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Shared between the allocator V8 owns and the runtime that sets budgets.
#[derive(Debug, Default)]
pub(crate) struct ArrayBufferBudget {
    used: AtomicUsize,
    /// Highest `used` allowed; `usize::MAX` means unlimited.
    ceiling: AtomicUsize,
    denials: AtomicUsize,
}

impl ArrayBufferBudget {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            used: AtomicUsize::new(0),
            ceiling: AtomicUsize::new(usize::MAX),
            denials: AtomicUsize::new(0),
        })
    }

    /// Allow live bytes to grow by `allowance` from now; `None` lifts it.
    pub(crate) fn set_allowance(&self, allowance: Option<usize>) {
        let ceiling = allowance.map_or(usize::MAX, |allowance| {
            self.used.load(Ordering::SeqCst).saturating_add(allowance)
        });
        self.ceiling.store(ceiling, Ordering::SeqCst);
    }

    /// Allocations refused since the last call.
    pub(crate) fn take_denials(&self) -> usize {
        self.denials.swap(0, Ordering::SeqCst)
    }

    #[cfg(test)]
    pub(crate) fn used(&self) -> usize {
        self.used.load(Ordering::SeqCst)
    }

    fn reserve(&self, len: usize) -> bool {
        let ceiling = self.ceiling.load(Ordering::SeqCst);
        let mut current = self.used.load(Ordering::SeqCst);
        loop {
            let Some(next) = current.checked_add(len).filter(|next| *next <= ceiling) else {
                self.denials.fetch_add(1, Ordering::SeqCst);
                return false;
            };
            match self
                .used
                .compare_exchange(current, next, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => return true,
                Err(actual) => current = actual,
            }
        }
    }

    fn release(&self, len: usize) {
        self.used.fetch_sub(len, Ordering::SeqCst);
    }
}

/// The allocator for a new isolate, drawing on `budget`.
pub(crate) fn allocator(budget: &Arc<ArrayBufferBudget>) -> v8::UniqueRef<v8::Allocator> {
    static VTABLE: v8::RustAllocatorVtable<ArrayBufferBudget> = v8::RustAllocatorVtable {
        allocate,
        allocate_uninitialized,
        free,
        drop,
    };
    let handle = Arc::into_raw(Arc::clone(budget));
    // SAFETY: `handle` comes from `Arc::into_raw` and stays valid until V8
    // calls `drop`, which turns it back into the Arc exactly once; the vtable
    // functions only ever read through it.
    unsafe { v8::new_rust_allocator(handle, &VTABLE) }
}

unsafe extern "C" fn allocate(budget: &ArrayBufferBudget, len: usize) -> *mut c_void {
    if !budget.reserve(len) {
        return std::ptr::null_mut();
    }
    // SAFETY: plain libc allocation; a zero-length request still gets a
    // unique pointer so V8 never mistakes it for a failure.
    let data = unsafe { libc::calloc(len.max(1), 1) };
    if data.is_null() {
        budget.release(len);
    }
    data
}

unsafe extern "C" fn allocate_uninitialized(budget: &ArrayBufferBudget, len: usize) -> *mut c_void {
    if !budget.reserve(len) {
        return std::ptr::null_mut();
    }
    // SAFETY: as in `allocate`; V8 asked for uninitialized memory.
    let data = unsafe { libc::malloc(len.max(1)) };
    if data.is_null() {
        budget.release(len);
    }
    data
}

unsafe extern "C" fn free(budget: &ArrayBufferBudget, data: *mut c_void, len: usize) {
    // SAFETY: `data` came from `allocate`/`allocate_uninitialized` above,
    // which used libc's allocator; V8 frees each pointer once with its length.
    unsafe { libc::free(data) };
    budget.release(len);
}

unsafe extern "C" fn drop(budget: *const ArrayBufferBudget) {
    // SAFETY: `budget` is the pointer `allocator` produced with
    // `Arc::into_raw`; V8 calls `drop` once when it releases the allocator.
    std::mem::drop(unsafe { Arc::from_raw(budget) });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowance_counts_from_the_level_at_the_start_of_the_call() {
        let budget = ArrayBufferBudget::new();
        assert!(budget.reserve(1000), "unlimited outside a call");
        budget.set_allowance(Some(100));
        assert!(budget.reserve(60));
        assert!(!budget.reserve(60), "past the allowance");
        assert_eq!(budget.take_denials(), 1);
        budget.release(60);
        assert!(budget.reserve(100), "freed bytes are available again");
        budget.set_allowance(None);
        assert!(budget.reserve(usize::MAX - budget.used()));
        assert_eq!(budget.take_denials(), 0);
    }
}
