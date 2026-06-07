#[cfg(unix)]
#[cfg(target_os = "macos")]
pub(super) fn current_rss_mib() -> Option<f64> {
    // SAFETY: `MACH_TASK_SELF` is the libSystem task port for the current
    // process, and `info`/`count` point to writable storage sized for
    // `mach_task_basic_info`.
    unsafe {
        unsafe extern "C" {
            #[link_name = "mach_task_self_"]
            static MACH_TASK_SELF: libc::mach_port_t;
        }
        let mut info: libc::mach_task_basic_info = std::mem::zeroed();
        let mut count = (std::mem::size_of::<libc::mach_task_basic_info>()
            / std::mem::size_of::<libc::integer_t>())
            as libc::mach_msg_type_number_t;
        let result = libc::task_info(
            MACH_TASK_SELF,
            libc::MACH_TASK_BASIC_INFO,
            (&mut info as *mut _) as *mut libc::integer_t,
            &mut count,
        );
        if result == libc::KERN_SUCCESS {
            Some(info.resident_size as f64 / (1024.0 * 1024.0))
        } else {
            None
        }
    }
}

#[cfg(target_os = "linux")]
pub(super) fn current_rss_mib() -> Option<f64> {
    let contents = std::fs::read_to_string("/proc/self/statm").ok()?;
    let resident_pages: f64 = contents.split_whitespace().nth(1)?.parse().ok()?;
    // SAFETY: `sysconf(_SC_PAGESIZE)` has no pointer arguments and only reads
    // the process configuration reported by libc.
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as f64;
    Some(resident_pages * page_size / (1024.0 * 1024.0))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(super) fn current_rss_mib() -> Option<f64> {
    None
}
