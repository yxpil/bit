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

从 [Releases](https://github.com/yxpil/bit/releases) 下载最新的 **`BIT-Setup-x.y.z.exe`**（Inno Setup 安装包，已内置运行所需的 `WebView2Loader.dll`）并安装。

> 本项目为 GNU (MinGW) 工具链构建，运行时需要 `WebView2Loader.dll` 与 `bit.exe` 位于同一目录。请通过安装包安装，**不要单独拷贝 exe 运行**。

首次使用：进入「AI 设置」→ 添加一个提供方（协议 / Base URL / API Key / 模型）→ 点击播放按钮激活 → 回到「对话」开始使用。

## 开发

前置：[Node.js](https://nodejs.org/)、[Rust](https://www.rust-lang.org/) 工具链、Tauri 系统依赖。

```bash
npm install          # 安装前端依赖
npm run tauri dev    # 开发模式（热更新）
npm run tauri build  # 构建 release（生成 exe 与 NSIS/MSI 安装包）
```

### 打包 Inno Setup 安装包

`tauri build` 生成 release 二进制后，用 Inno Setup 打包（脚本已内置 WebView2 DLL）：

```powershell
& "C:\Program Files\Inno Setup 7\ISCC.exe" installer\bit.iss
# 产物：installer\Output\BIT-Setup-<version>.exe
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

暂未指定（保留所有权利）。
