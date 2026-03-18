// crabtop: A memory-safe Rust system monitor
// Security improvements over the C++ original:
//   1. Memory safety guaranteed by Rust's borrow checker — no buffer overflows
//   2. Privilege model: capabilities dropped immediately after collection setup
//   3. No SUID binary option — only fine-grained Linux capabilities via setcap
//   4. Release signing via GitHub Actions + sigstore/cosign (see .github/workflows)
//   5. SECURITY.md with responsible disclosure policy
//   6. overflow-checks = true in release profile
//   7. panic = "abort" — no stack unwinding attack surface
//
// v1.1 changes:
//   - current_thread tokio runtime (1 OS thread vs 12 with rt-multi-thread)
//   - Arc<Vec<>> for disk/temp cache — clone is a refcount bump, not a heap copy
//   - Direct /proc process reading — sysinfo System dropped entirely
//   - Smart process sort — only re-sorts when top data actually changes

mod app;
mod capabilities;
mod collect;
mod config;
mod error;
mod input;
mod theme;
mod ui;

use anyhow::Result;
use clap::Parser;
use tracing::{error, info};

use app::App;
use capabilities::{assert_not_suid, drop_privileges};
use config::Config;

/// crabtop — a memory-safe system resource monitor
#[derive(Parser, Debug)]
#[command(name = "crabtop", version, about)]
pub struct Cli {
    /// Start with a specific preset (0–9)
    #[arg(short, long, value_name = "ID", value_parser = clap::value_parser!(u8).range(0..=9))]
    preset: Option<u8>,

    /// Force TTY mode (max 16 colors)
    #[arg(short, long)]
    tty: bool,

    /// Update interval in milliseconds (100–86400000)
    #[arg(short, long, default_value = "2000",
          value_parser = clap::value_parser!(u64).range(100..=86_400_000))]
    interval: u64,

    /// Config file path (default: ~/.config/crabtop/crabtop.toml)
    #[arg(short, long)]
    config: Option<std::path::PathBuf>,

    /// Enable debug logging
    #[arg(long)]
    debug: bool,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // ── SUID check — must be FIRST, before any other code runs ───────────────
    // Bug fix: assert_not_suid() was defined but never called. Without this,
    // the SUID rejection policy documented in SECURITY.md was not enforced.
    assert_not_suid()?;

    // ── Logging ──────────────────────────────────────────────────────────────
    let log_level = if cli.debug { "debug" } else { "warn" };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| log_level.into()),
        )
        .init();

    info!("crabtop starting — version {}", env!("CARGO_PKG_VERSION"));

    // ── Configuration ────────────────────────────────────────────────────────
    let config = Config::load(cli.config.as_deref()).unwrap_or_else(|e| {
        error!("Config load error (using defaults): {e}");
        Config::default()
    });

    // ── Privilege Drop ───────────────────────────────────────────────────────
    // Drop all Linux capabilities we don't need BEFORE entering the UI.
    // crabtop only needs CAP_SYS_PTRACE (to read other users' process info)
    // and only if running as root. We never set SUID.
    //
    // Security note: unlike the original btop which offers a `make setuid`
    // option granting full root, crabtop uses fine-grained capabilities only.
    // Prefer `sudo setcap cap_sys_ptrace+ep /usr/local/bin/crabtop` for
    // installation; no SUID mechanism is provided or supported.
    #[cfg(feature = "capabilities")]
    {
        if let Err(e) = drop_privileges() {
            // Non-fatal: we just won't see other users' processes
            error!("Could not drop capabilities: {e}. Some process info may be unavailable.");
        }
    }

    // ── Run App ──────────────────────────────────────────────────────────────
    let mut app = App::new(config, cli.preset.unwrap_or(0), cli.tty, cli.interval).await?;
    app.run().await?;

    Ok(())
}
