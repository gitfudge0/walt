#!/bin/bash

set -euo pipefail

REPO_OWNER="gitfudge0"
REPO_NAME="walt"
DEFAULT_REF="${WALT_REF:-main}"
ARCHIVE_URL="https://github.com/${REPO_OWNER}/${REPO_NAME}/archive/refs/heads/${DEFAULT_REF}.tar.gz"
INSTALL_DIR="${HOME}/.local/bin"
INSTALL_PATH="${INSTALL_DIR}/walt"

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

prepare_source_tree() {
  if [[ -f "./Cargo.toml" ]] && grep -q '^name = "walt"' "./Cargo.toml"; then
    pwd
    return
  fi

  require_cmd curl
  require_cmd tar

  local tmp_dir
  tmp_dir="$(mktemp -d)"
  trap 'rm -rf "$tmp_dir"' EXIT

  echo "Downloading Walt source from ${DEFAULT_REF}..."
  curl -fsSL "$ARCHIVE_URL" | tar -xz -C "$tmp_dir"
  printf '%s\n' "${tmp_dir}/${REPO_NAME}-${DEFAULT_REF}"
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
  local source_dir
  source_dir="$(prepare_source_tree)"
  install_walt "$source_dir"

  local terminal
  terminal="$(detect_terminal)"

  echo ""
  echo "Installation complete!"
  echo ""
  print_terminal_instructions "$terminal"
  echo ""
  echo "First run:"
  echo "  1. Copy the path to your wallpaper directory."
  echo "  2. Launch Walt."
  echo "  3. Paste the directory path into the app and press Enter."
  echo ""
  echo "Make sure you have:"
  echo "  - hyprpaper installed and running"
  echo "  - A terminal with image preview support"
}

main "$@"
