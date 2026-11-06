//! `--memory-limit-mib`: a process caps its own address space, for a caller
//! that shares a container with it and cannot give it a cgroup of its own.

/// Applies the cap, or exits when the kernel refuses it: a caller relying on
/// the cap must not get a process running without it.
pub(crate) fn apply(limit_mib: Option<u64>) {
    let Some(limit_mib) = limit_mib else {
        return;
    };
    if let Err(error) = set_address_space_limit(limit_mib) {
        eprintln!("Error: cannot apply --memory-limit-mib {limit_mib}: {error}");
        std::process::exit(crate::EXIT_TOOLING_ERROR);
    }
}

#[cfg(target_os = "linux")]
fn set_address_space_limit(limit_mib: u64) -> std::io::Result<()> {
    let bytes = limit_mib.saturating_mul(1024 * 1024);
    let limit = libc::rlimit {
        rlim_cur: bytes,
        rlim_max: bytes,
    };
    // SAFETY: `limit` is a fully initialized `rlimit` that outlives the call,
    // and `setrlimit` only reads it.
    if unsafe { libc::setrlimit(libc::RLIMIT_AS, &raw const limit) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

// Keeps the Linux twin's signature, so `apply` has one shape.
#[cfg(not(target_os = "linux"))]
#[allow(clippy::unnecessary_wraps)]
fn set_address_space_limit(_limit_mib: u64) -> std::io::Result<()> {
    eprintln!("Warning: --memory-limit-mib only takes effect on Linux, running without it");
    Ok(())
}
