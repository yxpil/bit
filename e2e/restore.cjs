// 恢复原激活提供方，并移除 mock 提供方与 e2e 测试会话
// 跨平台：与 activate.cjs 相同的目录判定
const fs = require("fs");
const os = require("os");
const path = require("path");
const dir = process.platform === "win32"
  ? path.join(process.env.APPDATA, "com.bit.hub")
  : path.join(os.homedir(), ".local", "share", "com.bit.hub");
const macDir = path.join(os.homedir(), "Library", "Application Support", "com.bit.hub");
const p = fs.existsSync(path.join(dir, "ai_config.json")) ? dir : macDir;
const orig = fs.readFileSync(path.join(p, "ai_config.json.orig-active"), "utf8");
const ai = JSON.parse(fs.readFileSync(path.join(p, "ai_config.json"), "utf8"));
ai.providers = ai.providers.filter((x) => x.id !== "e2e-mock-provider");
ai.providers.forEach((x) => (x.active = x.id === orig));
fs.writeFileSync(path.join(p, "ai_config.json"), JSON.stringify(ai));
fs.unlinkSync(path.join(p, "ai_config.json.orig-active"));
const sj = JSON.parse(fs.readFileSync(path.join(p, "sessions.json"), "utf8"));
sj.sessions = sj.sessions.filter((x) => !x.id.startsWith("e2e-"));
// 同时清理 E2E 创建的测试工具，避免残留
try {
  const tj = JSON.parse(fs.readFileSync(path.join(p, "tools.json"), "utf8"));
  const kept = tj.filter((x) => x.name !== "e2e-doubler");
  if (kept.length !== tj.length) fs.writeFileSync(path.join(p, "tools.json"), JSON.stringify(kept));
} catch {}
fs.writeFileSync(path.join(p, "sessions.json"), JSON.stringify(sj));
console.log("restored provider:", ai.providers.find((x) => x.active)?.name || "none");
