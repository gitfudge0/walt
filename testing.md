# Testing Walt with Isolated Hyprland/Hyprpaper Environments

This guide covers testing Walt against specific Hyprland and Hyprpaper version combinations using distrobox containers on Arch Linux, without affecting your host setup.

## Background

Issue [#31](https://github.com/gitfudge0/walt/issues/31) identified a protocol compatibility problem between Hyprland 0.54.x and Hyprpaper 0.8.x. The versions reported in the issue are:

| Reporter | Hyprland | Hyprpaper | Outcome |
|---|---|---|---|
| khaosdoctor | 0.54.2 | (not stated) | Broken -- preload + listactive fail with "protocol version too low" |
| MaoRusk | 0.54.3 | 0.8.3 | Broken when `source = hyprpaper.conf` in hyprland.conf; works when removed |
| khaosdoctor (noted) | 0.53.x | (paired version) | Worked fine |

## Quick Start (Automated)

The `test-setup.sh` script automates container creation and provisioning.

### Prerequisites

Install distrobox and podman:

```bash
sudo pacman -S distrobox podman
```

### Full Setup

```bash
./test-setup.sh all
```

This will:
1. Create two distrobox containers (`walt-test-054` and `walt-test-053`)
2. Install the correct Hyprland/Hyprpaper versions in each
3. Clone and build Walt on the `fix/issue-31-hyprpaper-compatibility` branch
4. Create pre-built test config sandboxes (bad config + good config)

The script prints test instructions when done.

### Script Commands

| Command | What it does |
|---|---|
| `./test-setup.sh create` | Create both containers |
| `./test-setup.sh provision` | Install packages and build Walt inside each container |
| `./test-setup.sh all` | Create + provision (full setup) |
| `./test-setup.sh status` | Show container status and installed versions |
| `./test-setup.sh cleanup` | Stop and remove both containers |

### Running Tests

After setup, enter each container and run the test cases:

```bash
# Enter the 0.54.x container
distrobox enter walt-test-054

# Test with bad config (source line present) -- expect warning:
cd ~/walt && HOME=/tmp/walt-sandbox-bad cargo run -- random

# Test with good config (no source line) -- expect no warning:
cd ~/walt && HOME=/tmp/walt-sandbox-good cargo run -- random

# Run unit tests:
cd ~/walt && cargo test hyprpaper -- --nocapture

exit
```

Repeat in the 0.53.x container:

```bash
distrobox enter walt-test-053
# (same commands as above)
```

### Cleanup

```bash
./test-setup.sh cleanup
```

## Manual Setup (Reference)

If you prefer to set things up manually or need to customize the process, the details are below.

### Environment 1: Current Versions (Hyprland 0.54.2 + Hyprpaper 0.8.3)

This matches the broken setup reported in issue #31. The current Arch repos ship these exact versions.

```bash
distrobox create --name walt-test-054 --image archlinux
distrobox enter walt-test-054

# Inside the container:
sudo pacman -Syu --noconfirm hyprland hyprpaper base-devel rustup git
rustup default stable
```

### Environment 2: Older Versions (Hyprland 0.53.x)

Arch doesn't keep old package versions in its repos. Two options:

#### Option A: Arch Linux Archive (ALA)

The Arch Linux Archive keeps snapshots of every repo state. Pin the container's pacman to a date when 0.53.x was current (0.54.0 landed late Feb 2026, so a mid-Feb snapshot should have 0.53.x):

```bash
distrobox create --name walt-test-053 --image archlinux
distrobox enter walt-test-053

# Inside the container:
sudo bash -c 'echo "Server = https://archive.archlinux.org/repos/2026/02/15/\$repo/os/\$arch" > /etc/pacman.d/mirrorlist'
sudo pacman -Syyuu --noconfirm
sudo pacman -S --noconfirm hyprland hyprpaper base-devel rustup git
rustup default stable
```

#### Option B: Build from Source Tags

```bash
distrobox create --name walt-test-053-src --image archlinux
distrobox enter walt-test-053-src

# Inside the container:
sudo pacman -Syu --noconfirm base-devel cmake git meson ninja rustup \
  wayland wayland-protocols libinput pixman libdrm mesa pango cairo \
  xorg-xwayland libxkbcommon tomlplusplus hyprlang hyprutils hyprwayland-scanner

# Build Hyprland 0.53.0
git clone --branch v0.53.0 --depth 1 https://github.com/hyprwm/Hyprland.git
cd Hyprland && cmake -B build && cmake --build build -j$(nproc) && sudo cmake --install build
cd ..

# Build Hyprpaper 0.7.1 (the version paired with 0.53.x)
git clone --branch v0.7.1 --depth 1 https://github.com/hyprwm/hyprpaper.git
cd hyprpaper && cmake -B build && cmake --build build -j$(nproc) && sudo cmake --install build
cd ..
```

## Test Procedure

Run these inside each container.

### Clone and Build Walt

```bash
git clone --branch fix/issue-31-hyprpaper-compatibility https://github.com/gitfudge0/walt.git
cd walt && cargo build
```

### Test Case A: Bad Config (source line present)

This simulates the misconfiguration that causes the silent failure.

```bash
mkdir -p /tmp/walt-sandbox-bad/.config/hypr

cat > /tmp/walt-sandbox-bad/.config/hypr/hyprland.conf << 'EOF'
source = ~/.config/hypr/hyprpaper.conf
exec-once = hyprpaper
EOF

cat > /tmp/walt-sandbox-bad/.config/hypr/hyprpaper.conf << 'EOF'
preload = /tmp/test.jpg
wallpaper = ,/tmp/test.jpg
EOF

HOME=/tmp/walt-sandbox-bad cargo run -- random
```

**Expected:** Compatibility warning is printed about the `source` line.

### Test Case B: Good Config (no source line)

```bash
mkdir -p /tmp/walt-sandbox-good/.config/hypr

cat > /tmp/walt-sandbox-good/.config/hypr/hyprland.conf << 'EOF'
exec-once = hyprpaper
EOF

HOME=/tmp/walt-sandbox-good cargo run -- random
```

**Expected:** No compatibility warning.

### Test Case C: Unit Tests

```bash
cargo test hyprpaper -- --nocapture
```

**Expected:** All tests pass in both containers.

## Test Matrix

| Container | Config | Expected Warning | Expected Wallpaper |
|---|---|---|---|
| walt-test-054 | Bad (source line) | Yes -- source line detected | Reports success but may not visibly change |
| walt-test-054 | Good (no source) | No | Sets wallpaper normally |
| walt-test-053 | Bad (source line) | Yes -- source line detected | May work (older protocol is compatible) |
| walt-test-053 | Good (no source) | No | Sets wallpaper normally |

## Distrobox and Wayland IPC

Distrobox shares your host's Wayland socket. This has implications:

- **Config detection logic** exercises against the container's config files (controlled by the `HOME` override). This is safe.
- **`hyprctl hyprpaper wallpaper` commands** talk to your host compositor. If the command succeeds, it changes your host wallpaper. This is cosmetic and easily reversible.
- **The container's installed hyprpaper binary is not the running one.** Your host's hyprpaper daemon handles IPC. So the protocol version behavior depends on your host's running hyprpaper, not the container's.

### Testing Actual IPC Protocol Differences

To test that different hyprpaper versions respond differently to `listactive` or `preload`, you need to swap the running hyprpaper daemon. This is a manual step -- the setup script does not do this.

```bash
# On the host: stop the current hyprpaper
killall hyprpaper

# Inside the distrobox container: start the container's hyprpaper
# (it connects to the host Wayland socket automatically)
hyprpaper &

# Run walt
HOME=/tmp/walt-sandbox-bad cargo run -- random

# When done, kill the container's hyprpaper and restart your host's
killall hyprpaper
# Back on the host:
hyprpaper &
```

This is safe -- worst case you just restart hyprpaper.
