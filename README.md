# BIT — Agent Tool Hub

BIT 是一个基于 **Tauri 2 + React 18** 的桌面应用：一个可审计、可远程访问的 AI Agent 工具中枢。你可以配置任意 AI 提供方，与模型进行流式对话，并让 AI 调用本机工具、自写脚本、沉淀记忆与技能。

> 无边框自定义标题栏 · 深/浅色主题 · 黑白线性设计

## 功能特性

- **流式对话**：前后端全链路流式（SSE + Tauri Event），模型回复逐字展示；助手消息支持 Markdown 渲染（加粗、列表、表格、代码块等）。
- **多提供方 AI 设置**：支持 OpenAI / Gemini / Claude 三种协议，可配置多家但**同一时刻仅激活一个**（互斥）。上游连通性可先测试、通过后再保存。
- **工具中枢**：注册、启停、调用工具；探测并注册本机解释器（JS / Python 等），AI 只需写一段能通讯的脚本即可成为工具。
- **AI 自建能力**：AI 可通过内置工具自写插件 / 直接执行脚本 / 把脚本沉淀为常驻工具。
- **记忆与技能**：AI 通过 `add_memory` / `skill` 工具自行总结沉淀，无需手动触发。
- **审计日志**：所有工具调用与关键操作留痕，可在「审计」页查看。
- **远程访问**：内置 HTTP API，支持客户端密钥与访问密码，密钥自动生成、无需手填。

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
chmod +x BIT_0.4.2_amd64.AppImage
./BIT_0.4.2_amd64.AppImage
```

### 首次使用

进入「AI 设置」→ 添加一个提供方（协议 / Base URL / API Key / 模型，可点"从 API 获取"拉取模型列表）→ 点击播放按钮激活 → 回到「对话」开始使用。

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
  src/             ai、agent、registry、runtime、http_api、audit …
installer/bit.iss  Inno Setup 打包脚本
```

## 许可

[Apache License 2.0](LICENSE)
