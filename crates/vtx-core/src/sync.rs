//! Synchronization types.
//!
//! Most importantly, a fair mutex is included.

use parking_lot::{Mutex, MutexGuard};

/// A fair mutex.
///
/// Uses an extra lock to ensure that if one thread is waiting that it will
/// get the lock before a single thread can re-lock it.
///
/// This protects the terminal against UI starvation: the PTY reader thread
/// processing output would otherwise be able to re-acquire the terminal lock
/// between two UI redraws indefinitely.
pub struct FairMutex<T> {
    /// Data.
    data: Mutex<T>,
    /// Next-to-access.
    next: Mutex<()>,
}

impl<T> FairMutex<T> {
    /// Create a new fair mutex.
    pub fn new(data: T) -> FairMutex<T> {
        FairMutex {
            data: Mutex::new(data),
            next: Mutex::new(()),
        }
    }

    /// Acquire a lease to reserve the mutex lock.
    ///
    /// This will prevent others from acquiring a terminal lock, but block if
    /// anyone else is already holding a lease.
    pub fn lease(&self) -> MutexGuard<'_, ()> {
        self.next.lock()
    }

    /// Lock the mutex.
    pub fn lock(&self) -> MutexGuard<'_, T> {
        // Must bind to a temporary or the lock will be freed before going
        // into data.lock().
        let _next = self.next.lock();
        self.data.lock()
    }

    /// Unfairly lock the mutex.
    pub fn lock_unfair(&self) -> MutexGuard<'_, T> {
        self.data.lock()
    }

    /// Unfairly try to lock the mutex.
    pub fn try_lock_unfair(&self) -> Option<MutexGuard<'_, T>> {
        self.data.try_lock()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn lock_mutates_data() {
        let mutex = FairMutex::new(1);

        {
            let mut data = mutex.lock();
            *data += 1;
        }
        {
            let mut data = mutex.lock();
            *data += 1;
        }

        assert_eq!(*mutex.lock_unfair(), 3);
    }

    #[test]
    fn try_lock_unfair_fails_while_locked() {
        let mutex = FairMutex::new(());

        let guard = mutex.lock();
        assert!(mutex.try_lock_unfair().is_none());
        drop(guard);
        assert!(mutex.try_lock_unfair().is_some());
    }

    #[test]
    fn unfair_lock_bypasses_lease_queue() {
        let mutex = FairMutex::new(1);

        // A lease reserves the fair path; callers which already own
        // consistent state may still take the unfair fast path.
        let _lease = mutex.lease();
        assert_eq!(*mutex.lock_unfair(), 1);
    }

    #[test]
    fn concurrent_locks_preserve_data() {
        const THREADS: usize = 4;
        const INCREMENTS: usize = 1_000;

        let mutex = FairMutex::new(0u64);
        thread::scope(|scope| {
            for _ in 0..THREADS {
                scope.spawn(|| {
                    for _ in 0..INCREMENTS {
                        *mutex.lock() += 1;
                    }
                });
            }
        });

        assert_eq!(*mutex.lock_unfair(), (THREADS * INCREMENTS) as u64);
    }
}
