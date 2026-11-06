# tiler

Arranges your windows into tiles. Up to 3 per monitor; any extra windows go full screen on the laptop screen.

## Install

Needs Rust: https://rustup.rs

```
cargo install --path crates\tiler
```

Run it again after updating the code.

## Use

| Command | What it does |
|---|---|
| `tiler --once` | Arrange the windows now, then exit. |
| `tiler --listen` | Press **Win+Y** any time to arrange. Runs quietly in the background. |
| `tiler --stop` | Stop the Win+Y listener. |
| `tiler --autostart` | Start the Win+Y listener at every login. Undo with `tiler --no-autostart`. |

If Win+Y is taken, it uses Win+J. To pick your own key: `tiler --listen ctrl+alt+t`

## Settings

Edit `%APPDATA%\tiler\config.toml`. The ones you're most likely to change:

```toml
max_tiles = 3       # windows per monitor before the rest go full screen on the laptop
[gaps]
outer = 8
inner = 8
```

## If something goes wrong

- `tiler --restore` brings back any windows that went missing.
- Log file: `%LOCALAPPDATA%\tiler\tiler.log`

## Full mode

Plain `tiler` keeps running and re-arranges windows automatically whenever they open or close.
It adds workspaces and Alt-key shortcuts; `tiler --help` lists everything.
