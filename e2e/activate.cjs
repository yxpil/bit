// 激活 mock 提供方（记住原激活提供方，供 restore.cjs 恢复）
const fs = require("fs");
const p = process.env.APPDATA + "\\com.bit.hub\\";
const ai = JSON.parse(fs.readFileSync(p + "ai_config.json", "utf8"));
const active = ai.providers.find((x) => x.active);
fs.writeFileSync(p + "ai_config.json.orig-active", active ? active.id : "");
ai.providers.forEach((x) => (x.active = false));
const existing = ai.providers.find((x) => x.id === "e2e-mock-provider");
const mock = existing || (ai.providers.push({ id: "e2e-mock-provider", name: "E2E-Mock", protocol: "openai", base_url: "http://127.0.0.1:9901/v1", api_key: "sk-e2e-mock", model: "mock-1", active: false, temperature_mode: "default", reasoning_effort: "default" }), ai.providers[ai.providers.length - 1]);
mock.active = true;
fs.writeFileSync(p + "ai_config.json", JSON.stringify(ai));
console.log("mock provider activated (original:", (active && active.name) || "none", ")");
