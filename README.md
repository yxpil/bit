# BIT — Agent Tool Hub
 [虾跑分/clawscore](https://paofen.cocoloop.cn/report/ses_1788600989079_6apd3l) 
简体中文 | [English](README_EN.md)

[![Release](https://img.shields.io/github/v/release/yxpil/bit?style=flat-square&label=%E7%89%88%E6%9C%AC)](https://github.com/yxpil/bit/releases/latest) [![下载](https://img.shields.io/github/downloads/yxpil/bit/total?style=flat-square&label=%E4%B8%8B%E8%BD%BD)](https://github.com/yxpil/bit/releases) [![License](https://img.shields.io/github/license/yxpil/bit?style=flat-square)](https://github.com/yxpil/bit/blob/main/LICENSE) [![CI](https://img.shields.io/github/actions/workflow/status/yxpil/bit/release.yml?style=flat-square&branch=main&label=CI)](https://github.com/yxpil/bit/actions) [![平台](https://img.shields.io/badge/%E5%B9%B3%E5%8F%B0-macOS%20%C2%B7%20Windows%20%C2%B7%20Linux%20%C2%B7%20%E9%BE%99%E8%8A%AF%20%C2%B7%20RISC--V-black?style=flat-square)](https://osbt.space) [![官网](https://img.shields.io/website?up_message=osbt.space&down_message=%E7%A6%BB%E7%BA%BF&style=flat-square&url=https%3A%2F%2Fosbt.space)](https://osbt.space) [![QQ群](https://img.shields.io/badge/QQ%E7%BE%A4-%E7%82%B9%E5%87%BB%E5%8A%A0%E5%85%A5-black?style=flat-square)](https://qm.qq.com/q/qlFr8ct0ps)

[![Homebrew](https://img.shields.io/badge/Homebrew-brew%20install%20--cask%20bit-black?style=flat-square)](https://github.com/yxpil/homebrew-bit) [![Scoop](https://img.shields.io/badge/Scoop-scoop%20install%20bit-black?style=flat-square)](https://github.com/yxpil/scoop-bit) [![npm](https://img.shields.io/badge/npm-bit--agent-black?style=flat-square)](https://www.npmjs.com/package/bit-agent) [![winget](https://img.shields.io/badge/winget-yxpil.bit-black?style=flat-square)](https://github.com/microsoft/winget-pkgs/pull/428288) [![APT](https://img.shields.io/badge/APT-yxpil%2Fapt--repo-black?style=flat-square)](https://yxpil.github.io/apt-repo) [![DNF](https://img.shields.io/badge/DNF-yxpil%2Fdnf--repo-black?style=flat-square)](https://yxpil.github.io/dnf-repo) [![pacman](https://img.shields.io/badge/pacman-yxpil%2Fpacman--repo-black?style=flat-square)](https://yxpil.github.io/pacman-repo)

BIT 是一个基于 **Tauri 2 + React 18** 的桌面应用：一个可审计、可远程访问的 AI Agent 工具中枢。你可以配置任意 AI 提供方，与模型进行流式对话，并让 AI 调用本机工具、自写脚本、沉淀记忆与技能。

**BIT 永久免费**：完全开源（Apache-2.0），所有功能对个人与商业用户永久免费——无内购、无订阅、无功能锁、无遥测，可随时自行编译。

> 无边框自定义标题栏 · 深/浅色主题 · 黑白线性设计 · [QQ 交流群](https://qm.qq.com/q/qlFr8ct0ps)

## 目录

[功能特性](#功能特性) · [技术栈](#技术栈) · [安装使用](#安装使用) · [远程访问与 API](#远程访问与-api) · [文档](#文档) · [安全与隐私](#安全与隐私) · [开发](#开发) · [项目结构](#项目结构) · [许可](#许可)

## 功能特性

**对话与 AI**

- **流式对话**：前后端全链路流式（SSE + Tauri Event），回复逐字展示；思考过程独立显示；助手消息 Markdown 渲染（表格、代码块等）；缓存命中率实时统计。
- **多提供方**：OpenAI / Gemini / Claude 三种协议，可配置多家、同一时刻激活一个（互斥）；先测试上游连通性再保存；「从 API 获取」一键拉取模型列表。
- **多模态**：支持图片输入，随消息发给多模态模型。
- **终端模式**：终端直接输入 `bit` 进入简约 TUI——无窗口、无单实例约束、不监听端口，适合 SSH / 无桌面环境。

**Agent 能力**

- **工具中枢**：注册、启停、调用工具；自动探测并注册本机解释器（JS / Python 等），AI 只需写一段能通讯的脚本即可成为工具；工具热更新。
- **AI 自建能力**：AI 可通过内置工具自写插件 / 直接执行脚本 / 把脚本沉淀为常驻工具（Rhai 沙箱受限执行）。
- **记忆与技能**：AI 通过 `add_memory` / `skill` 工具自行总结沉淀，跨会话复用，无需手动触发。

**协议与集成**

- **MCP 客户端**：接入任意标准 MCP 服务器（Streamable HTTP / JSON-RPC 2.0），外部工具自动并入注册表。
- **MCP 服务器**：BIT 自身也暴露标准 MCP 端点（`POST /mcp`），Claude Desktop 等任何 MCP 客户端可直接调用 BIT 的全部启用工具。
- **OpenAI 兼容端点**：`/v1/chat/completions` 支持流式，第三方应用可把 BIT 当本地 AI 网关使用。

**可靠与治理**

- **审计日志**：所有工具调用与关键操作留痕，可在「审计」页查看。
- **自动更新**：全平台自动检查、下载、换装（可关闭）。
- **数据本地化**：会话、记忆、技能、配置全部保存在本机。

## 技术栈

| 层 | 技术 |
|----|------|
| 前端 | React 18、Vite 6、Tailwind CSS 4、react-markdown + remark-gfm |
| 桌面 | Tauri 2（Rust） |
| 后端 | reqwest（含 stream）、tokio、axum、rhai、futures-util |

## 安装使用

从 [Releases](https://github.com/yxpil/bit/releases) 下载对应平台的安装包：

| 平台 | 安装包 | 说明 |
|---|---|---|
| Windows x64 | `*-setup.exe`（NSIS）或 `*.msi` | 双击安装；ARM64 笔记本（骁龙 X）选 `aarch64` 版 |
| macOS Apple Silicon | `*_aarch64.dmg` | M 系列芯片 |
| macOS Intel | `*_x64.dmg` | 拖入 Applications 安装 |
| Linux x64 / ARM64 | `*.deb` / `*.AppImage` / `*.rpm` | 按发行版习惯选择 |
| 龙芯 LoongArch64（3A5000/3A6000） | `*_loongarch64.deb` / `*_loongarch64.tar.gz` | deb 双击安装；其他发行版解压二进制 |
| RISC-V 64（VisionFive 2 等） | `*_riscv64.deb` / `*_riscv64.tar.gz` | 同上 |
| 飞腾 / 鲲鹏 / 麒麟 ARM | `*_arm64.deb` / `*_arm64.AppImage` / `*_arm64.rpm` | 与 Linux ARM64 通用 |
| 兆芯 / 海光 | `*_amd64.deb` / `*_amd64.AppImage` / `*_x86_64.rpm` | 与 Linux x64 通用 |

> 全部支持的芯片架构与操作系统明细（含飞腾 / 鲲鹏 / 麒麟 / UOS / ChromeOS 矩阵）见 [Wiki：芯片与操作系统支持](https://github.com/yxpil/bit/wiki/%E8%8A%AF%E7%89%87%E4%B8%8E%E6%93%8D%E4%BD%9C%E7%B3%BB%E7%BB%9F%E6%94%AF%E6%8C%81)。

### ChromeOS（Crostini）安装

ChromeOS 内置 Linux 开发环境（Debian 12 容器），BIT 的 Linux 安装包可直接使用，无需专门版本：

1. 设置 → 关于 ChromeOS → 开发者 → Linux 开发环境 → 启用（Intel/AMD 与 ARM 机型均支持）
2. 在 Linux 终端安装（Intel/AMD 选 amd64，ARM 选 arm64）：

```bash
sudo apt install ./BIT_0.5.9_amd64.deb
```

依赖（libwebkit2gtk-4.1、libgtk-3、libayatana-appindicator3）会由 Debian 12 仓库自动补齐；安装后 BIT 出现在「Linux 应用」文件夹，窗口经 Wayland 显示，与原生 Linux 体验一致。


### 包管理器安装

macOS（Homebrew）：

```bash
brew tap yxpil/bit
brew install --cask bit
```

Windows（Scoop）：

```powershell
scoop bucket add bit https://github.com/yxpil/scoop-bit
scoop install bit
```

npm（跨平台，自动下载对应平台应用）：

```bash
npm install -g bit-agent
bit-agent   # 启动 BIT
```

Windows（winget，审核中）：`winget install yxpil.bit`

Debian / Ubuntu / UOS / 麒麟（APT 源）：

```bash
echo "deb [trusted=yes] https://yxpil.github.io/apt-repo stable main" | sudo tee /etc/apt/sources.list.d/bit.list
sudo apt update && sudo apt install bit
```

Fedora / RHEL / openSUSE（dnf 源）：

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

Arch / Manjaro（pacman 源）：

```bash
echo "
[bit]
Server = https://yxpil.github.io/pacman-repo/\$arch
SigLevel = Never" | sudo tee /etc/pacman.d/bit.conf
# 在 /etc/pacman.conf 的 [core] 前加一行：Include = /etc/pacman.d/bit.conf
sudo pacman -Sy bit
```

### macOS 提示"已损坏，无法打开"？

BIT 目前未购买 Apple 开发者证书（$99/年），采用 ad-hoc 签名。macOS 对**从网络下载**的应用默认拦截，新系统会直接报"已损坏"。以下任一方式即可正常使用：

**方式一：移除隔离属性（最可靠，推荐）**

```bash
# 安装后执行一次即可
xattr -cr /Applications/BIT.app
```

**方式二：系统设置放行**

1. 双击 dmg 安装，首次打开若弹出警告，**先不要点"移到废纸篓"**
2. 打开 系统设置 → 隐私与安全性 → 滚动到下方安全区 → 点击 **"仍要打开"**

**方式三：右键打开（macOS 14 及更早版本）**

按住 Control（或右键）点击 BIT → 选"打开" → 再点"打开"确认。

> 原理：`xattr -cr` 删除文件的 quarantine 隔离标记；签名本身完整可校验，去掉隔离后 macOS 不再拦截。

### Windows 首次运行提示 SmartScreen？

安装包未做代码签名（EV 证书同样需付费）。SmartScreen 弹窗时点 **"更多信息" → "仍要运行"** 即可。

### Linux 运行 AppImage

```bash
chmod +x BIT_0.5.9_amd64.AppImage
./BIT_0.5.9_amd64.AppImage
```

### 首次使用

进入「AI 设置」→ 添加一个提供方（协议 / Base URL / API Key / 模型，可点"从 API 获取"拉取模型列表）→ 点击播放按钮激活 → 回到「对话」开始使用。

终端场景：安装后在任意终端输入 `bit` 直接进入 TUI 对话。

## 远程访问与 API

「远程」页开启后，BIT 在本机监听 HTTP API（默认 `127.0.0.1:8600`，可改为 `0.0.0.0` 供局域网访问；默认关闭，开启时自动生成密钥与访问密码）。

**认证**

- **Client Key**（`bit_` 前缀，自动生成）：`Authorization: Bearer <key>` 或 `?key=<key>`——用于 `/v1/*` 与 `/mcp` 端点
- **访问密码**：`/api/*` 管理端点额外要求 `X-Access-Password` 头（OpenAI / MCP 客户端无法携带自定义头，故豁免）

**端点**

| 端点 | 方法 | 说明 |
|---|---|---|
| `/v1/chat/completions` | POST | OpenAI 兼容对话（支持 SSE 流式） |
| `/v1/models` | GET | 模型列表 |
| `/mcp` | POST / DELETE | 标准 MCP 服务器（Streamable HTTP / JSON-RPC 2.0） |
| `/api/*` | — | 会话 / 配置 / 审计等管理接口（需访问密码） |
| `/api/health` | GET | 健康检查（无需认证） |

```bash
curl http://127.0.0.1:8600/v1/chat/completions \
  -H "Authorization: Bearer $BIT_KEY" \
  -H "Content-Type: application/json" \
  -d '{"model":"YOUR_MODEL","messages":[{"role":"user","content":"列出当前目录"}]}'
```

详见 Wiki：[远程访问与 API](https://github.com/yxpil/bit/wiki/远程访问与-API) · [MCP-集成](https://github.com/yxpil/bit/wiki/MCP-集成)

## 文档

完整文档在 [Wiki](https://github.com/yxpil/bit/wiki)（中英双语，每页顶部可切换语言）：

- **使用**：[安装指南](https://github.com/yxpil/bit/wiki/安装指南) · [芯片与操作系统支持](https://github.com/yxpil/bit/wiki/芯片与操作系统支持) · [快速上手](https://github.com/yxpil/bit/wiki/快速上手) · [对话功能](https://github.com/yxpil/bit/wiki/对话功能) · [TUI-终端模式](https://github.com/yxpil/bit/wiki/TUI-终端模式) · [FAQ](https://github.com/yxpil/bit/wiki/FAQ)
- **进阶**：[工具系统](https://github.com/yxpil/bit/wiki/工具系统) · [MCP-集成](https://github.com/yxpil/bit/wiki/MCP-集成) · [记忆与技能](https://github.com/yxpil/bit/wiki/记忆与技能) · [远程访问与 API](https://github.com/yxpil/bit/wiki/远程访问与-API) · [自动更新](https://github.com/yxpil/bit/wiki/自动更新) · [审计日志](https://github.com/yxpil/bit/wiki/审计日志)
- **参与**：[开发与构建](https://github.com/yxpil/bit/wiki/开发与构建)

English wiki: [Home-EN](https://github.com/yxpil/bit/wiki/Home-EN) — every page has a language switcher at the top.

在线站点：[osbt.space](https://osbt.space) · [文档](https://osbt.space/docs.html)

## 安全与隐私

- **数据本地化**：会话、记忆、技能、配置全部保存在本机应用数据目录，不上传遥测。
- **双重认证**：Client Key（常数时间比较防时序侧信道）+ 访问密码；远程访问默认关闭。
- **沙箱与限额**：AI 自建脚本经 Rhai 沙箱受限执行（深度 / 操作数 / 墙钟预算）；子进程工具带超时杀灭、输出上限与资源回收。
- **MCP 会话治理**：会话 30 分钟空闲过期、数量上限、显式 DELETE 终止。
- **签名透明**：macOS ad-hoc 签名 / Windows 无 EV 证书（均为无付费证书的取舍，见上方安装说明），源码与 CI 构建流程全部公开可查。

## 开发

前置：[Node.js](https://nodejs.org/)、[Rust](https://www.rust-lang.org/) 工具链、Tauri 系统依赖。

```bash
npm install          # 安装前端依赖
npm run tauri dev    # 开发模式（热更新）
npm run tauri build  # 构建 release（NSIS / MSI / dmg / AppImage / deb）
```

## 项目结构

```
src/               React 前端
  pages/           对话 / AI 设置 / 工具 / 技能 / 记忆 / 审计 / 远程
  components/      Markdown、工具卡片、图标等
src-tauri/         Tauri (Rust) 后端
  src/             ai、agent、mcp、registry、runtime、script_runtime、
                   http_api、update、audit、session、goal、memory …
installer/bit.iss  Inno Setup 打包脚本
```

## 许可

[Apache License 2.0](LICENSE) — **BIT 永久免费**：所有功能无内购、无订阅、无功能锁，个人与商业使用均免费。
