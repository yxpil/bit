// E2E 驱动脚本：通过 BIT 远程 API 驱动完整 AI 链路（mock 上游），逐场景断言
const http = require("http");

const BASE = "127.0.0.1", PORT = 8600;
const KEY = "bit_7eeaf8b450b734d9e4cea9b0e81c35e1";
const SID = "e2e-test";
// 禁用 keep-alive，排除连接复用导致的瞬态竞态
const agent = new http.Agent({ keepAlive: false });

function call(path, body) {
  return new Promise((resolve, reject) => {
    const data = JSON.stringify(body);
    const req = http.request(
      { host: BASE, port: PORT, path, method: "POST",
        headers: { "Content-Type": "application/json", Authorization: `Bearer ${KEY}`, "X-Access-Password": "56247930", "Content-Length": Buffer.byteLength(data) },
        timeout: 120000,
        agent },
      (res) => { let b = ""; res.on("data", (c) => (b += c)); res.on("end", () => resolve({ code: res.statusCode, body: b })); }
    );
    req.on("error", reject);
    req.on("timeout", () => { req.destroy(); reject(new Error("timeout")); });
    req.write(data);
    req.end();
  });
}

function getSSE(path, body) {
  return new Promise((resolve, reject) => {
    const data = JSON.stringify(body);
    const req = http.request(
      { host: BASE, port: PORT, path, method: "POST",
        headers: { "Content-Type": "application/json", Authorization: `Bearer ${KEY}`, "Content-Length": Buffer.byteLength(data) },
        timeout: 120000,
        agent },
      (res) => {
        let b = "";
        res.on("data", (c) => (b += c));
        res.on("end", () => resolve({ code: res.statusCode, sse: b }));
      }
    );
    req.on("error", reject);
    req.write(data);
    req.end();
  });
}

async function chat(sid, msg) {
  let r = await call("/api/chat", { session_id: sid, message: msg });
  if (r.code !== 200) {
    // 400 瞬态探测：立即原样重试一次，记录重试结果用于定位竞态
    console.log(`  [retry] first attempt code=${r.code} body=${r.body.slice(0, 60)}`);
    r = await call("/api/chat", { session_id: sid, message: msg });
  }
  if (r.code !== 200) throw new Error(`HTTP ${r.code}: ${r.body.slice(0, 200)}`);
  return JSON.parse(r.body);
}

const results = [];
function record(name, ok, detail) {
  results.push({ name, ok, detail });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}  ${ok ? "" : "| " + detail}`);
}

(async () => {
  // 等服务就绪
  for (let i = 0; i < 20; i++) {
    try { await new Promise((res, rej) => { const q = http.get({ host: BASE, port: PORT, path: "/api/health", timeout: 2000 }, (r) => { r.resume(); res(); }); q.on("error", rej); q.on("timeout", () => { q.destroy(); rej(new Error("t")); }); }); break; } catch { await new Promise((r) => setTimeout(r, 2000)); }
  }

  // T1 普通对话
  try {
    const r = await chat("e2e-t1", "E2E-PLAIN ping");
    record("T1 plain-chat", /E2E-FINAL-PLAIN/.test(r.reply || ""), `reply=${(r.reply || "").slice(0, 80)}`);
  } catch (e) { record("T1 plain-chat", false, e.message); }

  // T2 shell 工具调用（完整两轮：调用→反馈→最终答案）
  try {
    const r = await chat("e2e-t2", "E2E-CMD-SHELL run it");
    record("T2 tool-shell", /E2E-FINAL-OK.*e2e-shell-ok/.test(r.reply || ""), `reply=${(r.reply || "").slice(0, 120)}`);
  } catch (e) { record("T2 tool-shell", false, e.message); }

  // T3 自创标记 + 裸对象解析（v0.1.9 兼容性，经 /api/chat 即 chat_turn 路径）
  try {
    const r = await chat("e2e-t3", "E2E-CMD-MARKUP run");
    record("T3 tool-markup-bare-object", /E2E-FINAL-OK.*e2e-markup-ok/.test(r.reply || ""), `reply=${(r.reply || "").slice(0, 120)}`);
  } catch (e) { record("T3 tool-markup-bare-object", false, e.message); }

  // T4 单轮双工具
  try {
    const r = await chat("e2e-t4", "E2E-CMD-MULTI run both");
    record("T4 tool-multi", /E2E-FINAL-OK.*e2e-multi-a/.test(r.reply || ""), `reply=${(r.reply || "").slice(0, 120)}`);
  } catch (e) { record("T4 tool-multi", false, e.message); }

  // T5 write_file → edit 跨轮连续工具调用 + 文件内容验证
  const fs = require("fs");
  try { fs.unlinkSync(".e2e-tmp.txt"); } catch {}
  try {
    const r = await chat("e2e-t5", "E2E-CMD-FILES go");
    const content = fs.readFileSync(".e2e-tmp.txt", "utf8");
    record("T5 tool-files-roundtrip", /E2E-FINAL-FILES/.test(r.reply || "") && content === "alpha-beta", `reply=${(r.reply || "").slice(0, 60)} file=${content}`);
  } catch (e) { record("T5 tool-files-roundtrip", false, e.message); } finally { try { fs.unlinkSync(".e2e-tmp.txt"); } catch {} }

  // T6 plan 待办沉淀
  try {
    const r = await chat("e2e-t6", "E2E-CMD-PLAN todo");
    record("T6 tool-plan", /E2E-FINAL-OK/.test(r.reply || ""), `reply=${(r.reply || "").slice(0, 80)}`);
  } catch (e) { record("T6 tool-plan", false, e.message); }

  // T7 skill save → search 跨轮连续调用
  try {
    const r = await chat("e2e-t7", "E2E-CMD-SKILL go");
    record("T7 tool-skill-roundtrip", /E2E-FINAL-SKILL/.test(r.reply || ""), `reply=${(r.reply || "").slice(0, 80)}`);
  } catch (e) { record("T7 tool-skill-roundtrip", false, e.message); }

  // T8 OpenAI 兼容流式（stream=true 返回 SSE 增量）
  try {
    const r = await getSSE("/v1/chat/completions", { messages: [{ role: "user", content: "E2E-PLAIN stream please" }], stream: true });
    const okSSE = r.code === 200 && r.sse.includes("data:") && /E2E-FINAL-PLAIN/.test(r.sse);
    record("T8 openai-sse-stream", okSSE, `code=${r.code} body=${r.sse.slice(0, 100)}`);
  } catch (e) { record("T8 openai-sse-stream", false, e.message); }

  // T9 AI 自建工具全流程：add_tool 注册 node 脚本 → 立即调用新工具 → 结果回传
  try {
    const r = await chat("e2e-t9", "E2E-CMD-ADDTOOL go");
    record("T9 add-tool-roundtrip", /E2E-FINAL-ADDTOOL doubled=42/.test(r.reply || ""), `reply=${(r.reply || "").slice(0, 100)}`);
  } catch (e) { record("T9 add-tool-roundtrip", false, e.message); }

  // T10 新工具已进入工具清单（后续对话 AI 可见可用），并直接调用验证执行正确性
  try {
    const q = await new Promise((resolve, reject) => {
      const req = http.request(
        { host: BASE, port: PORT, path: "/api/tools", method: "GET",
          headers: { Authorization: `Bearer ${KEY}`, "X-Access-Password": "56247930" }, timeout: 15000, agent },
        (res) => { let b = ""; res.on("data", (c) => (b += c)); res.on("end", () => resolve(JSON.parse(b))); }
      );
      req.on("error", reject);
      req.end();
    });
    const t = (q.tools || []).find((x) => x.name === "e2e-doubler");
    if (!t) {
      record("T10 new-tool-in-manifest", false, "e2e-doubler 未出现在工具清单");
    } else {
      const inv = JSON.parse(JSON.stringify({ params: { a: 100 } }));
      const r = await new Promise((resolve, reject) => {
        const data = JSON.stringify(inv);
        const req = http.request(
          { host: BASE, port: PORT, path: `/api/tools/${t.id}/invoke`, method: "POST",
            headers: { "Content-Type": "application/json", Authorization: `Bearer ${KEY}`, "X-Access-Password": "56247930", "Content-Length": Buffer.byteLength(data) },
            timeout: 60000, agent },
          (res) => { let b = ""; res.on("data", (c) => (b += c)); res.on("end", () => resolve({ code: res.statusCode, body: b })); }
        );
        req.on("error", reject);
        req.write(data);
        req.end();
      });
      const ok = r.code === 200 && r.body.includes('"doubled":200');
      record("T10 new-tool-in-manifest", ok, `code=${r.code} body=${r.body.slice(0, 100)}`);
    }
  } catch (e) { record("T10 new-tool-in-manifest", false, e.message); }

  const pass = results.filter((x) => x.ok).length;
  console.log(`\n==== ${pass}/${results.length} passed ====`);
  process.exit(pass === results.length ? 0 : 1);
})();
