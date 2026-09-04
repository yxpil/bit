# BIT — Agent Tool Hub

**English** | [简体中文](README.md)

[![Release](https://img.shields.io/github/v/release/yxpil/bit?style=flat-square&label=%E7%89%88%E6%9C%AC)](https://github.com/yxpil/bit/releases/latest) [![下载](https://img.shields.io/github/downloads/yxpil/bit/total?style=flat-square&label=%E4%B8%8B%E8%BD%BD)](https://github.com/yxpil/bit/releases) [![License](https://img.shields.io/github/license/yxpil/bit?style=flat-square)](https://github.com/yxpil/bit/blob/main/LICENSE) [![CI](https://img.shields.io/github/actions/workflow/status/yxpil/bit/release.yml?style=flat-square&branch=main&label=CI)](https://github.com/yxpil/bit/actions) [![平台](https://img.shields.io/badge/%E5%B9%B3%E5%8F%B0-macOS%20%C2%B7%20Windows%20%C2%B7%20Linux%20%C2%B7%20%E9%BE%99%E8%8A%AF%20%C2%B7%20RISC--V-black?style=flat-square)](https://osbt.space) [![官网](https://img.shields.io/website?up_message=osbt.space&down_message=%E7%A6%BB%E7%BA%BF&style=flat-square&url=https%3A%2F%2Fosbt.space)](https://osbt.space) [![QQ群](https://img.shields.io/badge/QQ%E7%BE%A4-%E7%82%B9%E5%87%BB%E5%8A%A0%E5%85%A5-black?style=flat-square)](https://qm.qq.com/q/qlFr8ct0ps)

BIT is a desktop app built on **Tauri 2 + React 18**: an auditable, remotely accessible AI Agent tool hub. Configure any AI provider, chat with models over streaming, and let the AI call local tools, write its own scripts, and accumulate memory and skills.

> Frameless custom title bar · Dark/light themes · Minimal black-and-white design · [QQ group](https://qm.qq.com/q/qlFr8ct0ps)

## Features

- **Streaming chat**: end-to-end streaming across frontend and backend (SSE + Tauri Event); model replies render character by character, and assistant messages support Markdown rendering (bold, lists, tables, code blocks, etc.).
- **Multi-provider AI settings**: supports the OpenAI / Gemini / Claude protocols; multiple providers can be configured but **only one is active at a time** (mutually exclusive). Upstream connectivity can be tested before saving.
- **Tool hub**: register, enable/disable, and invoke tools; detects and registers local interpreters (JS / Python, etc.) — the AI only needs to write a script that can communicate, and it becomes a tool.
- **AI-built capabilities**: the AI can write its own plugins via built-in tools / execute scripts directly / promote scripts into persistent tools.
- **Memory and skills**: the AI summarizes and stores knowledge by itself via the `add_memory` / `skill` tools — no manual triggering.
- **Audit log**: all tool calls and key operations are logged, viewable on the **Audit** page.
- **Remote access**: built-in HTTP API with client key and access password support; the key is generated automatically, no manual entry.

## Tech Stack

| Layer | Technologies |
|----|------|
| Frontend | React 18, Vite 6, Tailwind CSS 4, react-markdown + remark-gfm |
| Desktop | Tauri 2 (Rust) |
| Backend | reqwest (with stream), tokio, axum, rhai, futures-util |

## Installation

Download the installer for your platform from [Releases](https://github.com/yxpil/bit/releases):

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

> For the full breakdown of supported CPU architectures and operating systems (including the Phytium / Kunpeng / Kylin / UOS / ChromeOS matrix), see the [Wiki: Chips and OS Support](https://github.com/yxpil/bit/wiki/Chips-and-OS-Support).

### ChromeOS (Crostini) Installation

ChromeOS ships with a built-in Linux development environment (a Debian 12 container), so BIT's Linux packages work directly — no special build needed:

1. Settings → About ChromeOS → Developers → Linux development environment → Enable (supported on both Intel/AMD and ARM devices)
2. Install from the Linux terminal (amd64 for Intel/AMD, arm64 for ARM):

```bash
sudo apt install ./BIT_0.5.6_amd64.deb
```

The dependencies (libwebkit2gtk-4.1, libgtk-3, libayatana-appindicator3) are pulled in automatically from the Debian 12 repositories. After installation BIT appears in the "Linux apps" folder; the window is displayed via Wayland, matching the native Linux experience.


### Package Managers

macOS (Homebrew):

```bash
brew tap yxpil/bit
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
chmod +x BIT_0.5.6_amd64.AppImage
./BIT_0.5.6_amd64.AppImage
```

### First Run

Open **AI Settings** → add a provider (protocol / Base URL / API Key / model; click "Fetch from API" to pull the model list) → click the play button to activate it → go back to **Chat** and start using it.

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

[Apache License 2.0](LICENSE)
