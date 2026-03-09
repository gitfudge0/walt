#!/bin/bash

set -euo pipefail

REPO_OWNER="gitfudge0"
REPO_NAME="walt"
DEFAULT_REF="${WALT_REF:-main}"
ARCHIVE_URL="https://github.com/${REPO_OWNER}/${REPO_NAME}/archive/refs/heads/${DEFAULT_REF}.tar.gz"
INSTALL_DIR="${HOME}/.local/bin"
INSTALL_PATH="${INSTALL_DIR}/walt"
SOURCE_DIR=""
TEMP_SOURCE_DIR=""

detect_terminal() {
  if [[ "${TERM_PROGRAM:-}" == "ghostty" ]] || [[ "${TERM:-}" == "xterm-ghostty" ]]; then
    printf '%s\n' "ghostty"
    return
  fi

  if [[ "${TERM_PROGRAM:-}" == "WezTerm" ]] || [[ -n "${WEZTERM_EXECUTABLE:-}" ]]; then
    printf '%s\n' "wezterm"
    return
  fi

  if [[ -n "${KITTY_PID:-}" ]] || [[ "${TERM:-}" == "xterm-kitty" ]]; then
    printf '%s\n' "kitty"
    return
  fi

  printf '%s\n' "unknown"
}

print_terminal_instructions() {
  local terminal="$1"

  case "$terminal" in
    ghostty)
      cat <<'EOF'
Detected terminal: Ghostty

Add this to your ~/.config/hypr/hyprland.conf:

  bind = $mainMod SHIFT, D, exec, ghostty --class=walt -e ~/.local/bin/walt
  bind = $mainMod CTRL, D, exec, ~/.local/bin/walt gui
  bind = $mainMod, D, exec, ~/.local/bin/walt random

Optional floating rules:

  windowrulev2 = float, class:^(com\.mitchellh\.ghostty\.walt)$
  windowrulev2 = size 900 600, class:^(com\.mitchellh\.ghostty\.walt)$
  windowrulev2 = center, class:^(com\.mitchellh\.ghostty\.walt)$
EOF
      ;;
    wezterm)
      cat <<'EOF'
Detected terminal: WezTerm

Add this to your ~/.config/hypr/hyprland.conf:

  bind = $mainMod SHIFT, D, exec, wezterm start --class walt -- ~/.local/bin/walt
  bind = $mainMod CTRL, D, exec, ~/.local/bin/walt gui
  bind = $mainMod, D, exec, ~/.local/bin/walt random

Optional floating rules:

  windowrulev2 = float, class:^(walt)$
  windowrulev2 = size 900 600, class:^(walt)$
  windowrulev2 = center, class:^(walt)$
EOF
      ;;
    kitty)
      cat <<'EOF'
Detected terminal: Kitty

Add this to your ~/.config/hypr/hyprland.conf:

  bind = $mainMod SHIFT, D, exec, kitty --class walt -e ~/.local/bin/walt
  bind = $mainMod CTRL, D, exec, ~/.local/bin/walt gui
  bind = $mainMod, D, exec, ~/.local/bin/walt random

Optional floating rules:

  windowrulev2 = float, class:^(walt)$
  windowrulev2 = size 900 600, class:^(walt)$
  windowrulev2 = center, class:^(walt)$
EOF
      ;;
    *)
      cat <<'EOF'
Detected terminal: unknown

Add a bind for your preferred terminal manually.
Examples:

  ghostty --class=walt -e ~/.local/bin/walt
  wezterm start --class walt -- ~/.local/bin/walt
  kitty --class walt -e ~/.local/bin/walt
  ~/.local/bin/walt gui
  ~/.local/bin/walt random

Then match the resulting class/app-id in Hyprland with window rules.
EOF
      ;;
  esac
}

require_cmd() {
  local cmd="$1"
  if ! command -v "$cmd" >/dev/null 2>&1; then
    echo "Missing required command: $cmd" >&2
    exit 1
  fi
}

cleanup_tmp_dir() {
  local tmp_dir="$1"
  if [[ -n "$tmp_dir" && -d "$tmp_dir" ]]; then
    rm -rf -- "$tmp_dir"
  fi
}

prepare_source_tree() {
  if [[ -f "./Cargo.toml" ]] && grep -q '^name = "walt"' "./Cargo.toml"; then
    SOURCE_DIR="$(pwd)"
    return
  fi

  require_cmd curl
  require_cmd tar

  TEMP_SOURCE_DIR="$(mktemp -d)"
  trap "cleanup_tmp_dir '$TEMP_SOURCE_DIR'" EXIT

  echo "Downloading Walt source from ${DEFAULT_REF}..." >&2
  curl -fsSL "$ARCHIVE_URL" | tar -xz -C "$TEMP_SOURCE_DIR"

  SOURCE_DIR="${TEMP_SOURCE_DIR}/${REPO_NAME}-${DEFAULT_REF}"
  if [[ ! -f "${SOURCE_DIR}/Cargo.toml" ]]; then
    echo "Downloaded archive did not contain ${SOURCE_DIR}/Cargo.toml" >&2
    exit 1
  fi
}

install_walt() {
  local source_dir="$1"

  require_cmd cargo
  mkdir -p "$INSTALL_DIR"

  echo "Building Walt..."
  cargo build --release --manifest-path "${source_dir}/Cargo.toml"

  echo "Installing binary..."
  install -Dm755 "${source_dir}/target/release/walt" "$INSTALL_PATH"
}

main() {
  prepare_source_tree
  install_walt "$SOURCE_DIR"

  local terminal
  terminal="$(detect_terminal)"

  echo ""
  echo "Installation complete!"
  echo ""
  print_terminal_instructions "$terminal"
  echo ""
  echo "First run:"
  echo "  1. Copy the path to your wallpaper directory."
  echo "  2. Launch Walt with \`walt\` or \`walt gui\`."
  echo "  3. Add the directory path inside the app."
  echo ""
  echo "Make sure you have:"
  echo "  - hyprpaper installed and running"
  echo "  - A terminal with image preview support"
}

main "$@"
