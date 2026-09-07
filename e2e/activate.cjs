// 激活 mock 提供方（记住原激活提供方，供 restore.cjs 恢复）
// 目录判定：BIT_DATA_DIR 优先（E2E 隔离目录与被测实例一致），
// 否则 Windows 用 %APPDATA%，macOS/Linux 用家目录固定路径（与 BIT 数据目录一致）
const fs = require("fs");
const os = require("os");
const path = require("path");
const envDir = process.env.BIT_DATA_DIR;
const dir = envDir
  || (process.platform === "win32"
    ? path.join(process.env.APPDATA, "com.bit.hub")
    : path.join(os.homedir(), ".local", "share", "com.bit.hub"));
const macDir = path.join(os.homedir(), "Library", "Application Support", "com.bit.hub");
const p = envDir || (fs.existsSync(path.join(dir, "ai_config.json")) ? dir : macDir);
const cfgPath = path.join(p, "ai_config.json");
// E2E 隔离目录可能还没有 ai_config.json（首次运行）：引导最小配置
const ai = fs.existsSync(cfgPath) ? JSON.parse(fs.readFileSync(cfgPath, "utf8")) : { providers: [] };
if (!Array.isArray(ai.providers)) ai.providers = [];
const active = ai.providers.find((x) => x.active);
fs.writeFileSync(path.join(p, "ai_config.json.orig-active"), active ? active.id : "");
ai.providers.forEach((x) => (x.active = false));
const existing = ai.providers.find((x) => x.id === "e2e-mock-provider");
const mock = existing || (ai.providers.push({ id: "e2e-mock-provider", name: "E2E-Mock", protocol: "openai", base_url: "http://127.0.0.1:9901/v1", api_key: "sk-e2e-mock", model: "mock-1", active: false, temperature_mode: "default", reasoning_effort: "default" }), ai.providers[ai.providers.length - 1]);
mock.active = true;
// 文本协议降级开关（逐家提供方，默认关）：E2E 大量场景依赖「端点拒绝 tools → 降级文本协议」，
// mock 提供方显式打开；e2e-mock-strict 保持关闭，专测开关未开时的明确报错
mock.text_fallback = true;
// 多协议原生工具调用桥接测试用：同端口不同协议路径（openai 原生 tool_calls 用独立全新
// 提供方保证首次对话走原生探测；claude /v1/messages、gemini /v1beta/models/...）
for (const spec of [
  { id: "e2e-mock-openai-native", name: "E2E-Mock-OpenAI-Native", protocol: "openai", base_url: "http://127.0.0.1:9901/v1", model: "mock-1" },
  { id: "e2e-mock-claude", name: "E2E-Mock-Claude", protocol: "claude", base_url: "http://127.0.0.1:9901", model: "mock-1" },
  { id: "e2e-mock-gemini", name: "E2E-Mock-Gemini", protocol: "gemini", base_url: "http://127.0.0.1:9901", model: "mock-1" },
]) {
  const found = ai.providers.find((x) => x.id === spec.id);
  if (found) found.text_fallback = true;
  else
    ai.providers.push({
      api_key: "sk-e2e-mock", active: false, temperature_mode: "default", reasoning_effort: "default",
      text_fallback: true, ...spec,
    });
}
const strict = ai.providers.find((x) => x.id === "e2e-mock-strict");
if (!strict)
  ai.providers.push({ id: "e2e-mock-strict", name: "E2E-Mock-Strict", protocol: "openai", base_url: "http://127.0.0.1:9901/v1", api_key: "sk-e2e-mock", model: "mock-1", active: false, temperature_mode: "default", reasoning_effort: "default", text_fallback: false });
fs.writeFileSync(cfgPath, JSON.stringify(ai));
console.log("mock provider activated (dir:", p, "original:", (active && active.name) || "none", ")");
