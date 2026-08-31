# BIT v0.1.4

## 修复

### 🖥️ 启动时不再弹出 CMD 控制台窗口
- 原因：Windows 下 release 可执行文件被编译为控制台子系统程序，启动时会附带一个黑色命令行窗口
- 修复：为 release 构建声明 `windows_subsystem = "windows"`，启动后只显示主窗口
- debug 开发模式仍保留控制台，便于查看日志

## 安装

下载 `BIT-Setup-0.1.4.exe`（Inno Setup 安装包，含 WebView2Loader.dll），双击安装即可。

## 历史版本亮点

- v0.1.3：中英双语界面（顶栏 中/EN 一键切换）
- v0.1.2：附件上传（图片/Excel/Word/网页）、多会话并发、OpenAI 兼容接口、上下文压缩

## 完整变更

见 [commit](https://github.com/yxpil/bit/commits/main)
