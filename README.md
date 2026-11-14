# SC Overlay (`sc_overlay_rust`)

A fast, strictly non-intrusive **Star Citizen** nameplate, organization intelligence, citizen dossier, and mining signal overlay written in **100% Rust** — built natively for **both Windows 11 and Bazzite Linux** (Fedora Atomic / Wayland / KDE Plasma / GNOME / XWayland).

Inspired by and **100% configuration-compatible** with [Niteb1rd/IRIS](https://github.com/Niteb1rd/IRIS).

---

## Key Features

- **Strictly Non-Intrusive (EAC Safe by Design)**
  - Reads pixels from the desktop/window compositor, draws a click-through transparent overlay window above the screen, and queries public `https://robertsspaceindustries.com` pages.
  - **Never** attaches to `StarCitizen.exe`, **never** reads or writes process memory, **never** injects DLLs/Vulkan layers, and **never** sends synthetic keyboard or mouse input.
- **Real-Time Nameplate OCR & Sub-Pixel Tracking**
  - Detects Star Citizen `<` chevron + handle nameplates inside a configurable screen boundary.
  - Uses IRIS's embedded 2-layer MLP neural glyph classifier (`letters-model.bin`), sub-pixel + multi-frame log-likelihood `O` vs `0` disambiguator, `_` vs `-` vertical rule, chromatic aberration detector & mitigator, multi-frame consensus, and Hungarian + velocity-extrapolated tracker.
- **4-Tier Color & Bracket Intelligence**
  - **Tier 1 — Player List `{Handle}`** *(curly brackets)*: Individual player overrides.
  - **Tier 2 — Organization List `[Handle]`** *(square brackets)*: Priority-ranked org SID rules (`1..9` Color Ranks strip).
  - **Tier 3 — Organization Attribute `<Handle>`** *(angle brackets)*: Priority-ranked rules across all 6 RSI Archetypes, 15 RSI Activities, `Redacted`, `HiddenMainOrg`, and `NoOrg`.
  - **Tier 4 — Fallback `<Handle>`**: Configurable default color for unmatched citizens.
- **Live Right-Side HUD Panel & Citizen Dossier (`Shift+0`)**
  - Live side panel shows confirmed nearby handles, main & affiliate orgs, classification reason, and the first line of your personal dossier note.
  - Press `Shift+0` for the full-screen **Interactive Citizen Dossier**: search any citizen handle, inspect UEE Citizen Record #, moniker, and 2-column org breakdown, right-click players or org cards to set colors on the spot, open RSI profile/org pages, and write persistent **Dossier Notes** (`+930` Star Citizen in-lore year timestamps, automatically preserved across handle renames via UEE Citizen Record #).
- **Mining HUD Signal Decoder**
  - Reads 4–6 digit radar signal values from Star Citizen's mining HUD and decodes ore type + cluster count (e.g. `Quantainium 2x`, `Bexalite`, `Gold`) using Star Citizen 4.7 signatures (`mining.txt`).
- **100% IRIS File Compatibility & Cloud Sync**
  - Drop-in compatible with IRIS `social-config.txt`, `settings.txt`, `mining.txt`, `exclusions.txt`, `dossier-notes.txt`, and `Data/cache.db` (SQLite WAL v3).
  - Built-in Import / Export and 24-hour **Hosted Config URL** sync (`Merge`, `MergeImportedWins`, or `BackupExisting`, with automatic GitHub blob → raw and Pastebin → raw URL normalization).

---

## Platform Architecture

| Subsystem | Windows 11 (`win32`) | Bazzite / Fedora Linux (`linux`) |
| :--- | :--- | :--- |
| **Game Window Detection** | Win32 `GetForegroundWindow` + process image check (`StarCitizen.exe`) + borderless client/monitor geometry | X11/XWayland `_NET_ACTIVE_WINDOW` (`StarCitizen.exe` / Wine / Proton `steam_app_*`) + KDE `kdotool` / Wayland fallback |
| **Screen Capture** | Primary: **DXGI Desktop Duplication** (`IDXGIOutputDuplication` GPU texture staging)<br>Secondary: **Windows Graphics Capture (WGC)** (`IGraphicsCaptureItemInterop`, optional borderless capture) | Primary (Wayland): **XDG Desktop Portal `ScreenCast` + PipeWire** (`ashpd` with saved `restore_token` + GStreamer `pipewiresrc`)<br>Fallback (X11/XWayland): Pure-Rust `x11rb` root/window `GetImage` |
| **Click-Through Overlay** | Win32 `WS_EX_LAYERED \| WS_EX_TRANSPARENT \| WS_EX_TOPMOST \| WS_EX_NOACTIVATE` + `UpdateLayeredWindow` (`ULW_ALPHA`) + `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` | Native Wayland **`zwlr_layer_shell_v1`** (`Layer::Overlay`, empty `wl_region` click-through, `KeyboardInteractivity::None` — supported natively on Bazzite KDE Plasma & Sway/Hyprland) + X11 `SHAPE` input-empty ARGB32 fallback |
| **Global Hotkeys** | Win32 `RegisterHotKey` + non-consuming `GetAsyncKeyState` poll for single-key `Tab` timed peek while Star Citizen is focused | X11 `GrabKey` + non-consuming `/dev/input` (`evdev`) listener + instant CLI IPC (`sc-overlay --toggle` / `--timed` / `--interactive`) for Wayland desktop shortcuts |
| **System Tray** | Native Win32 `Shell_NotifyIconW` popup menu | Pure-Rust StatusNotifierItem (`ksni` / D-Bus `zbus`) |

---

## Installation & Usage

> **Important:** Set Star Citizen to **Borderless** (Windowed Borderless) in Star Citizen's Graphics Settings on both Windows 11 and Linux. Exclusive Fullscreen bypasses desktop compositors.

### Windows 11

#### Run Pre-Built or Build from Source
Requires [Rust stable](https://rustup.rs/) (`x86_64-pc-windows-msvc`):

```powershell
git clone https://github.com/joesp/sc_overlay_rust.git
cd sc_overlay_rust
cargo build --release
.\target\release\sc-overlay.exe
```

- Closing the Settings window (`✕`) minimizes SC Overlay to the Windows system tray; use the **Quit** button or right-click the tray icon to exit completely.
- Pass `--portable` (or create a `Config` folder next to `sc-overlay.exe`) to store `Config/`, `Data/`, and `Logs/` alongside the executable instead of `%APPDATA%\sc-overlay`.

---

### Bazzite Linux (Fedora Atomic / KDE Plasma / GNOME)

Bazzite has an immutable `/usr` filesystem, so SC Overlay includes a zero-root installer ([packaging/linux/install-bazzite.sh](packaging/linux/install-bazzite.sh)) that installs cleanly into `~/.local/bin` and `~/.local/share`.

#### Option A: Install from Pre-Built Linux Artifact
```bash
chmod +x packaging/linux/install-bazzite.sh
./packaging/linux/install-bazzite.sh
```

#### Option B: Build from Source on Bazzite (via Distrobox) or Fedora Workstation
On standard Fedora Workstation:
```bash
sudo dnf install -y rust cargo gcc pkgconf-pkg-config \
    wayland-devel libxkbcommon-devel libX11-devel mesa-libGL-devel \
    gstreamer1-plugins-base gstreamer1-plugins-good pipewire-gstreamer
cargo build --release
./packaging/linux/install-bazzite.sh
```

On **Bazzite** (where PipeWire and GStreamer are already pre-installed on the host, and build compilers live inside Distrobox):
```bash
distrobox create --name fedora-dev --image registry.fedoraproject.org/fedora-toolbox:latest
distrobox enter fedora-dev -- bash -lc "sudo dnf install -y rust cargo gcc pkgconf-pkg-config wayland-devel libxkbcommon-devel libX11-devel mesa-libGL-devel && cd '$PWD' && cargo build --release"
./packaging/linux/install-bazzite.sh
```

#### Bazzite Wayland Setup Notes
1. **One-Time Screen Capture Grant**: On first detection of Star Citizen under Wayland, KDE/GNOME's XDG Portal prompts you to select the monitor/window and check **"Remember / Allow restore"**. SC Overlay persists the portal `restore_token` in `~/.config/sc-overlay/Config/settings.txt` so subsequent launches connect automatically.
2. **Wayland Global Shortcuts**: Wayland isolates keyboard input between apps for security. Bind global shortcuts in **KDE System Settings → Keyboard → Shortcuts → Add New → Command**:
   - `Shift+F12` → `~/.local/bin/sc-overlay --toggle`
   - `Shift+0` → `~/.local/bin/sc-overlay --interactive`
   - Timed Peek → `~/.local/bin/sc-overlay --timed`
3. **Optional Non-Consuming `Tab` Timed Peek**: If you want SC Overlay to observe single-key `Tab` presses directly on Wayland without consuming `Tab` from Star Citizen, grant your user `input` group access (`sudo usermod -aG input "$USER"` and log out/in).

---

## Default Hotkeys & CLI Flags

| Action | Default Hotkey | CLI Flag (signals running instance via IPC) |
| :--- | :--- | :--- |
| Toggle Overlay On / Off | `Shift+F12` | `sc-overlay --toggle` |
| Toggle Live Debug / Heartbeat HUD | *(Settings / Tray)* | `sc-overlay --debug` |
| Timed Overlay Peek (when disabled) | `Tab` | `sc-overlay --timed` |
| Interactive Citizen Dossier & Notes | `Shift+0` | `sc-overlay --interactive` |
| Open / Focus Settings Window | *(Tray click)* | `sc-overlay --settings` |
| Start Minimized to Tray | — | `sc-overlay --minimized` |
| Portable Mode (`./Config`, `./Data`, `./Logs`) | — | `sc-overlay --portable` |
| Stop Running Instance | — | `sc-overlay --quit` |

---

## Configuration & Data Locations

| OS | Mode | Config Directory | Cache & Data Directory |
| :--- | :--- | :--- | :--- |
| **Windows 11** | Standard | `%APPDATA%\sc-overlay\Config\` | `%LOCALAPPDATA%\sc-overlay\Data\` |
| **Bazzite / Linux** | Standard | `~/.config/sc-overlay/Config/` | `~/.local/share/sc-overlay/Data/` |
| **Any OS** | `--portable` | `<exe_dir>/Config/` | `<exe_dir>/Data/` |

All configuration files (`social-config.txt`, `settings.txt`, `mining.txt`, `exclusions.txt`, `dossier-notes.txt`) can be copied directly to/from [Niteb1rd/IRIS](https://github.com/Niteb1rd/IRIS) without conversion.

---

## Workspace Structure

- `crates/iris-core` — Platform-agnostic OCR (`letters-model.bin`), O/0 & `-`/`_` disambiguation, chromatic aberration handling, Hungarian multi-frame tracker, mining HUD scanner, RSI HTTP client + HTML/JSON parsers, SQLite WAL v3 cache, TOML config & merger, and `tiny-skia` + `Quantico-Bold` anti-aliased software overlay renderer.
- `crates/iris-platform` — Native Win32 (`DXGI`, `WGC`, `UpdateLayeredWindow`, `RegisterHotKey`, `Shell_NotifyIconW`) and Linux (`ashpd` XDG ScreenCast + PipeWire, `zwlr_layer_shell_v1` + X11 `SHAPE`, `evdev` + X11 hotkeys, `ksni` SNI tray, and single-instance TCP IPC) backends.
- `crates/iris-app` — `sc-overlay` binary, background capture/detection coordinator thread, `eframe`/`egui` Settings window, and full-screen Interactive Citizen Dossier.

---

## License & Credits

Licensed under **GNU General Public License v3.0 or later (`GPL-3.0-or-later`)** — see [LICENSE](LICENSE) and [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
Special thanks to **Niteb1rd** and contributors of [IRIS](https://github.com/Niteb1rd/IRIS) for the original design, neural OCR weights, and Star Citizen mining signature tables.

