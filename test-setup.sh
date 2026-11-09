#!/usr/bin/env bash
#
# test-setup.sh -- Create and provision distrobox containers for testing
# Walt against specific Hyprland/Hyprpaper version combinations.
#
# Usage:
#   ./test-setup.sh create      Create both containers
#   ./test-setup.sh provision   Install packages and build Walt inside each container
#   ./test-setup.sh all         Create + provision (full setup)
#   ./test-setup.sh cleanup     Stop and remove both containers
#   ./test-setup.sh status      Show container status and installed versions
#
# See testing.md for the full test procedure.

set -euo pipefail

CONTAINER_054="walt-test-054"
CONTAINER_053="walt-test-053"
WALT_REPO="https://github.com/gitfudge0/walt.git"
WALT_BRANCH="fix/issue-31-hyprpaper-compatibility"
ALA_DATE="2026/02/15"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BOLD='\033[1m'
NC='\033[0m'

info()  { echo -e "${GREEN}[INFO]${NC} $*"; }
warn()  { echo -e "${YELLOW}[WARN]${NC} $*"; }
error() { echo -e "${RED}[ERROR]${NC} $*"; }
header() { echo -e "\n${BOLD}=== $* ===${NC}\n"; }

# ---------------------------------------------------------------------------
# Preflight
# ---------------------------------------------------------------------------
preflight() {
    local missing=0

    if ! command -v distrobox &>/dev/null; then
        error "distrobox is not installed. Install it with:"
        echo "  sudo pacman -S distrobox"
        missing=1
    fi

    if ! command -v podman &>/dev/null && ! command -v docker &>/dev/null; then
        error "Neither podman nor docker is installed. Install one with:"
        echo "  sudo pacman -S podman"
        missing=1
    fi

    if [[ $missing -eq 1 ]]; then
        exit 1
    fi

    info "Preflight checks passed."
}

# ---------------------------------------------------------------------------
# Create containers
# ---------------------------------------------------------------------------
create_containers() {
    header "Creating containers"

    for name in "$CONTAINER_054" "$CONTAINER_053"; do
        if distrobox list 2>/dev/null | grep -q "$name"; then
            warn "Container '$name' already exists, skipping creation."
        else
            info "Creating container '$name' ..."
            distrobox create --name "$name" --image archlinux --yes
            info "Container '$name' created."
        fi
    done
}

# ---------------------------------------------------------------------------
# Provision helper -- runs a script inside a container
# ---------------------------------------------------------------------------
run_in_container() {
    local name="$1"
    shift
    # distrobox enter runs the command non-interactively when given --
    distrobox enter "$name" -- bash -c "$*"
}

# ---------------------------------------------------------------------------
# Provision: install packages, build walt, create test configs
# ---------------------------------------------------------------------------
provision_054() {
    header "Provisioning $CONTAINER_054 (Hyprland 0.54.x + Hyprpaper 0.8.x)"

    info "Installing packages ..."
    run_in_container "$CONTAINER_054" "
        sudo pacman -Syu --noconfirm hyprland hyprpaper base-devel rustup git
        rustup default stable
    "

    info "Cloning and building Walt ..."
    run_in_container "$CONTAINER_054" "
        if [ -d ~/walt ]; then
            cd ~/walt && git fetch origin && git checkout $WALT_BRANCH && git pull origin $WALT_BRANCH
        else
            git clone --branch $WALT_BRANCH $WALT_REPO ~/walt
        fi
        cd ~/walt && cargo build 2>&1
    "

    info "Creating test sandbox configs ..."
    run_in_container "$CONTAINER_054" "$(sandbox_config_script)"

    info "$CONTAINER_054 provisioned."
}

provision_053() {
    header "Provisioning $CONTAINER_053 (Hyprland 0.53.x via Arch Linux Archive)"

    info "Pinning pacman to ALA snapshot ($ALA_DATE) ..."
    run_in_container "$CONTAINER_053" "
        sudo bash -c 'echo \"Server = https://archive.archlinux.org/repos/${ALA_DATE}/\\\$repo/os/\\\$arch\" > /etc/pacman.d/mirrorlist'
        sudo pacman -Syyuu --noconfirm
    "

    info "Installing packages ..."
    run_in_container "$CONTAINER_053" "
        sudo pacman -S --noconfirm hyprland hyprpaper base-devel rustup git
        rustup default stable
    "

    info "Cloning and building Walt ..."
    run_in_container "$CONTAINER_053" "
        if [ -d ~/walt ]; then
            cd ~/walt && git fetch origin && git checkout $WALT_BRANCH && git pull origin $WALT_BRANCH
        else
            git clone --branch $WALT_BRANCH $WALT_REPO ~/walt
        fi
        cd ~/walt && cargo build 2>&1
    "

    info "Creating test sandbox configs ..."
    run_in_container "$CONTAINER_053" "$(sandbox_config_script)"

    info "$CONTAINER_053 provisioned."
}

# ---------------------------------------------------------------------------
# Sandbox config creation (shared between both containers)
# ---------------------------------------------------------------------------
sandbox_config_script() {
    cat << 'SANDBOX_EOF'
# --- Bad config: source line present ---
mkdir -p /tmp/walt-sandbox-bad/.config/hypr

cat > /tmp/walt-sandbox-bad/.config/hypr/hyprland.conf << 'CONF'
source = ~/.config/hypr/hyprpaper.conf
exec-once = hyprpaper
CONF

cat > /tmp/walt-sandbox-bad/.config/hypr/hyprpaper.conf << 'CONF'
preload = /tmp/test.jpg
wallpaper = ,/tmp/test.jpg
CONF

# --- Good config: no source line ---
mkdir -p /tmp/walt-sandbox-good/.config/hypr

cat > /tmp/walt-sandbox-good/.config/hypr/hyprland.conf << 'CONF'
exec-once = hyprpaper
CONF

cat > /tmp/walt-sandbox-good/.config/hypr/hyprpaper.conf << 'CONF'
preload = /tmp/test.jpg
wallpaper = ,/tmp/test.jpg
CONF

echo "Sandbox configs created at /tmp/walt-sandbox-bad and /tmp/walt-sandbox-good"
SANDBOX_EOF
}

# ---------------------------------------------------------------------------
# Status
# ---------------------------------------------------------------------------
show_status() {
    header "Container status"

    for name in "$CONTAINER_054" "$CONTAINER_053"; do
        if distrobox list 2>/dev/null | grep -q "$name"; then
            echo -e "${GREEN}[EXISTS]${NC} $name"
            # Try to get version info
            run_in_container "$name" "
                echo \"  Hyprland: \$(hyprctl version 2>/dev/null | head -1 || echo 'not installed')\"
                echo \"  Hyprpaper: \$(hyprpaper --version 2>/dev/null || pacman -Q hyprpaper 2>/dev/null || echo 'not installed')\"
                echo \"  Rust: \$(rustc --version 2>/dev/null || echo 'not installed')\"
                if [ -d ~/walt ]; then
                    echo \"  Walt: built (branch: \$(cd ~/walt && git branch --show-current))\"
                else
                    echo \"  Walt: not cloned\"
                fi
            " 2>/dev/null || warn "  Could not query versions (container may not be running)"
        else
            echo -e "${RED}[MISSING]${NC} $name"
        fi
    done
}

# ---------------------------------------------------------------------------
# Cleanup
# ---------------------------------------------------------------------------
cleanup() {
    header "Cleaning up containers"

    for name in "$CONTAINER_054" "$CONTAINER_053"; do
        if distrobox list 2>/dev/null | grep -q "$name"; then
            info "Stopping '$name' ..."
            distrobox stop "$name" --yes 2>/dev/null || true
            info "Removing '$name' ..."
            distrobox rm "$name" --yes
            info "'$name' removed."
        else
            warn "'$name' does not exist, skipping."
        fi
    done
}

# ---------------------------------------------------------------------------
# Print test instructions
# ---------------------------------------------------------------------------
print_instructions() {
    header "Setup complete"

    cat << 'EOF'
Both containers are provisioned with Walt built and test configs ready.
Run the following to test:

  # -------------------------------------------------------
  # Container: walt-test-054 (Hyprland 0.54.x, Hyprpaper 0.8.x)
  # -------------------------------------------------------
  distrobox enter walt-test-054

  # Bad config -- expect compatibility warning:
  cd ~/walt && HOME=/tmp/walt-sandbox-bad cargo run -- random

  # Good config -- expect no warning:
  cd ~/walt && HOME=/tmp/walt-sandbox-good cargo run -- random

  # Unit tests:
  cd ~/walt && cargo test hyprpaper -- --nocapture

  exit  # leave the container

  # -------------------------------------------------------
  # Container: walt-test-053 (Hyprland 0.53.x, older Hyprpaper)
  # -------------------------------------------------------
  distrobox enter walt-test-053

  # Bad config -- expect compatibility warning:
  cd ~/walt && HOME=/tmp/walt-sandbox-bad cargo run -- random

  # Good config -- expect no warning:
  cd ~/walt && HOME=/tmp/walt-sandbox-good cargo run -- random

  # Unit tests:
  cd ~/walt && cargo test hyprpaper -- --nocapture

  exit  # leave the container

  # -------------------------------------------------------
  # Cleanup when done:
  # -------------------------------------------------------
  ./test-setup.sh cleanup
EOF
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
usage() {
    cat << EOF
Usage: $0 <command>

Commands:
  create      Create both distrobox containers
  provision   Install packages and build Walt inside each container
  all         Create + provision (full setup)
  cleanup     Stop and remove both containers
  status      Show container status and installed versions
  help        Show this message
EOF
}

main() {
    local cmd="${1:-help}"

    case "$cmd" in
        create)
            preflight
            create_containers
            ;;
        provision)
            preflight
            provision_054
            provision_053
            print_instructions
            ;;
        all)
            preflight
            create_containers
            provision_054
            provision_053
            print_instructions
            ;;
        cleanup)
            cleanup
            ;;
        status)
            show_status
            ;;
        help|--help|-h)
            usage
            ;;
        *)
            error "Unknown command: $cmd"
            usage
            exit 1
            ;;
    esac
}

main "$@"
