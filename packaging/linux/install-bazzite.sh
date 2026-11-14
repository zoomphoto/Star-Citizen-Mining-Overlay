#!/usr/bin/env bash
# ==============================================================================
# SC Overlay — Bazzite & Fedora Linux User-Space Installer
# ==============================================================================
# Designed for Bazzite (Fedora Atomic / Wayland / KDE Plasma / GNOME / Gamescope)
# and standard Fedora Workstation. Installs cleanly into ~/.local without
# requiring root or rpm-ostree layering.
# ==============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

BIN_DIR="${HOME}/.local/bin"
APP_DIR="${HOME}/.local/share/applications"
ICON_DIR="${HOME}/.local/share/icons/hicolor/256x256/apps"

echo "==> Installing SC Overlay to user profile (~/.local)..."

# 1. Locate or build release binary
BINARY_PATH=""
if [[ -x "${REPO_ROOT}/sc-overlay" ]]; then
    BINARY_PATH="${REPO_ROOT}/sc-overlay"
elif [[ -x "${REPO_ROOT}/target/release/sc-overlay" ]]; then
    BINARY_PATH="${REPO_ROOT}/target/release/sc-overlay"
elif command -v cargo >/dev/null 2>&1; then
    echo "==> Building release binary with cargo..."
    (cd "${REPO_ROOT}" && cargo build --release)
    BINARY_PATH="${REPO_ROOT}/target/release/sc-overlay"
else
    echo "ERROR: No pre-built 'sc-overlay' binary found and 'cargo' is not installed."
    echo ""
    echo "On Bazzite (immutable Fedora Atomic), you can either:"
    echo "  A) Download the Linux release binary from GitHub Actions / Releases and place"
    echo "     it in ${REPO_ROOT}/sc-overlay, then re-run this script; OR"
    echo "  B) Build inside a Fedora Distrobox container:"
    echo "       distrobox create --name fedora-dev --image registry.fedoraproject.org/fedora-toolbox:latest"
    echo "       distrobox enter fedora-dev -- bash -lc 'sudo dnf install -y rust cargo gcc pkgconf-pkg-config libX11-devel wayland-devel libxkbcommon-devel mesa-libGL-devel && cd \"${REPO_ROOT}\" && cargo build --release'"
    echo "     Then re-run ./packaging/linux/install-bazzite.sh on the host."
    exit 1
fi

mkdir -p "${BIN_DIR}" "${APP_DIR}" "${ICON_DIR}"
install -m 0755 "${BINARY_PATH}" "${BIN_DIR}/sc-overlay"
echo "  [OK] Installed binary: ${BIN_DIR}/sc-overlay"

# 2. Install desktop entry with absolute binary path so KDE/GNOME shortcuts always find it
sed "s|^Exec=sc-overlay|Exec=${BIN_DIR}/sc-overlay|g" \
    "${SCRIPT_DIR}/sc-overlay.desktop" > "${APP_DIR}/sc-overlay.desktop"
chmod 0644 "${APP_DIR}/sc-overlay.desktop"
echo "  [OK] Installed desktop entry: ${APP_DIR}/sc-overlay.desktop"

# 3. Install PNG icon if bundled
if [[ -f "${SCRIPT_DIR}/sc-overlay.png" ]]; then
    install -m 0644 "${SCRIPT_DIR}/sc-overlay.png" "${ICON_DIR}/sc-overlay.png"
    echo "  [OK] Installed icon: ${ICON_DIR}/sc-overlay.png"
fi

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "${APP_DIR}" >/dev/null 2>&1 || true
fi

cat <<EOF

================================================================================
SC Overlay installed successfully!
================================================================================

Quick Start on Bazzite (KDE Plasma Wayland / GNOME / LUG Helper / Lutris / Steam):

  1. Set Star Citizen to "Borderless" in Graphics Settings (not Exclusive Fullscreen).
     If using Gamescope, run without isolating Star Citizen on a separate nested
     display, or run Star Citizen directly on your desktop Wayland/XWayland session
     via LUG Helper / Lutris / Steam Proton.

  2. Launch SC Overlay:
       ${BIN_DIR}/sc-overlay

  3. First-time Wayland Screen Capture Permission:
     When Star Citizen is active, KDE/GNOME's XDG ScreenCast portal will prompt
     once to share your monitor/window. Check "Remember / Allow restore" — SC Overlay
     saves the restore token in ~/.config/sc-overlay/Config/settings.txt so you
     won't be prompted again.

  4. Wayland Global Hotkeys (Recommended on KDE Plasma / GNOME):
     Wayland isolates input between windows by design. You can bind global shortcuts
     in KDE System Settings -> Keyboard -> Shortcuts -> Add New -> Command:
       • Toggle Overlay (Shift+F12):     ${BIN_DIR}/sc-overlay --toggle
       • Interactive Dossier (Shift+0):  ${BIN_DIR}/sc-overlay --interactive
       • Timed Overlay Peek:             ${BIN_DIR}/sc-overlay --timed

     (Optional) For non-consuming single-key Tab peek directly in Star Citizen on
     Wayland without desktop shortcuts, grant your user read access to /dev/input:
       sudo usermod -aG input "\$USER"   # takes effect after re-login

EOF
