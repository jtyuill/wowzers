# wowzers

A World of Warcraft addon manager written in Rust with [GPUI](https://gpui.rs). It wraps the [CurseForge API](https://docs.curseforge.com/rest-api) and does four things:

- **Finds your WoW install**: Wine prefixes (plain Wine, Lutris, Faugus, Heroic, Bottles, Proton), `/Applications` on macOS, Program Files on Windows, or any folder you pick.
- **Lists the addons in each client flavor** (Retail, Classic, Classic Era, WoW Forever, PTR, ...), grouping multi-folder addons.
- **Installs, removes and searches** CurseForge addons.
- **Updates everything with one click** (*Update all*).

This is a prototype.

## CurseForge API key

CurseForge requires an API key. Create one (free) at <https://console.curseforge.com/#/api-keys> and paste it into *Settings*, or set `CURSEFORGE_API_KEY`. The key is stored in `~/.config/wowzers/config.json` with mode 0600.

## Build and run

```sh
nix develop -c cargo run -p wowzers      # Linux / macOS, with the flake's dev shell
nix run github:jtyuill/wowzers           # or run the packaged build
```

Without Nix, install a Rust toolchain plus GPUI's system libraries (on Linux: fontconfig, freetype, libxkbcommon, Vulkan loader, Wayland, X11/xcb) and run `cargo run -p wowzers`.

```sh
cargo test --workspace
```

## How it works

| Piece | Where |
| --- | --- |
| Install and flavor detection, `.build.info` parsing | `wowzers-core/src/detect.rs` |
| `.toc` parsing, folder grouping | `toc.rs`, `installed.rs` |
| CurseForge client (search, batch mods/files, download + SHA-1 check) | `curseforge.rs` |
| Update check, install, update-all, uninstall | `manager.rs` |
| Safe zip extraction and rollback | `install.rs` |
| What wowzers installed | `<AddOns>/.wowzers.json` (`manifest.rs`) |
| GPUI app | `crates/wowzers` |

Update checking costs two API requests however many addons you have: one batched `POST /v1/mods` and one batched `POST /v1/mods/files`. Each project's newest file is chosen from `latestFilesIndexes`, filtered to the client's game version (read from `.build.info`) and your release channel (stable by default).

CurseForge stores no "installed version", so addons not installed through wowzers are matched by the `X-Curse-Project-ID` in their `.toc`. If the `.toc` version appears in the latest file's name, the addon counts as current. Otherwise it counts as outdated, and *Update all* reinstalls it, after which wowzers tracks it exactly. Reinstalling touches only addon folders, never `WTF` (your settings and saved variables).

Updates are all-or-nothing per addon: files are extracted to a staging folder, old folders are moved aside, new ones are moved in, and a failure restores the old ones. Downloads that fail their SHA-1 check are discarded. Addons whose author disabled third-party downloads are listed as *Manual download only*.

## Limits

- Addons installed outside wowzers are matched by project id only; addons without an `X-Curse-Project-ID` show as *Not from CurseForge*. (Fingerprint matching, as the official client does, is not implemented.)
- Dependencies listed by CurseForge are not installed automatically.
- No rollback to older versions.
- Developed and tested on Linux. macOS and Windows paths are detected but untested.

## License

MIT
