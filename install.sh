#!/bin/bash

set -euo pipefail

detect_terminal() {
  if [[ "${TERM_PROGRAM:-}" == "ghostty" ]] || [[ "${TERM:-}" == "xterm-ghostty" ]]; then
    printf '%s\n' "ghostty"
    return
  fi

  if [[ "${TERM_PROGRAM:-}" == "WezTerm" ]] || [[ -n "${WEZTERM_EXECUTABLE:-}" ]]; then
    printf '%s\n' "wezterm"
    return
  fi

  if [[ "${KITTY_PID:-}" != "" ]] || [[ "${TERM:-}" == "xterm-kitty" ]]; then
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

echo "Building Walt..."
cargo build --release

echo "Installing binary..."
install -Dm755 target/release/walt ~/.local/bin/

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
