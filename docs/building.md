# Building and packaging

[← Docs index](README.md) · [Getting started](getting-started.md) ·
[Document format](document-format.md) · [Bridge](bridge.md) · [API](api-reference.md) ·
[Security](security.md) · [Architecture](architecture.md) · **Building** ·
[Contributing](contributing.md) · [AGENT.md](AGENT.md)

---

## Building the runtime

```sh
cargo build --release
cargo test --workspace          # 130 tests
./target/release/htmlapp install
```

Rust 1.90 or newer.

### System dependencies

| | Ubuntu / Debian | Fedora | Arch |
|---|---|---|---|
| WebKitGTK | `libwebkit2gtk-4.1-dev` | `webkit2gtk4.1-devel` | `webkit2gtk-4.1` |
| JavaScriptCore | `libjavascriptcoregtk-4.1-dev` | *(with WebKitGTK)* | *(with WebKitGTK)* |
| GTK 3 | `libgtk-3-dev` | `gtk3-devel` | `gtk3` |
| libsoup 3 | `libsoup-3.0-dev` | `libsoup3-devel` | `libsoup3` |
| udev | `libudev-dev` | `systemd-devel` | `systemd-libs` |
| X11 | `libx11-dev` | `libX11-devel` | `libx11` |
| xkbcommon | `libxkbcommon-dev` | `libxkbcommon-devel` | `libxkbcommon` |
| xkbcommon-x11 | `libxkbcommon-x11-dev` | `libxkbcommon-x11-devel` | *(with xkbcommon)* |
| Wayland | `libwayland-dev` | `wayland-devel` | `wayland` |
| fontconfig | `libfontconfig1-dev` | `fontconfig-devel` | `fontconfig` |
| FreeType | `libfreetype-dev` | `freetype-devel` | `freetype2` |
| Vulkan | `libvulkan-dev` | `vulkan-loader-devel` | `vulkan-icd-loader` |
| OpenSSL | `libssl-dev` | `openssl-devel` | `openssl` |

`udev` is easy to miss: nothing names it directly, but `serialport` and the `udev` module both pull
in `libudev-sys`, which probes for it. `xkbcommon-x11` is the other one — a separate package from
`xkbcommon` on Debian and Fedora, wanted by gpui's X11 backend, and absent from the list it fails
at link time with `unable to find library -lxkbcommon-x11` rather than with a pkg-config error. CI
installs this exact list from
[`.github/actions/linux-deps`](../.github/actions/linux-deps/action.yml), which is the canonical
copy.

Optional, for specific features:

| For | Install |
|---|---|
| Layer-shell window modes | `libgtk-layer-shell-dev` / `gtk-layer-shell-devel` / `gtk-layer-shell` |
| `--sandbox` | `bubblewrap` |
| Portals (dialogs, screenshots) | `xdg-desktop-portal` + a backend for your desktop |
| `.AppImage` output | `appimagetool` |

`nix develop` gives you all of it, including the optional ones.

---

## Cargo features

Default features cover everything that builds without an extra system library.

| Crate | Feature | Default | What it does |
|---|---|---|---|
| `htmlapp` | `layer-shell` | off | `mode: "layer"` and `"lock"`. Needs `libgtk-layer-shell-dev`. |
| `htmlapp-engine` | `wry-backend` | **on** | The WebKitGTK backend. |
| `htmlapp-engine` | `wpe-backend` | off | Reserved for the offscreen WPE backend. |
| `htmlapp-engine` | `devtools` | off | Makes the engine inspector reachable. |
| `htmlapp-api` | `tier1`…`tier4` | **on** | The API catalog tiers. |
| `htmlapp-api` | `usb` | off | `rusb`, with libusb built from source. Adds a C build to every compile. |
| `htmlapp-api` | `bluetooth` | off | `bluer`, over BlueZ's D-Bus interface. |
| `htmlapp-api` | `plugin` | off | `wasmtime`. Large; adds minutes to a cold build. |
| `htmlapp-engine-cef` | `cef` | off | Scaffolded, not implemented. |

`serial` is on by default because it needs no extra system library; `usb`, `bluetooth`, and
`plugin` are off because each adds real build cost — libusb is compiled from source, and wasmtime
adds minutes to a cold build.

```sh
# Everything, including the hardware and WebAssembly backends
cargo build --release -p htmlapp \
  --features layer-shell,htmlapp-api/usb,htmlapp-api/bluetooth,htmlapp-api/plugin

# A minimal build: Tier 1 only, no hardware backends
cargo build --release --no-default-features \
  --features htmlapp-engine/wry-backend,htmlapp-api/tier1
```

A feature that is off does not make a module vanish from the catalog or the TypeScript. It makes its
methods return `unsupported` with a message naming the feature — so a document gets a clear
explanation rather than a mysterious `methodNotFound`.

---

## Shipping an app you wrote

```sh
htmlapp build tool.hta
```

```
binary     tool                     standalone; the document stapled to a copy of the runtime
desktop    tool.desktop             with the icon extracted from the manifest
appdir     tool.AppDir              ready for appimagetool
flatpak    com.example.tool.json    Flatpak manifest
```

| Flag | |
|---|---|
| `-o, --out <DIR>` | Where to write (default `.`) |
| `--no-appimage` | Skip the AppDir |
| `--no-flatpak` | Skip the Flatpak manifest |

`./tool` runs its own embedded document with no arguments. The launcher never appears, and the
person you give it to never learns the word "HTML App".

### How stapling works

The document is **appended** to a copy of the runtime, followed by a 24-byte trailer:

```
[ runtime binary ][ document bytes ][ "HTMLAPP\0STAP" | len: u64 LE | version: u32 LE ]
```

At startup the runtime reads the last 24 bytes. Magic present and the length plausible → run the
embedded document. Otherwise → normal CLI. This check runs *before* argument parsing, because a
built app's flags are its own business.

Appending rather than embedding is what keeps this a **copy**, not a build: the runtime is already
compiled, so producing a standalone app is a file write rather than a toolchain invocation. That is
what makes single-file distribution compatible with zero-build.

Magic bytes appearing in the runtime's own body do not confuse extraction — the constant is
compiled into the real binary, so there is a test for exactly that.

---

## Packaging the runtime

Recipes in [`packaging/`](../packaging).

### Debian

```sh
dpkg-buildpackage -us -uc -b
```

`packaging/debian/rules` installs the binary, the `.desktop` entry, the MIME package, the icon, and
the bundled examples. Registration is part of the install, not a post-install step that might not
run.

### Arch

```sh
cd packaging/aur && makepkg -si
```

`optdepends` covers `xdg-desktop-portal`, `bubblewrap`, and `gtk-layer-shell`, so pacman explains
what each one unlocks.

### Nix

```sh
nix run .                # the launcher
nix run . -- doc.hta     # a document
nix develop              # a shell with every dependency, including the optional ones
```

The flake wraps the binary with `LD_LIBRARY_PATH`, because Vulkan and the X/Wayland client libraries
are loaded at runtime rather than linked.

### Flatpak

```sh
flatpak-builder build packaging/flatpak/app.htmlapp.HtmlApp.yml
```

The confinement is deliberately tight. A runtime shipping with `--filesystem=home` would quietly
overrule every manifest in every document it ran, which is exactly the failure the whole model
exists to prevent. Documents reach the filesystem through the portal instead, so your own file
chooser is the grant.

Not granted, on purpose: `--filesystem=home`, `org.freedesktop.systemd1`, `--device=all`.

### AppImage

```sh
./packaging/appimage/build-appimage.sh
```

Bundles WebKitGTK and its dependencies — deliberately excluding glibc and the GL/Vulkan loaders,
which is how an AppImage breaks on a newer host.

An AppImage cannot register a MIME type by itself. Run `./HtmlApp-x86_64.AppImage install` once to
associate `.hta` files.

### Installer

```sh
curl -fsSL https://htmlapp.dev/install.sh | sh
```

Installs to `~/.local/bin`, verifies the published checksum when there is one, and runs
`htmlapp install`. Never runs as root and never touches anything outside your home directory.

---

## Desktop integration

`htmlapp install` writes:

| | |
|---|---|
| `~/.local/share/applications/htmlapp.desktop` | `Exec=htmlapp %f` |
| `~/.local/share/mime/packages/htmlapp.xml` | `application/hta`, `application/x-hta` |
| `~/.local/share/icons/hicolor/scalable/apps/htmlapp.svg` | |

then runs `update-desktop-database` and `update-mime-database`, and reports if either fails. An
install that leaves double-click broken is a failed install, so it exits non-zero.

`--system` writes to `/usr/share` instead.

`%f` is the whole mechanism: the application menu passes no argument and the launcher appears, while
a file manager passes a path and the document runs. One entry serves both.

**`.html` is deliberately not hijacked.** The extension is the signal that a file expects a runtime,
and taking `.html` would break every browser on the system.

---

## The CLI

```
htmlapp [OPTIONS] [DOCUMENT] [COMMAND]
```

| | |
|---|---|
| `htmlapp` | The launcher |
| `htmlapp doc.hta` | Run a document |
| `--headless` | No window; stdin → stdout |
| `--format <FMT>` | Passed to the page as `htmlapp.format` |
| `--sandbox` | Re-exec under bubblewrap |
| `--devtools` | Make the inspector reachable |
| `--private` | Do not record in recents |
| `--no-permissions` | Run powerless whatever the manifest asks |
| `--open` | Go straight to the file chooser |
| `--permissions-ui` | Open the consent manager |
| `-v`, `-vv`, `-vvv` | Verbosity (or set `HTMLAPP_LOG`) |

| Subcommand | |
|---|---|
| `inspect <doc>` | Manifest, hash, requested permissions, consent status |
| `permissions [list\|allow\|deny\|revoke\|revoke-all]` | Manage stored decisions |
| `build <doc>` | Produce a standalone app |
| `types` | Emit TypeScript declarations |
| `cache [info\|purge]` | The pinned-module cache |
| `install` / `uninstall` | Desktop registration |

---

## Configuration

`~/.config/htmlapp/config.toml`, all optional:

```toml
launcher = true       # false makes a bare `htmlapp` print help instead
recents = true        # false is a permanent --private
sandbox = false       # true is a permanent --sandbox
devtools = false
# color_scheme = "dark"
```

Unknown keys are an error rather than a warning: a typo like `launchre = false` would otherwise
leave the launcher on while you believe you turned it off.

---

## Paths

| | |
|---|---|
| `~/.config/htmlapp/config.toml` | Configuration |
| `~/.local/share/htmlapp/consent.json` | Permission decisions |
| `~/.local/share/htmlapp/recents.json` | Launcher recents |
| `~/.local/share/htmlapp/apps/<id>/` | Per-document `store` data |
| `~/.cache/htmlapp/modules/` | Pinned import cache |
| `/usr/share/htmlapp/examples/` | Bundled examples |

---

[← Architecture](architecture.md) · **Building and packaging** · [Contributing →](contributing.md)
