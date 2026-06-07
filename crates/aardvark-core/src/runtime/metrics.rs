pub(super) fn bytes_from_mb(value: u64) -> usize {
    const MIB: usize = 1024 * 1024;
    (value as usize).saturating_mul(MIB)
}

pub(super) fn thread_cpu_time_ns() -> Option<u64> {
    #[cfg(unix)]
    {
        use std::mem::MaybeUninit;

        let which = thread_rusage_scope();
        let mut usage = MaybeUninit::<libc::rusage>::uninit();
        // SAFETY: `usage` points to writable storage for `libc::rusage`; it is
        // only read with `assume_init` after `getrusage` reports success.
        unsafe {
            if libc::getrusage(which, usage.as_mut_ptr()) != 0 {
                return None;
            }
            let usage = usage.assume_init();
            let user = timeval_to_ns(usage.ru_utime);
            let sys = timeval_to_ns(usage.ru_stime);
            Some(user.saturating_add(sys))
        }
    }
    #[cfg(not(unix))]
    {
        None
    }
}

#[cfg(unix)]
fn timeval_to_ns(tv: libc::timeval) -> u64 {
    let secs = tv.tv_sec as i128;
    let micros = tv.tv_usec as i128;
    let total = secs
        .saturating_mul(1_000_000_000)
        .saturating_add(micros.saturating_mul(1_000));
    if total < 0 {
        0
    } else {
        total as u64
    }
}

#[cfg(unix)]
fn thread_rusage_scope() -> libc::c_int {
    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "openbsd",
        target_os = "netbsd"
    ))]
    {
        libc::RUSAGE_THREAD
    }
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        libc::RUSAGE_SELF
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "macos",
        target_os = "ios"
    )))]
    {
        libc::RUSAGE_SELF
    }
}

pub(super) fn ns_to_ms(value: u64) -> u64 {
    value.div_ceil(1_000_000)
}
