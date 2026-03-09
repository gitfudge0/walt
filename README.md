# Walt

Walt is a terminal wallpaper picker for Hyprland. It lets you browse, preview, apply, randomize, and rotate wallpapers without leaving the keyboard, using `hyprpaper` to set the background. The TUI stays focused on fast navigation through large wallpaper directories while still giving you themes and rotation controls when you need them.

![Walt banner](assets/walt-banner.jpg)

## At a glance

- Browse and apply wallpapers from the terminal
- Preview images in place before switching
- Navigate large wallpaper libraries quickly
- Use built-in themes, including `System`
- Apply a random wallpaper with a single command
- Install an optional background rotation service
- Work with multi-monitor Hyprland setups through `hyprpaper`

![Walt screenshot](assets/screenshot.png)

## Quick Start

### Requirements

- `rust` and `cargo`
- `hyprpaper`
- `hyprctl`
- an image-capable terminal:
  - Ghostty
  - Kitty
  - WezTerm
  - iTerm2

### Install

Quick install:

```bash
curl -fsSL https://raw.githubusercontent.com/gitfudge0/walt/main/install.sh | bash
```

This installs `walt` to `~/.local/bin/walt`.

From a local checkout:

```bash
./install.sh
```

Manual install:

```bash
cargo build --release
install -Dm755 target/release/walt ~/.local/bin/
```

### First run

1. Launch `walt`.
2. Paste the path to your wallpaper directory.
3. Press `Enter`.

Walt stores its config in `~/.config/walt/`. If you have older settings in `~/.config/wallpaper-switcher/`, Walt will read them automatically.

### Hyprland example

For Ghostty:

```conf
bind = $mainMod SHIFT, D, exec, ghostty --class=walt -e ~/.local/bin/walt
bind = $mainMod, D, exec, ~/.local/bin/walt random
windowrulev2 = float, class:^(com\.mitchellh\.ghostty\.walt)$
windowrulev2 = size 900 600, class:^(com\.mitchellh\.ghostty\.walt)$
windowrulev2 = center, class:^(com\.mitchellh\.ghostty\.walt)$
```

`$mainMod + Shift + D` opens the Walt TUI. `$mainMod + D` applies a random wallpaper immediately.

`install.sh` detects Ghostty, WezTerm, or Kitty and prints matching launch instructions, including the random-wallpaper bind.

Make sure `hyprpaper` is running:

```conf
exec-once = hyprpaper
```

## CLI Commands

### Open the TUI

```bash
walt
```

Launch the wallpaper browser.

### Apply a random wallpaper

```bash
walt random
```

This picks one random wallpaper from all configured directories and applies it without opening the TUI.

### Manage the rotation service

```bash
walt rotation install
walt rotation status
walt rotation interval 900
walt rotation disable
walt rotation enable
walt rotation uninstall
```

- `install` installs and starts the user service
- `status` shows the current service state
- `interval 900` sets the rotation interval in seconds
- `disable` stops the installed service
- `enable` starts it again
- `uninstall` removes it completely

Example `status` output:

```text
Rotation Service
Status:   running
Loaded:   loaded (~/.config/systemd/user/walt-rotation.service)
Enabled:  enabled
Active:   active
Mode:     selected wallpapers
Interval: 300s (5m)
Entries:  12 wallpapers
```

Walt does not auto-rotate wallpapers while the TUI is open unless you install the background service.
When rotate-all mode is enabled from the TUI, `Entries` changes to `all wallpapers`.

### Uninstall

```bash
walt uninstall
```

Prompts before removing the rotation service, config, cache, and installed `~/.local/bin/walt` binary.

For non-interactive use:

```bash
walt uninstall --yes
```

## Keyboard Controls

### Browser

Walt opens with the current wallpaper selected in the `All` list when it is already indexed.

- `↑/↓` or `j/k` move
- `Tab` or `l` switch between `All` and `Rotation`
- `Shift+Tab` or `h` switch to the previous section
- `g/G` jump to the top or bottom
- `Enter` apply the selected wallpaper
- `/` filter the active section
- `s` toggle sort for the active section between name and modification date
- `r` add or remove the selected wallpaper from the manual rotation list
- `Ctrl+r` pick and apply a random wallpaper
- `R` open the rotation popup for service actions and rotate-all mode
- `i` change the interval used by the installed rotation service
- `p` manage wallpaper paths
- `t` open the theme picker
- `?` open the keybindings popup
- `q` or `Esc` quit

`walt rotation enable` and `walt rotation disable` still control only the background service. The rotate-all mode is available only from the `R` popup.

### Path manager

- `↑/↓` or `j/k` move
- `a` add a path
- `d` remove the selected path
- `p`, `q`, or `Esc` return
- `t` open the theme picker

### Theme picker

- `↑/↓` or `j/k` preview themes
- `Enter` confirm
- `Esc` or `q` cancel

## Themes

Walt includes these themes:

- `System`
- `Catppuccin Mocha`
- `Tokyo Night`
- `Gruvbox Dark`
- `Dracula`
- `Nord`
- `Solarized Dark`
- `Kanagawa`
- `One Dark`
- `Everforest Dark`
- `Rosé Pine`

`System` uses your terminal defaults. The named themes use opaque surfaces for a cleaner in-app look.

## Build

```bash
cargo build --release
```

The binary will be available at `target/release/walt`.

## License

MIT
