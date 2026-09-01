// 恢复原激活提供方，并移除 mock 提供方与 e2e 测试会话
const fs = require("fs");
const p = process.env.APPDATA + "\\com.bit.hub\\";
const orig = fs.readFileSync(p + "ai_config.json.orig-active", "utf8");
const ai = JSON.parse(fs.readFileSync(p + "ai_config.json", "utf8"));
ai.providers = ai.providers.filter((x) => x.id !== "e2e-mock-provider");
ai.providers.forEach((x) => (x.active = x.id === orig));
fs.writeFileSync(p + "ai_config.json", JSON.stringify(ai));
fs.unlinkSync(p + "ai_config.json.orig-active");
const sj = JSON.parse(fs.readFileSync(p + "sessions.json", "utf8"));
sj.sessions = sj.sessions.filter((x) => !x.id.startsWith("e2e-"));
// 同时清理 E2E 创建的测试工具，避免残留
try {
  const tj = JSON.parse(fs.readFileSync(p + "tools.json", "utf8"));
  const kept = tj.filter((x) => x.name !== "e2e-doubler");
  if (kept.length !== tj.length) fs.writeFileSync(p + "tools.json", JSON.stringify(kept));
} catch {}
fs.writeFileSync(p + "sessions.json", JSON.stringify(sj));
console.log("restored provider:", ai.providers.find((x) => x.active)?.name || "none");
