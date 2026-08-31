<p align="center">
  <a href="https://dele.to">
    <img src="./.github/dele-to-logo.png" width="100" height="100" alt="DELE.TO">
  </a>
</p>

<h1 align="center">🦀 DELE.TO CLI 🦀</h1>

<p align="center">
  Share secrets that disappear — directly from your terminal.
</p>

<p align="center">
  <a href="https://dele.to">Website</a> ·
  <a href="https://dele.to/developers">API keys</a> ·
  <a href="https://github.com/dele-to/dele-to">DELE.TO on GitHub</a>
</p>

The official command-line client for [DELE.TO](https://dele.to). It encrypts secrets locally, uploads only an opaque encrypted payload, and returns a link that expires after a chosen time or number of views.

<p align="center">
  <a href="https://dele.to/#cli">
    <img src="https://dele.to/cli-tui.png" alt="DELE.TO terminal interface" width="820">
  </a>
  <br>
  <sub>Run <code>deleto</code> to open the TUI. Click the image for the interactive demo.</sub>
</p>

## Install

**macOS and Linux**

```sh
curl -fsSL https://dele.to/install.sh | sh
```

**Windows PowerShell**

```powershell
irm https://dele.to/install.ps1 | iex
```

Or build from source with Rust:

```sh
cargo install --path . --locked
```

## Use it

```sh
# Open the interactive TUI
deleto

# Share text
deleto 'the launch code is 1234'

# Pipe from another command
printf '%s' "$DATABASE_URL" | deleto

# Share a file with custom limits
deleto --file .env --expires 15m --views 1

# Decrypt a share in the terminal
deleto view 'https://dele.to/view/<id>#<fragment>'

# Include expiry and the private delete capability
deleto --receipt 'temporary credential'
```

Run `deleto --help` for every command and option.

## Why it is safe

- Secrets are encrypted on your machine with AES-256-GCM.
- The root secret stays in the URL fragment (`#...`), which is not sent to the server.
- The API receives an opaque encrypted payload plus expiry and view-limit metadata — never your plaintext.
- Shares automatically expire and can be limited to a single view.
- A private delete capability lets the creator revoke a share early.

The complete share URL is sensitive: anyone who has it can open the share while it remains available.

## Optional analytics

DELE.TO CLI sends anonymous product analytics by default to help improve the installer and CLI. Analytics are **optional** and do not include secret contents, file contents, share URLs, API keys, or capabilities.

CLI events can include the command outcome, input type, content length, expiry, view limit, OS, CLI version, and a randomly generated anonymous ID stored at `~/.deleto/anonymous-id`. Installer events can include the OS, architecture, downloaded asset, and install stage.

Disable analytics at any time:

```sh
export DELETO_NO_ANALYTICS=1
```

For a single command:

```sh
DELETO_NO_ANALYTICS=1 deleto 'secret'
```

The installers also respect `DELETO_NO_ANALYTICS`, `DATABUDDY_DISABLED`, and `DO_NOT_TRACK`.

## Configuration

| Variable | Purpose |
| --- | --- |
| `DELETO_API_KEY` | Optional API key for higher limits |
| `DELETO_API_URL` | API origin; defaults to `https://dele.to` |
| `DELETO_NO_ANALYTICS` | Set to `1` to disable anonymous analytics |

Create an API key at [dele.to/developers](https://dele.to/developers), or point the CLI at an isolated [Deleto Cloud](https://dele.to/cloud) instance with `DELETO_API_URL`.

## Development

```sh
cargo test
cargo run -- 'hello from source'
```

## License

MIT
