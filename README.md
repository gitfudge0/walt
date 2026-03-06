# Walt

Walt is a terminal-first wallpaper picker for Hyprland. It gives you a two-column browser with live image preview, themeable UI, path management, and one-key random wallpaper selection.

![Walt wordmark](assets/walt-wordmark.svg)

## Features

- Live terminal image preview with `ratatui-image`
- Wallpaper browser with vim-style and arrow-key navigation
- `r` to pick and apply a random wallpaper instantly
- Path manager for adding and removing wallpaper directories
- Hidden directories included in path suggestions
- 10 built-in themes plus `System`, which follows your terminal colors
- Multi-monitor wallpaper application through `hyprpaper`

## Requirements

- `rust` and `cargo`
- `hyprpaper`
- `hyprctl`
- A terminal with image protocol support
  - Ghostty
  - Kitty
  - WezTerm
  - iTerm2

## Build

```bash
cargo build --release
```

The compiled binary will be `target/release/walt`.

## Install

```bash
./install.sh
```

Or manually:

```bash
cargo build --release
install -Dm755 target/release/walt ~/.local/bin/
```

## Hyprland Setup

Add this to [hyprland.conf](/home/gitfudge/.config/hypr/hyprland.conf):

```conf
bind = $mainMod SHIFT, D, exec, ghostty --class=walt -e ~/.local/bin/walt
```

Optional floating rule:

```conf
windowrulev2 = float, class:^(com\.mitchellh\.ghostty\.walt)$
windowrulev2 = size 900 600, class:^(com\.mitchellh\.ghostty\.walt)$
windowrulev2 = center, class:^(com\.mitchellh\.ghostty\.walt)$
```

`./install.sh` detects your current terminal and prints the matching launch command and Hyprland rules for Ghostty, WezTerm, or Kitty.

Make sure `hyprpaper` is started:

```conf
exec-once = hyprpaper
```

## Usage

### First Run

- Copy the path to your wallpaper directory, then paste it into Walt
- Use `↑/↓` to move through suggestions
- Press `Tab` to autocomplete a suggestion
- Press `Enter` to save the path

Walt stores config in `~/.config/walt/` and automatically reads legacy settings from `~/.config/wallpaper-switcher/`.

### Wallpaper View

- `↑/↓` or `j/k`: move selection
- `g/G`: jump to top or bottom
- `Enter`: apply selected wallpaper
- `r`: pick and apply a random wallpaper
- `p`: open the path manager
- `t`: cycle UI themes
- `q` or `Esc`: quit

### Path Manager

- `↑/↓` or `j/k`: move selection
- `a`: add a path
- `d`: delete the selected path
- `p`, `q`, or `Esc`: return to wallpaper view
- `t`: cycle UI themes

## Themes

Built-in themes:

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

`System` intentionally uses the host terminal's default foreground and background behavior instead of hardcoding a palette.

## Files

- Paths: `~/.config/walt/paths.conf`
- Theme: `~/.config/walt/theme.conf`

## Development

```bash
cargo build
```

## License

MIT
