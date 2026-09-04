# bit-agent

BIT 的 npm 分发通道。`BIT` 是本地优先的 AI Agent 工具集（MCP / 工具注册 / 技能）。

```bash
npm install -g bit-agent
bit-agent          # 启动 BIT（首次运行自动下载对应平台的应用）
```

`postinstall` 会自动识别平台并从 [GitHub Releases](https://github.com/yxpil/OpenBit/releases) 下载对应安装产物：

| 平台 | 产物 |
|---|---|
| macOS Apple Silicon / Intel | dmg（解包到 `~/.bit-agent/BIT.app`） |
| Linux x64 / ARM64 | AppImage |
| Linux RISC-V64 / LoongArch64（龙芯） | 裸二进制 tar.gz |
| Windows x64 | 免安装便携版 |

主项目：<https://github.com/yxpil/OpenBit>
