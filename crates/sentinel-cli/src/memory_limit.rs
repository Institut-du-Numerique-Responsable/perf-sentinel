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
    use std::os::unix::process::CommandExt;

    let bytes = limit_mib.saturating_mul(1024 * 1024);
    let mut current = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `current` is a fully initialized `rlimit` that outlives the call,
    // and `getrlimit` only writes to it.
    if unsafe { libc::getrlimit(libc::RLIMIT_AS, &raw mut current) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // In force since this process started: the restart below already happened,
    // or the parent set the same limit.
    if current.rlim_cur == bytes {
        return Ok(());
    }
    let limit = libc::rlimit {
        rlim_cur: bytes,
        rlim_max: bytes,
    };
    // SAFETY: `limit` is a fully initialized `rlimit` that outlives the call,
    // and `setrlimit` only reads it.
    if unsafe { libc::setrlimit(libc::RLIMIT_AS, &raw const limit) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // The cap only refuses new mappings, and the musl release builds' mimalloc
    // has already reserved 1 GiB: under a lower cap it would keep allocating
    // from that arena past the cap while a new thread could not map its stack.
    // Restarting makes every reservation under the cap. Nothing was read or
    // written yet, and the pid stays the same for the caller.
    let mut args = std::env::args_os();
    let argv0 = args.next().unwrap_or_default();
    Err(std::process::Command::new("/proc/self/exe")
        .arg0(argv0)
        .args(args)
        .exec())
}

// Keeps the Linux twin's signature, so `apply` has one shape.
#[cfg(not(target_os = "linux"))]
#[allow(clippy::unnecessary_wraps)]
fn set_address_space_limit(_limit_mib: u64) -> std::io::Result<()> {
    eprintln!("Warning: --memory-limit-mib only takes effect on Linux, running without it");
    Ok(())
}
