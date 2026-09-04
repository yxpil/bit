# BIT — Agent Tool Hub

**English** | [简体中文](README.md)

[![Release](https://img.shields.io/github/v/release/yxpil/OpenBit?style=flat-square&label=%E7%89%88%E6%9C%AC)](https://github.com/yxpil/OpenBit/releases/latest) [![下载](https://img.shields.io/github/downloads/yxpil/OpenBit/total?style=flat-square&label=%E4%B8%8B%E8%BD%BD)](https://github.com/yxpil/OpenBit/releases) [![License](https://img.shields.io/github/license/yxpil/OpenBit?style=flat-square)](https://github.com/yxpil/OpenBit/blob/main/LICENSE) [![CI](https://img.shields.io/github/actions/workflow/status/yxpil/OpenBit/release.yml?style=flat-square&branch=main&label=CI)](https://github.com/yxpil/OpenBit/actions) [![平台](https://img.shields.io/badge/%E5%B9%B3%E5%8F%B0-macOS%20%C2%B7%20Windows%20%C2%B7%20Linux%20%C2%B7%20%E9%BE%99%E8%8A%AF%20%C2%B7%20RISC--V-black?style=flat-square)](https://osbt.space) [![官网](https://img.shields.io/website?up_message=osbt.space&down_message=%E7%A6%BB%E7%BA%BF&style=flat-square&url=https%3A%2F%2Fosbt.space)](https://osbt.space) [![QQ群](https://img.shields.io/badge/QQ%E7%BE%A4-%E7%82%B9%E5%87%BB%E5%8A%A0%E5%85%A5-black?style=flat-square)](https://qm.qq.com/q/qlFr8ct0ps)

[![Homebrew](https://img.shields.io/badge/Homebrew-brew%20install%20--cask%20bit-black?style=flat-square)](https://github.com/yxpil/homebrew-bit) [![Scoop](https://img.shields.io/badge/Scoop-scoop%20install%20bit-black?style=flat-square)](https://github.com/yxpil/scoop-bit) [![npm](https://img.shields.io/badge/npm-bit--agent-black?style=flat-square)](https://www.npmjs.com/package/bit-agent) [![winget](https://img.shields.io/badge/winget-yxpil.bit-black?style=flat-square)](https://github.com/microsoft/winget-pkgs/pull/428288) [![APT](https://img.shields.io/badge/APT-yxpil%2Fapt--repo-black?style=flat-square)](https://yxpil.github.io/apt-repo) [![DNF](https://img.shields.io/badge/DNF-yxpil%2Fdnf--repo-black?style=flat-square)](https://yxpil.github.io/dnf-repo) [![pacman](https://img.shields.io/badge/pacman-yxpil%2Fpacman--repo-black?style=flat-square)](https://yxpil.github.io/pacman-repo)

BIT is a desktop app built on **Tauri 2 + React 18**: an auditable, remotely accessible AI Agent tool hub. Configure any AI provider, chat with models over streaming, and let the AI call local tools, write its own scripts, and accumulate memory and skills.

**BIT is free forever**: fully open source (Apache-2.0), every feature free for individuals and businesses — no in-app purchases, no subscriptions, no locked features, no telemetry; build it yourself from source anytime.

> Frameless custom title bar · Dark/light themes · Minimal black-and-white design · [QQ group](https://qm.qq.com/q/qlFr8ct0ps)

## Table of Contents

[Features](#features) · [Tech Stack](#tech-stack) · [Installation](#installation) · [Remote Access & API](#remote-access--api) · [Documentation](#documentation) · [Security & Privacy](#security--privacy) · [Development](#development) · [Project Structure](#project-structure) · [License](#license)

## Features

**Chat & AI**

- **Streaming chat**: end-to-end streaming across frontend and backend (SSE + Tauri Event); replies render character by character; the thinking process is shown separately; assistant messages support Markdown rendering (tables, code blocks, etc.); real-time cache-hit-rate stats.
- **Multi-provider**: the OpenAI / Gemini / Claude protocols; multiple providers can be configured but **only one is active at a time** (mutually exclusive); test upstream connectivity before saving; "Fetch from API" pulls the model list in one click.
- **Multimodal**: image input is supported and sent along with the message to multimodal models.
- **Terminal mode**: type `bit` in any terminal to enter a minimal TUI — no window, no single-instance constraint, no port listening. Ideal for SSH / headless environments.

**Agent capabilities**

- **Tool hub**: register, enable/disable, and invoke tools; auto-detects and registers local interpreters (JS / Python, etc.) — the AI only needs to write a script that can communicate and it becomes a tool; tools hot-reload.
- **AI-built capabilities**: the AI can write its own plugins via built-in tools / execute scripts directly / promote scripts into persistent tools (executed inside the restricted Rhai sandbox).
- **Memory and skills**: the AI summarizes and stores knowledge by itself via the `add_memory` / `skill` tools, reused across sessions — no manual triggering.

**Protocols & integration**

- **MCP client**: connect to any standard MCP server (Streamable HTTP / JSON-RPC 2.0); external tools are merged into the registry automatically.
- **MCP server**: BIT itself also exposes a standard MCP endpoint (`POST /mcp`); any MCP client such as Claude Desktop can directly call all of BIT's enabled tools.
- **OpenAI-compatible endpoint**: `/v1/chat/completions` supports streaming, so third-party apps can use BIT as a local AI gateway.

**Reliability & governance**

- **Audit log**: all tool calls and key operations are logged, viewable on the **Audit** page.
- **Auto update**: checks, downloads, and swaps in new versions automatically on all platforms (can be disabled).
- **Local-first data**: sessions, memory, skills, and settings all stay on your machine.

## Tech Stack

| Layer | Technologies |
|----|------|
| Frontend | React 18, Vite 6, Tailwind CSS 4, react-markdown + remark-gfm |
| Desktop | Tauri 2 (Rust) |
| Backend | reqwest (with stream), tokio, axum, rhai, futures-util |

## Installation

Download the installer for your platform from [Releases](https://github.com/yxpil/OpenBit/releases):

| Platform | Installer | Notes |
|---|---|---|
| Windows x64 | `*-setup.exe` (NSIS) or `*.msi` | Double-click to install; ARM64 laptops (Snapdragon X) should pick the `aarch64` build |
| macOS Apple Silicon | `*_aarch64.dmg` | M-series chips |
| macOS Intel | `*_x64.dmg` | Drag into Applications to install |
| Linux x64 / ARM64 | `*.deb` / `*.AppImage` / `*.rpm` | Pick whichever fits your distro's convention |
| Loongson LoongArch64 (3A5000/3A6000) | `*_loongarch64.deb` / `*_loongarch64.tar.gz` | Double-click the deb to install; on other distros extract the binary |
| RISC-V 64 (VisionFive 2, etc.) | `*_riscv64.deb` / `*_riscv64.tar.gz` | Same as above |
| Phytium / Kunpeng / Kylin ARM | `*_arm64.deb` / `*_arm64.AppImage` / `*_arm64.rpm` | Same as Linux ARM64 |
| Zhaoxin / Hygon | `*_amd64.deb` / `*_amd64.AppImage` / `*_x86_64.rpm` | Same as Linux x64 |

> For the full breakdown of supported CPU architectures and operating systems (including the Phytium / Kunpeng / Kylin / UOS / ChromeOS matrix), see the [Wiki: Chips and OS Support](https://github.com/yxpil/OpenBit/wiki/Chips-and-OS-Support).

### ChromeOS (Crostini) Installation

ChromeOS ships with a built-in Linux development environment (a Debian 12 container), so BIT's Linux packages work directly — no special build needed:

1. Settings → About ChromeOS → Developers → Linux development environment → Enable (supported on both Intel/AMD and ARM devices)
2. Install from the Linux terminal (amd64 for Intel/AMD, arm64 for ARM):

```bash
sudo apt install ./BIT_0.5.8_amd64.deb
```

The dependencies (libwebkit2gtk-4.1, libgtk-3, libayatana-appindicator3) are pulled in automatically from the Debian 12 repositories. After installation BIT appears in the "Linux apps" folder; the window is displayed via Wayland, matching the native Linux experience.


### Package Managers

macOS (Homebrew):

```bash
brew tap yxpil/OpenBit
brew install --cask bit
```

Windows (Scoop):

```powershell
scoop bucket add bit https://github.com/yxpil/scoop-bit
scoop install bit
```

npm (cross-platform; automatically downloads the app for your platform):

```bash
npm install -g bit-agent
bit-agent   # Start BIT
```

Windows (winget, under review): `winget install yxpil.bit`

Debian / Ubuntu / UOS / Kylin (APT repository):

```bash
echo "deb [trusted=yes] https://yxpil.github.io/apt-repo stable main" | sudo tee /etc/apt/sources.list.d/bit.list
sudo apt update && sudo apt install bit
```

Fedora / RHEL / openSUSE (dnf repository):

```bash
sudo tee /etc/yum.repos.d/bit.repo <<'EOF'
[bit]
name=BIT
baseurl=https://yxpil.github.io/dnf-repo
enabled=1
gpgcheck=0
EOF
sudo dnf install bit
```

Arch / Manjaro (pacman repository):

```bash
echo "
[bit]
Server = https://yxpil.github.io/pacman-repo/\$arch
SigLevel = Never" | sudo tee /etc/pacman.d/bit.conf
# Add this line before [core] in /etc/pacman.conf: Include = /etc/pacman.d/bit.conf
sudo pacman -Sy bit
```

### macOS Says the App "Is Damaged and Can't Be Opened"?

BIT does not currently hold a paid Apple Developer certificate ($99/year) and is signed ad-hoc. macOS blocks apps **downloaded from the internet** by default, and newer systems report them as "damaged" outright. Any of the following fixes it:

**Option 1: Remove the quarantine attribute (most reliable, recommended)**

```bash
# Run once after installing
xattr -cr /Applications/BIT.app
```

**Option 2: Allow it in System Settings**

1. Double-click the dmg to install. If a warning appears on first launch, **do not click "Move to Trash" yet**
2. Open System Settings → Privacy & Security → scroll down to the Security section → click **"Open Anyway"**

**Option 3: Right-click to open (macOS 14 and earlier)**

Control-click (or right-click) BIT → choose "Open" → click "Open" again to confirm.

> Why it works: `xattr -cr` removes the file's quarantine attribute; the signature itself is intact and verifiable, so once quarantine is removed macOS no longer blocks it.

### Windows SmartScreen Warning on First Launch?

The installer is not code-signed (EV certificates also cost money). When the SmartScreen prompt appears, click **"More info" → "Run anyway"**.

### Running the AppImage on Linux

```bash
chmod +x BIT_0.5.8_amd64.AppImage
./BIT_0.5.8_amd64.AppImage
```

### First Run

Open **AI Settings** → add a provider (protocol / Base URL / API Key / model; click "Fetch from API" to pull the model list) → click the play button to activate it → go back to **Chat** and start using it.

For terminal users: after installing, type `bit` in any terminal to jump straight into the TUI chat.

## Remote Access & API

Once enabled on the **Remote** page, BIT serves an HTTP API (default `127.0.0.1:8600`; switch to `0.0.0.0` for LAN access; disabled by default — the client key and access password are generated automatically when you turn it on).

**Authentication**

- **Client Key** (`bit_` prefix, generated automatically): `Authorization: Bearer <key>` or `?key=<key>` — used for `/v1/*` and `/mcp`
- **Access password**: `/api/*` admin endpoints additionally require an `X-Access-Password` header (OpenAI / MCP clients cannot carry custom headers, so those endpoints are exempt)

**Endpoints**

| Endpoint | Method | Description |
|---|---|---|
| `/v1/chat/completions` | POST | OpenAI-compatible chat (SSE streaming supported) |
| `/v1/models` | GET | Model list |
| `/mcp` | POST / DELETE | Standard MCP server (Streamable HTTP / JSON-RPC 2.0) |
| `/api/*` | — | Admin endpoints for sessions / settings / audit (access password required) |
| `/api/health` | GET | Health check (no auth) |

```bash
curl http://127.0.0.1:8600/v1/chat/completions \
  -H "Authorization: Bearer $BIT_KEY" \
  -H "Content-Type: application/json" \
  -d '{"model":"YOUR_MODEL","messages":[{"role":"user","content":"list the current directory"}]}'
```

See the wiki for details: [Remote Access & API](https://github.com/yxpil/OpenBit/wiki/Remote-Access-and-API) · [MCP Integration](https://github.com/yxpil/OpenBit/wiki/MCP-Integration)

## Documentation

Full documentation lives in the [Wiki](https://github.com/yxpil/OpenBit/wiki) (bilingual — every page has a language switcher at the top):

- **Usage**: [Installation Guide](https://github.com/yxpil/OpenBit/wiki/Installation-Guide) · [Chips & OS Support](https://github.com/yxpil/OpenBit/wiki/Chips-and-OS-Support) · [Quick Start](https://github.com/yxpil/OpenBit/wiki/Quick-Start) · [Chat Features](https://github.com/yxpil/OpenBit/wiki/Chat-Features) · [TUI Terminal Mode](https://github.com/yxpil/OpenBit/wiki/TUI-Terminal-Mode) · [FAQ](https://github.com/yxpil/OpenBit/wiki/FAQ-EN)
- **Advanced**: [Tool System](https://github.com/yxpil/OpenBit/wiki/Tool-System) · [MCP Integration](https://github.com/yxpil/OpenBit/wiki/MCP-Integration) · [Memory & Skills](https://github.com/yxpil/OpenBit/wiki/Memory-and-Skills) · [Remote Access & API](https://github.com/yxpil/OpenBit/wiki/Remote-Access-and-API) · [Auto Update](https://github.com/yxpil/OpenBit/wiki/Auto-Update) · [Audit Log](https://github.com/yxpil/OpenBit/wiki/Audit-Log)
- **Development**: [Development & Build](https://github.com/yxpil/OpenBit/wiki/Development-and-Build)

中文文档：[Wiki 首页](https://github.com/yxpil/OpenBit/wiki)（每页顶部可切换语言）。

Website: [osbt.space](https://osbt.space) · [Docs](https://osbt.space/docs.html)

## Security & Privacy

- **Local-first data**: sessions, memory, skills, and settings all stay in the app's local data directory — no telemetry uploaded.
- **Two-factor auth**: Client Key (compared in constant time to prevent timing side channels) + access password; remote access is off by default.
- **Sandboxing and limits**: AI-built scripts run inside the restricted Rhai sandbox (depth / operation / wall-clock budgets); subprocess tools get timeout kills, output caps, and resource reaping.
- **MCP session governance**: sessions idle out after 30 minutes, are capped in count, and can be explicitly terminated via DELETE.
- **Signing transparency**: macOS ad-hoc signing / no Windows EV certificate (a trade-off of not paying for certificates — see the installation notes above); the source and CI build pipeline are fully public.

## Development

Prerequisites: [Node.js](https://nodejs.org/), the [Rust](https://www.rust-lang.org/) toolchain, and Tauri's system dependencies.

```bash
npm install          # Install frontend dependencies
npm run tauri dev    # Dev mode (hot reload)
npm run tauri build  # Build release (NSIS / MSI / dmg / AppImage / deb)
```

## Project Structure

```
src/               React frontend
  pages/           Chat / AI Settings / Tools / Skills / Memory / Audit / Remote
  components/      Markdown, tool cards, icons, etc.
src-tauri/         Tauri (Rust) backend
  src/             ai, agent, mcp, registry, runtime, script_runtime,
                   http_api, update, audit, session, goal, memory …
installer/bit.iss  Inno Setup packaging script
```

## License

[Apache License 2.0](LICENSE) — **BIT is free forever**: all features with no in-app purchases, no subscriptions, and no locked features, free for personal and commercial use.
