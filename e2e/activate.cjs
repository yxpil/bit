// 激活 mock 提供方（记住原激活提供方，供 restore.cjs 恢复）
// 跨平台：Windows 用 %APPDATA%，macOS/Linux 用家目录固定路径（与 BIT 数据目录一致）
const fs = require("fs");
const os = require("os");
const path = require("path");
const dir = process.platform === "win32"
  ? path.join(process.env.APPDATA, "com.bit.hub")
  : path.join(os.homedir(), ".local", "share", "com.bit.hub");
const macDir = path.join(os.homedir(), "Library", "Application Support", "com.bit.hub");
const p = fs.existsSync(path.join(dir, "ai_config.json")) ? dir : macDir;
const ai = JSON.parse(fs.readFileSync(path.join(p, "ai_config.json"), "utf8"));
const active = ai.providers.find((x) => x.active);
fs.writeFileSync(path.join(p, "ai_config.json.orig-active"), active ? active.id : "");
ai.providers.forEach((x) => (x.active = false));
const existing = ai.providers.find((x) => x.id === "e2e-mock-provider");
const mock = existing || (ai.providers.push({ id: "e2e-mock-provider", name: "E2E-Mock", protocol: "openai", base_url: "http://127.0.0.1:9901/v1", api_key: "sk-e2e-mock", model: "mock-1", active: false, temperature_mode: "default", reasoning_effort: "default" }), ai.providers[ai.providers.length - 1]);
mock.active = true;
fs.writeFileSync(path.join(p, "ai_config.json"), JSON.stringify(ai));
console.log("mock provider activated (original:", (active && active.name) || "none", ")");
