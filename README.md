# wowzers

A World of Warcraft addon manager written in Rust with [GPUI](https://gpui.rs). It reads the public [CurseForge](https://www.curseforge.com/wow/addons) site — no API key — and does four things:

- **Finds your WoW install**: Wine prefixes (plain Wine, Lutris, Faugus, Heroic, Bottles, Proton), `/Applications` on macOS, Program Files on Windows, or any folder you pick.
- **Lists the addons in each client flavor** (Retail, Classic, Classic Era, WoW Forever, PTR, ...), grouping multi-folder addons.
- **Installs, removes and searches** CurseForge addons.
- **Updates everything with one click** (*Update all*).

This is a prototype.

## No API key

Nothing to configure. wowzers reads the same JSON the CurseForge website renders its pages from: page requests marked as React Server Component requests (`RSC: 1`) return compact payloads, and project pages requested by id redirect to their slug. Downloads come from `edge.forgecdn.net`, which serves files without a key.

If the site changes its page payloads, search and update checks fail with a visible error instead of silently returning nothing.

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
| CurseForge client (site search, project pages, CDN downloads) | `curseforge.rs` |
| Update check, install, update-all, uninstall | `manager.rs` |
| Safe zip extraction and rollback | `install.rs` |
| What wowzers installed | `<AddOns>/.wowzers.json` (`manifest.rs`) |
| GPUI app | `crates/wowzers` |

Update checking fetches one project page per addon (six at a time); addons wowzers installed reuse the slug saved in the manifest, so they skip the `/projects/<id>` redirect lookup. Each project's newest file per client flavor comes from the page's `latestFiles`, filtered to the client's game version (read from `.build.info`) and your release channel (stable by default).

CurseForge stores no "installed version", so addons not installed through wowzers are matched by the `X-Curse-Project-ID` in their `.toc`. If the `.toc` version appears in the latest file's name, the addon counts as current. Otherwise it counts as outdated, and *Update all* reinstalls it, after which wowzers tracks it exactly. Reinstalling touches only addon folders, never `WTF` (your settings and saved variables).

Updates are all-or-nothing per addon: files are extracted to a staging folder, old folders are moved aside, new ones are moved in, and a failure restores the old ones. A download that is not a valid addon zip is discarded and the installed version stays in place.

## Limits

- Scraping depends on the site's rendered payloads, not a versioned API. A site change can break search and update checks until the parser is updated; downloads keep working because they are plain CDN URLs.
- The site exposes no file hashes, so downloads are verified only by extracting them.
- Addons installed outside wowzers are matched by project id only; addons without an `X-Curse-Project-ID` show as *Not from CurseForge*. (Fingerprint matching, as the official client does, is not implemented.)
- Dependencies listed by CurseForge are not installed automatically.
- No rollback to older versions.
- Developed and tested on Linux. macOS and Windows paths are detected but untested.

## License

MIT
