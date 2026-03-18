# crabtop

A memory-safe Rust rewrite of [btop++](https://github.com/aristocratos/btop), the terminal resource monitor.

```
┌─────────────────── crabtop v1.0.0  render: on-change  Uptime: 33h 49m ────┐
│ CPU: AMD Ryzen 5 5600U                    │ MEM: 14.4 GiB total            │
│ ████████░░░░░░░░░░░░░░░░░░  7.0%          │ RAM  5.6G / 14.4G  (38.8%)     │
│ ▁▂▄▃▅▄▆▅▄▃▄▅▃▄▂▃ (history)               │ SWAP 0.0G / 14.8G  ( 0.0%)     │
│ C0 C1 C2 C3 C4 C5 C6 C7 C8 C9 C10 C11   │                                 │
├─ TEMPS ────────────────────────────────────────────────────────────────────┤
│ Sensor    Load                                                   Temp       │
│ CPU       [████████████████████████████████░░░░░░░░░░░░░░░░░░]  48.4°C    │
│ System    [███████████████████████████████░░░░░░░░░░░░░░░░░░░░]  48.0°C   │
│ ThinkPad  [███████████████████████████████░░░░░░░░░░░░░░░░░░░░]  48.0°C   │
│ GPU       [█████████████████████████████░░░░░░░░░░░░░░░░░░░░░░]  46.0°C   │
│ SSD       [██████████████████████░░░░░░░░░░░░░░░░░░░░░░░░░░░░░]  38.8°C   │
│ WiFi      [█████████████████████░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░]  38.0°C   │
├─ PROCESSES (331) ──────────────────────────────────┬─ DISK ───────────────┤
│ PID     Name              CPU%  MEM      Thd  User │ Mount     %   Size    │
│ ▶ 1368  kwin_wayland      1.2   270.3M   28   alice│ /         9%  452.9G  │
│   219296 steamwebhelper   1.0   193.8M   29   alice│ /boot/efi 1%  1.0G    │
│   219374 steamwebhelper   0.8   561.1M   20   alice├─ NET ────────────────┤
│   1575  plasmashell       0.6   554.9M   156  alice│ Interface  ↓      ↑   │
│   1436  Xwayland          0.2   102.9M   14   alice│ enp3s0f0  0B/s  0B/s  │
│   217857 konsole          0.1   162.0M   16   alice│ wlp6s0    78B/s 301B/s│
│   219054 steam            0.1   270.0M   60   alice│ enx6c6e07 0B/s  0B/s  │
└────────────────────────────────────────────────────┴──────────────────────┘
```

## Why crabtop?

| Issue in btop C++ | crabtop solution |
|---|---|
| C++ memory unsafety (buffer overflows, UAF) | Rust borrow checker: entire class eliminated |
| `make setuid` grants full root to binary | SUID **not supported** — binary refuses to run with SUID bit |
| No release signing | Every binary signed with sigstore/cosign (keyless, OIDC) |
| No SECURITY.md | Full responsible disclosure policy in [SECURITY.md](./SECURITY.md) |
| No formal vulnerability process | GitHub Security Advisories + 90-day coordinated disclosure |
| No `cargo audit` in CI | Security audit runs on every push and PR |
| Integer overflow possible in release | `overflow-checks = true` in release profile |
| Render rate tied to collection rate | v2: fixed 10fps render loop independent of `--interval` |

## Architecture

```
┌─────────────────────────────────────┐
│          tokio select! loop          │
│                                      │
│  collect_tick (--interval) ───────── │──► /proc/stat, /proc/meminfo → draw()
│  input events (immediate)  ───────── │──► keyboard/mouse → draw() if changed
└─────────────────────────────────────┘
```

There is no separate render timer. `terminal.draw()` is called exactly once per collect tick and once per meaningful input event — never on a fixed interval. This means at `--interval 2000`, the terminal redraws twice per second (once per data update) rather than 10 times per second, eliminating 80% of the render work. `MissedTickBehavior::Skip` prevents burst collections if a tick is slow.

---

## Installation

### From GitHub Releases (recommended)

```bash
# Download the binary for your platform
curl -LO https://github.com/your-org/crabtop/releases/latest/download/crabtop-linux-x86_64
chmod +x crabtop-linux-x86_64
sudo mv crabtop-linux-x86_64 /usr/local/bin/crabtop
```

### Verify the signature (strongly recommended)

```bash
# Install cosign: https://docs.sigstore.dev/cosign/installation/

# Download signature + cert alongside the binary
curl -LO https://github.com/your-org/crabtop/releases/latest/download/crabtop-linux-x86_64.sig
curl -LO https://github.com/your-org/crabtop/releases/latest/download/crabtop-linux-x86_64.cert

# Verify
cosign verify-blob \
  --certificate crabtop-linux-x86_64.cert \
  --signature crabtop-linux-x86_64.sig \
  --certificate-identity "https://github.com/your-org/crabtop/.github/workflows/release.yml@refs/tags/v1.0.0" \
  --certificate-oidc-issuer "https://token.actions.githubusercontent.com" \
  crabtop-linux-x86_64
# Expected output: Verified OK
```

### From source

```bash
# Requires Rust 1.75+: https://rustup.rs
git clone https://github.com/your-org/crabtop
cd crabtop
cargo build --release
sudo cp target/release/crabtop /usr/local/bin/
```

---

## Privilege Setup

crabtop works without elevated privileges — you'll see your own processes and all
system metrics (CPU, memory, disk, network).

To also see processes owned by other users:

```bash
# Grant only the capability we need — nothing more
sudo setcap cap_sys_ptrace+ep /usr/local/bin/crabtop

# Verify
getcap /usr/local/bin/crabtop
# crabtop = cap_sys_ptrace+ep

# Revoke if desired
sudo setcap -r /usr/local/bin/crabtop
```

**Never** use `chmod +s` (SUID) — crabtop detects this and refuses to run.

---

## Keybindings

| Key | Action |
|-----|--------|
| `q` / `Ctrl-C` | Quit |
| `Tab` / `Shift-Tab` | Cycle layout presets |
| `1–4` | Focus CPU / Memory / Network / Processes box |
| `↑` / `↓` (or `k` / `j`) | Scroll process list |
| `c` `m` `p` `n` `t` | Sort by CPU / Memory / PID / Name / Threads |
| `r` | Reverse sort order |
| `e` | Toggle process tree view |
| `K` | Send SIGTERM to selected process |
| `T` | Cycle color themes |
| `R` / `F5` | Force refresh |
| `?` / `F1` | Help |

---

## Themes

Built-in: `default`, `dracula`, `gruvbox`

Set in config:
```toml
# ~/.config/crabtop/crabtop.toml
theme = "dracula"
```

---

## Configuration

```toml
# ~/.config/crabtop/crabtop.toml

theme = "default"
update_interval_ms = 2000
show_swap = true
show_io_stat = true
proc_sorting = "cpu"     # cpu | memory | pid | name | threads
proc_reversed = false
proc_tree = false
cpu_single_graph = false
net_auto = true
vim_keys = false
```

## License

Apache-2.0 — same as the original btop++.
