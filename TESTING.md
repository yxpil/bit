# BIT 测试说明

BIT 是一个 Tauri 2 桌面应用，Rust crate 位于 `src-tauri/`，是**单一 bin crate**（`src/main.rs`，无 `lib.rs` 库目标）。因此它没有 `tests/` 集成测试目录——Rust 的 `tests/` 只能链接库目标，而为这个 Tauri bin 强行拆出 `lib.rs` 会把整套模块声明复制一份、显著拖慢编译并可能破坏 `tauri::App` 的初始化路径。**该 crate 的测试按 Rust 惯例全部放在各模块内的 `#[cfg(test)]` 单元测试中**，这对 bin crate 是标准做法。

## 如何运行

```powershell
cd src-tauri
cargo test --no-fail-fast
```

首次编译会拉取并编译数百个依赖（tauri 2、axum 0.8、reqwest、calamine、jieba-rs、rhai、scraper、qrcode …），耗时数分钟；之后增量编译约 30 秒。

只跑某个模块：

```powershell
cargo test --bin bit plugins::     # 只跑插件相关
cargo test --bin bit sandbox::     # 只跑沙箱路径相关
cargo test --bin bit securefile::  # 只跑加密存储相关
```

## 测了什么

- **协议解析**（`agent.rs`）：OpenAI/函数式/数组式工具调用的解析、残缺/未闭合 JSON 的修复与截断、字符串内花括号不误判。
- **模型响应适配**（`ai.rs`）：OpenAI/Claude/Gemini/Responses API 的多段文本、tool_call 载荷不泄露、瞬时网络错误分类、SSE 跨 chunk 切分。
- **HTTP API 鉴权**（`http_api.rs`）：bearer/query client key 接受与拒绝、未配置 client key 时拒绝全部、debug 端点双因子、health 旁路、worker IP 前缀语义。
- **MCP**（`mcp.rs`）：JSON/RPC/SSE body 解析、非 MCP server 拒绝、UTF-8 边界截断。
- **安全/加密**（`securefile.rs` / `security.rs`）：BITENC1 加密往返、随机盐不泄露明文、MAC 篡改与错误密钥检测、明文/密文透明兼容、bitsign/bitcrypt 往返与篡改、HMAC 已知向量、nonce 重放/过期。
- **沙箱路径**（`sandbox.rs`）：`normalize` 消解 `.`/`..`，`../` 逃逸根目录被拒，相互抵消的 `src/../src` 仍在根内。
- **插件/钩子**（`plugins.rs`）：`parse_schedule` 定时表达式、`sanitize` 模型友好名、`resolve_code` 文件优先于内联 code、缺失 id 的 `plugin.json` 仍可解析（sync 时用目录名覆盖）、非法 JSON 被收集为错误而非 panic。
- **其它**：guardian 事件日志/签名轮换、session 预览与归一、update 的受信 host 白名单与二进制回滚、repetition 循环检测、shell `&` 检测、toolenv shell 解析。

## 注入测试（路径穿越 / 不可信输入）

位于 `src/sandbox.rs` 的单元测试，验证 AI 提供的路径在沙箱规范化下不逃逸：

- `dotdot_climbing_above_root_is_rejected`：`../../../etc/passwd` 规范化后不以根为前缀 → 拒绝。
- `dotdot_that_cancels_out_stays_inside`：`src/../src/main.rs` 抵消后仍落在根内（不误伤合法路径）。
- `current_dir_segments_are_collapsed`：`././a/./b.txt` 折叠为 `a/b.txt`。
- `root_itself_is_allowed`：根目录本身合法。

外加 `http_api.rs` 的鉴权强制（未配置 client key 时拒绝所有 API 请求）与 `securefile.rs` 的密文不泄露明文密钥，共同构成不可信输入的防线。

## 钩子测试（插件机制）

位于 `src/plugins.rs` 的单元测试，覆盖插件代码解析与清单容错：

- `resolve_code_prefers_file_over_inline_code`：声明了 `file` 且文件存在时，用文件内容、忽略内联 code。
- `resolve_code_falls_back_to_inline_when_file_missing`：`file` 指向不存在文件时回退内联 code。
- `resolve_code_empty_inline_is_none`：无文件且内联为空白/缺失时返回 `None`（不注册空工具）。
- `plugin_manifest_without_id_still_parses`：按文档编写、省略 `id` 的 `plugin.json` 仍能解析（`kind` 默认 `interpreter`、`jobs` 默认空）。
- `plugin_manifest_rejects_garbage_json`：非法 JSON 被 `serde_json` 拒绝，供 `scan` 收集为错误列表（不 panic、不静默丢弃整个插件）。

> 说明：插件的实际 `scan`/`sync` 需要构造重型 `Arc<Ctx>`（含 workspace_root、config、各种运行时状态），属集成态，未在单元测试中拉起；此处覆盖其纯函数（代码解析、清单容错、定时/命名规范化）。

## 预期结果

`cargo test` 全绿：**199 passed / 0 failed**（原有 190 + 本次新增 9，其中注入/路径穿越 4、插件钩子 5）。
