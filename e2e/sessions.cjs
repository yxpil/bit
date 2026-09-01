// 向 sessions.json 写入 7 个独立 e2e 测试会话（应用停止状态下执行）
const fs = require("fs");
const p = process.env.APPDATA + "\\com.bit.hub\\";
const sj = JSON.parse(fs.readFileSync(p + "sessions.json", "utf8"));
sj.sessions = sj.sessions.filter((x) => !x.id.startsWith("e2e-"));
const now = "2026-09-01 12:30:00";
for (let i = 1; i <= 9; i++)
  sj.sessions.push({ id: "e2e-t" + i, title: "E2E-T" + i, created: now, updated: now, messages: [] });
fs.writeFileSync(p + "sessions.json", JSON.stringify(sj));
console.log("7 isolated sessions ready");
