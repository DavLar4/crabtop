/// capabilities.rs — Hardened privilege model for crabtop
///
/// Security design (vs original btop C++):
/// ─────────────────────────────────────────
/// Original btop offers three privilege options:
///   1. `make setuid` — grants full root to the binary (HIGH RISK)
///   2. `make setcap` — grants CAP_SYS_PTRACE (safer)
///   3. `sudo btop`   — full root at runtime (HIGH RISK)
///
/// crabtop:
///   • Completely removes the SUID path — no mechanism to set SUID is provided.
///   • Uses only `setcap cap_sys_ptrace+ep` for elevated process visibility.
///   • Immediately drops all capabilities after the system info library
///     initialises, using PR_SET_NO_NEW_PRIVS to prevent re-escalation.
///   • Non-root users get process data for their own processes only — no crash,
///     no silent data corruption, just graceful degradation.

#[cfg(feature = "capabilities")]
use caps::{CapSet, Capability};
use anyhow::Result;
use tracing::{debug, warn};

/// Drop all Linux capabilities except the narrow set we actually need,
/// then lock down further privilege escalation with PR_SET_NO_NEW_PRIVS.
///
/// Call this once — before entering the TUI event loop — so that the
/// long-running UI loop runs with minimal privilege.
pub fn drop_privileges() -> Result<()> {
    #[cfg(feature = "capabilities")]
    {
        use caps::CapsHashSet;

        // Determine what we currently have
        let current = caps::read(None, CapSet::Permitted)?;
        debug!("Capabilities on entry: {:?}", current);

        // We only keep CAP_SYS_PTRACE and only if we actually have it.
        // All other capabilities are cleared from Permitted, Effective,
        // and Inheritable sets.
        let mut keep = CapsHashSet::new();
        if current.contains(&Capability::CAP_SYS_PTRACE) {
            keep.insert(Capability::CAP_SYS_PTRACE);
            debug!("Retaining CAP_SYS_PTRACE for process info collection");
        }

        // Clear everything we don't need
        let drop_set: CapsHashSet = current
            .iter()
            .filter(|c| !keep.contains(c))
            .cloned()
            .collect();

        for cap in &drop_set {
            // Drop from all three sets
            let _ = caps::drop(None, CapSet::Effective, *cap);
            let _ = caps::drop(None, CapSet::Permitted, *cap);
            let _ = caps::drop(None, CapSet::Inheritable, *cap);
        }

        debug!("Dropped {} capabilities", drop_set.len());

        // Set PR_SET_NO_NEW_PRIVS — prevents any child process or future
        // execve from regaining capabilities. This is a one-way door.
        set_no_new_privs()?;

        let remaining = caps::read(None, CapSet::Effective)?;
        debug!("Remaining effective capabilities: {:?}", remaining);
    }

    #[cfg(not(feature = "capabilities"))]
    {
        warn!("Compiled without 'capabilities' feature — privilege drop skipped");
    }

    Ok(())
}

/// Syscall wrapper for PR_SET_NO_NEW_PRIVS (Linux 3.5+).
/// After this call, the process and all descendants cannot gain new privileges
/// via setuid, setgid, or file capabilities — regardless of what files they exec.
#[cfg(target_os = "linux")]
fn set_no_new_privs() -> Result<()> {
    // Safety: prctl is a standard Linux syscall. The arguments are
    // well-defined constants with no memory aliasing concerns.
    let ret = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
    if ret != 0 {
        let e = std::io::Error::last_os_error();
        // Non-fatal on kernels < 3.5 or certain container runtimes
        warn!("PR_SET_NO_NEW_PRIVS failed: {e}");
    } else {
        debug!("PR_SET_NO_NEW_PRIVS set — no further privilege escalation possible");
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn set_no_new_privs() -> Result<()> {
    // macOS/BSD: use pledge/unveil equivalents or no-op
    debug!("PR_SET_NO_NEW_PRIVS not available on this platform");
    Ok(())
}

/// Installation instructions printed with --help or on capability errors.
#[allow(dead_code)]
pub const CAPABILITY_SETUP_INSTRUCTIONS: &str = r#"
crabtop privilege setup
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
To see processes owned by other users, grant the narrow
CAP_SYS_PTRACE capability to the binary:

    sudo setcap cap_sys_ptrace+ep /usr/local/bin/crabtop

NEVER use SUID (chmod +s) — crabtop does not support it
and will refuse to run with the SUID bit set.

To verify capabilities:
    getcap /usr/local/bin/crabtop
    # Should print: crabtop = cap_sys_ptrace+ep

To remove all capabilities:
    sudo setcap -r /usr/local/bin/crabtop
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
"#;

/// Refuse to run if the SUID bit is set on our own binary.
/// This hard-codes our security policy: we do not support SUID escalation.
pub fn assert_not_suid() -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let path = std::env::current_exe()?;
        let meta = std::fs::metadata(&path)?;
        // S_ISUID = 0o4000
        if meta.mode() & 0o4000 != 0 {
            anyhow::bail!(
                "crabtop detected SUID bit on its binary ({}).\n\
                 crabtop does not support SUID execution — this is a security risk.\n\
                 Remove the SUID bit: sudo chmod -s {}\n\
                 Use setcap instead: sudo setcap cap_sys_ptrace+ep {}",
                path.display(), path.display(), path.display()
            );
        }
    }
    Ok(())
}
