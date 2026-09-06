# Spotify TUI

A local-first Spotify controller and visualizer for `spotifyd`. The current
Linux runtime uses the session MPRIS interface for playback and controls, so it
does not require Spotify Web API access. Version 1 targets a Nix flake on Linux
and Homebrew on macOS, plus direct POSIX and Windows installers. The macOS and
Windows playback adapters are still to be implemented.

The project is under active development. Linux playback uses Spotifyd's MPRIS
interface. macOS and Windows builds compile and have release packaging, but
their native playback adapters are not implemented yet; those packages must not
be described as functionally complete until the platform acceptance tests pass.

## Installation

The intended canonical repository is `kylescudder/spotify-tui`, matching the
owner used by the existing dotfiles repositories. It has not been created or
released yet, so these commands become live after the external repository setup
and first release. The release workflow stamps the actual repository into both
direct installers, which also keeps forks functional.

### Nix

Run directly from the flake:

```bash
nix run github:kylescudder/spotify-tui
```

Or install it into the current profile:

```bash
nix profile install github:kylescudder/spotify-tui
```

The flake exposes `packages.default`, `apps.default`, `checks`, a development
shell, and a Home Manager module on `x86_64-linux` and `aarch64-linux`. A Home
Manager configuration can consume it with:

```nix
# flake.nix
{
  inputs.spotify-tui.url = "github:kylescudder/spotify-tui";
}
```

Then import it from a Home Manager module where your flake inputs are available:

```nix
{ inputs, ... }:
{
  imports = [ inputs.spotify-tui.homeManagerModules.default ];
  programs.spotify-tui.enable = true;
}
```

The module installs Spotify TUI and enables Spotifyd with session MPRIS. Set
`programs.spotify-tui.enableSpotifyd = false` to preserve a separately managed
Spotifyd service.

### Homebrew on macOS

Spotify TUI is published through the existing
[`kylescudder/tap`](https://github.com/kylescudder/homebrew-tap) tap:

```bash
brew install kylescudder/tap/spotify-tui
```

The formula depends on `spotifyd` and `cava`. The release workflow styles,
audits, builds, installs, and tests the formula on macOS before opening its tap
update pull request.

### Direct installer on Linux or macOS

```bash
curl --proto '=https' --proto-redir '=https' --tlsv1.2 -LsSf \
  https://github.com/kylescudder/spotify-tui/releases/latest/download/install.sh | sh
```

The installer detects the platform, downloads the matching release archive,
checks it against `SHA256SUMS`, and installs Spotify TUI, its diagnostic, and a
pinned Spotifyd runtime to `$HOME/.local/bin` without `sudo`. An existing
Spotifyd binary or configuration is preserved. A new Linux installation gets a
session-MPRIS configuration and an enabled systemd user service; macOS gets a
user LaunchAgent.

The bundled Linux Spotifyd is dynamically linked to the normal ALSA/PulseAudio,
D-Bus, OpenSSL, and system runtime libraries. The installer does not invoke a
system package manager to add those libraries. NixOS users should use the Nix
flake, which supplies the complete runtime closure.

To inspect the script or pin a version:

```bash
curl --proto '=https' --proto-redir '=https' --tlsv1.2 -LsSf \
  https://github.com/kylescudder/spotify-tui/releases/latest/download/install.sh \
  -o install-spotify-tui.sh
less install-spotify-tui.sh
sh install-spotify-tui.sh --version 0.1.0 --install-dir "$HOME/.local/bin"
```

Useful installer controls are:

| Option | Purpose |
| --- | --- |
| `--config-dir DIRECTORY` | Override the directory for a newly created `spotifyd.conf`. |
| `--no-dependencies` | Install only Spotify TUI and its diagnostic. |
| `--force-dependencies` | Replace an existing local Spotifyd with the bundled pinned build. |
| `--no-service` | Do not create or enable the systemd user unit or LaunchAgent. |

The direct installer does not invoke or modify Homebrew, Nix, Apt, or another
system package manager.

### Direct installer on Windows

From PowerShell:

```powershell
irm https://github.com/kylescudder/spotify-tui/releases/latest/download/install.ps1 | iex
```

The default destination is
`%LOCALAPPDATA%\Programs\spotify-tui\bin`, which is added to the user PATH. The
release archive contains Spotifyd built from the pinned upstream source with
the portable Rodio backend. The installer preserves an existing Spotifyd,
creates a minimal config when needed, and adds a user Startup entry.
The inspect-first, version-pinned form is:

```powershell
Invoke-WebRequest `
  https://github.com/kylescudder/spotify-tui/releases/latest/download/install.ps1 `
  -OutFile install-spotify-tui.ps1
Get-Content .\install-spotify-tui.ps1
.\install-spotify-tui.ps1 -Version 0.1.0 -NoModifyPath
```

PowerShell accepts `-ConfigDir`, `-NoDependencies`, `-ForceDependencies`,
`-NoService`, and `-NoModifyPath` for the equivalent Windows controls.

### Release verification

Every release includes `SHA256SUMS`, GitHub build-provenance attestations, the
Spotifyd GPLv3 licence, and the complete source corresponding to the bundled
Spotifyd binary.
After downloading an artifact, verify its checksum and provenance with:

```bash
sha256sum --check --ignore-missing SHA256SUMS
gh attestation verify spotify-tui-x86_64-unknown-linux-musl.tar.gz \
  --repo kylescudder/spotify-tui
```

### Uninstall

Use `nix profile remove`, `brew uninstall spotify-tui`, or remove the files
installed by the direct installer. Only remove `spotifyd` here if the direct
installer supplied it rather than preserving an existing installation:

```bash
rm "$HOME/.local/bin/spotify-tui" "$HOME/.local/bin/spotify-tui-diagnose"
rm "$HOME/.local/bin/spotifyd"
```

On Windows, remove `spotify-tui.exe`, `spotify-tui-diagnose.exe`, and a
direct-installer-owned `spotifyd.exe` from
`%LOCALAPPDATA%\Programs\spotify-tui\bin`; remove the generated `spotifyd.cmd`
from the user Startup directory and then remove the install directory from the
user PATH if the installer added it. Configuration and OAuth credentials are
deliberately retained during uninstall.

## Development

Run from a Rust checkout with:

```bash
cargo run
```

On Linux, the TUI connects to Spotifyd over the graphical session's MPRIS bus.
It updates from D-Bus signals and reconnects automatically if Spotifyd stops and
comes back. When Spotifyd is running but inactive, the TUI asks Spotifyd to
transfer playback to itself automatically. An official Spotify client or phone
is not required to activate the device. This uses Spotifyd's documented
[`TransferPlayback` D-Bus control](https://docs.spotifyd.rs/advanced/dbus.html).

### Playback controls

| Key | Action |
| --- | --- |
| `Space` | Play or pause. |
| `p` | Previous track. |
| `n` | Next track. |
| `h` or `Left` | Seek backward 5 seconds. |
| `l` or `Right` | Seek forward 5 seconds. |
| `j` or `Down` | Lower volume by 5%. |
| `k` or `Up` | Raise volume by 5%. |
| `a` | Leave the TUI temporarily and run Spotifyd authentication. |
| `r` | Retry the local playback connection immediately. |
| `q`, `Esc`, or `Ctrl-C` | Quit. |

Inspect the current normalized MPRIS state without starting the TUI with:

```bash
cargo run --bin spotify-tui-diagnose
```

The diagnostic is read-only and reports `connection: disconnected` until
Spotifyd has created its MPRIS player. The main TUI performs the additional
activation step automatically, so use it—not the diagnostic—to test first-run
device activation.

## Authentication

Spotify TUI does not collect a Spotify password or implement its own OAuth
client. Authentication is delegated to the installed `spotifyd` binary, which
stores and owns the resulting credential.

Authenticate once before the first normal launch:

```bash
spotify-tui auth
```

Spotifyd prints a browser URL. Open it, sign into Spotify, approve the
connection, and return to the terminal. After successful authentication,
Spotify TUI attempts to restart the `spotifyd.service` systemd user unit so the
new credential is picked up immediately. Authentication remains successful if
that restart is unavailable; a warning explains how to recover.

The disconnected, connecting, and error screens offer the same flow without
leaving the application permanently:

```text
a  leave the TUI temporarily and authenticate with spotifyd
r  retry the local connection
q  quit
```

Arguments after `auth` are forwarded to `spotifyd authenticate`. A leading `--`
is optional and useful for clarity:

```bash
spotify-tui auth -- --oauth-port 9876
spotify-tui auth -- --config-path /path/to/spotifyd.conf
```

Packaging wrappers can use these environment variables when Spotifyd is not in
the normal path or its generated configuration lives elsewhere:

| Variable | Default | Purpose |
| --- | --- | --- |
| `SPOTIFY_TUI_SPOTIFYD` | `spotifyd` | Spotifyd executable or absolute path. |
| `SPOTIFY_TUI_SPOTIFYD_CONFIG` | Unset | Config path passed to Spotifyd before the `authenticate` subcommand. |
| `SPOTIFY_TUI_SPOTIFYD_SERVICE` | `spotifyd.service` | systemd user unit restarted after successful authentication. |
| `SPOTIFY_TUI_SYSTEMCTL` | `systemctl` | `systemctl` executable or absolute path. |

Spotifyd also supports Spotify Connect discovery as an alternative: start the
daemon and select its device from an official Spotify client on the same local
network. This is optional on Linux: after authentication, Spotify TUI uses
Spotifyd's local D-Bus control interface to activate the device without a phone.
If Spotify has a resumable context, press `Space` to continue it. To guarantee a
specific context on a fresh session, configure `playback.startup_uri` as
described below.

Spotifyd requires a Spotify Premium account. The current MPRIS and systemd
integration is Linux-only. Homebrew on macOS and a PowerShell installer on
Windows are version-1 distribution targets, but their playback and service
adapters must be completed before those packages are called functionally
complete.

## Configuration

Configuration uses TOML and is optional. If no file is found, Spotify TUI starts
with the built-in `spotify` colour scheme.

### Config file location

The first applicable location wins:

1. The path in `SPOTIFY_TUI_CONFIG`, when the environment variable is set.
2. `$XDG_CONFIG_HOME/spotify-tui/config.toml`, when `XDG_CONFIG_HOME` is set.
3. `$HOME/.config/spotify-tui/config.toml`.
4. Built-in defaults when none of those paths can be resolved or the default
   config file does not exist.

An explicitly selected `SPOTIFY_TUI_CONFIG` file must exist. Unreadable files,
unknown options, and invalid values produce an actionable error before the
application enters raw terminal mode.

To try a file without installing it permanently:

```bash
SPOTIFY_TUI_CONFIG=/path/to/config.toml cargo run
```

### Top-level options

| Option | Required | Default | Description |
| --- | --- | --- | --- |
| `version` | Yes | — | Configuration schema version. The current and only supported value is `1`. |
| `theme` | No | `"spotify"` | Active built-in theme or a custom name declared under `[themes]`. |

The built-in theme names are `spotify`, `midnight`, and `high-contrast`:

```toml
version = 1
theme = "midnight"
```

### Playback options

Playback options live under `[playback]`:

| Option | Required | Default | Description |
| --- | --- | --- | --- |
| `startup_uri` | No | Unset | Spotify URI opened after the TUI activates an inactive Spotifyd session. |

Set `startup_uri` when the user should always have something playable without
first choosing the device from another Spotify client:

```toml
version = 1

[playback]
startup_uri = "spotify:playlist:37i9dQZF1DXcBWIGoYBM5M"
```

The value must be a Spotify URI such as `spotify:track:...`,
`spotify:album:...`, or `spotify:playlist:...`; web URLs are rejected. It is
opened only as part of activating Spotifyd, not every time the TUI starts while
Spotifyd already has an active player. Leave it unset to preserve and resume
Spotify's existing context.

### Custom theme options

Declare a custom theme with `[themes.<name>]`, then select that name with the
top-level `theme` option. Every custom theme option is optional: omitted colours
come from its `base`.

| Option | Default | UI role |
| --- | --- | --- |
| `base` | `"spotify"` | Built-in theme to inherit from. Must be `spotify`, `midnight`, or `high-contrast`; custom themes cannot inherit from other custom themes. |
| `background` | From `base` | Terminal canvas and panel background. |
| `foreground` | From `base` | Primary text and values. |
| `muted` | From `base` | Help text, secondary metadata, and disconnected states. |
| `border` | From `base` | Panel borders and dividers. |
| `accent` | From `base` | Title, active controls, and connected/playing states. |
| `warning` | From `base` | Connecting, loading, and other attention states. |
| `error` | From `base` | Playback and runtime error states. |

Custom names may contain any TOML-compatible key characters but cannot replace
a built-in theme name. This example shows every available custom theme option:

```toml
version = 1
theme = "ocean"

[themes.ocean]
base = "midnight"
background = "#07111f"
foreground = "#dbeafe"
muted = "dark-gray"
border = "#274060"
accent = "#38bdf8"
warning = "light-yellow"
error = "light-red"
```

### Colour values

Every colour option accepts either a six-digit RGB value such as `"#1ed760"` or
one of these case-insensitive terminal colour names:

```text
default, reset,
black, red, green, yellow, blue, magenta, cyan, white,
gray, grey, dark-gray, dark-grey,
light-red, light-green, light-yellow, light-blue, light-magenta, light-cyan
```

Underscores can be used instead of hyphens, so `"light_cyan"` and
`"light-cyan"` are equivalent.

### Catppuccin Mocha example

A complete ready-to-use configuration is included at
[themes/catppuccin-mocha.toml](themes/catppuccin-mocha.toml). Run it directly
from the repository with:

```bash
SPOTIFY_TUI_CONFIG=themes/catppuccin-mocha.toml cargo run
```

## Development

Run all repository checks with:

```bash
make check
```

The individual commands are `make format`, `make format-check`, `make lint`,
and `make test`.

See [HANDOVER.md](HANDOVER.md) for the product boundary, architecture, execution
sequence, and acceptance checks.
