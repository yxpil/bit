// 长文本 + 压力 E2E：走 BIT /api/chat 全链路（mock 上游），覆盖流中断与 max_tokens 截断
// 用法：mock-ai(9901) 与 BIT（隔离数据目录）就绪后：node e2e/stress.cjs
// 凭据：E2E_PORT / E2E_KEY / E2E_PASSWORD（与 run.cjs 一致）
const http = require("http");

const BASE = "127.0.0.1";
const PORT = Number(process.env.E2E_PORT) || 8600;
const KEY = process.env.E2E_KEY || "";
const PASSWORD = process.env.E2E_PASSWORD || "";
const agent = new http.Agent({ keepAlive: false });

function call(path, body, timeout = 300000) {
  return new Promise((resolve, reject) => {
    const data = JSON.stringify(body);
    const req = http.request(
      { host: BASE, port: PORT, path, method: "POST",
        headers: { "Content-Type": "application/json", Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD, "Content-Length": Buffer.byteLength(data) },
        timeout, agent },
      (res) => { let b = ""; res.on("data", (c) => (b += c)); res.on("end", () => resolve({ code: res.statusCode, body: b })); }
    );
    req.on("timeout", () => { req.destroy(); reject(new Error("timeout")); });
    req.on("error", reject);
    req.end(data);
  });
}

async function chat(session, message) {
  const r = await call("/api/chat", { session_id: session, message });
  let json = {};
  try { json = JSON.parse(r.body); } catch {}
  return { code: r.code, ...json };
}

// 流式调用 BIT 的 OpenAI 兼容端点（触发上游流式链路），返回 SSE 原文
function chatStreamSSE(session, message) {
  return new Promise((resolve, reject) => {
    const data = JSON.stringify({ model: "mock", stream: true, messages: [{ role: "user", content: message }] });
    const req = http.request(
      { host: BASE, port: PORT, path: "/v1/chat/completions", method: "POST",
        headers: { "Content-Type": "application/json", Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD, "Content-Length": Buffer.byteLength(data) },
        timeout: 120000, agent },
      (res) => { let b = ""; res.on("data", (c) => (b += c)); res.on("end", () => resolve(b)); }
    );
    req.on("timeout", () => { req.destroy(); reject(new Error("timeout")); });
    req.on("error", reject);
    req.end(data);
  });
}

const results = [];
const record = (name, ok, detail = "") => {
  results.push({ name, ok, detail });
  console.log(`${ok ? "✓" : "✗"} ${name}${detail ? "  " + detail : ""}`);
};

async function main() {

// ── L1 上行长文本：100KB 消息完整转发到上游 ──
{
  const body = ("中文字符ABC123".repeat(11106) + "。").slice(0, 99955);
  const msg = `E2E-CMD-ECHO\nHEAD-MARKER-7777${body}TAIL-MARKER-9999`;
  const r = await chat("stress-up", msg);
  const stats = String(r.reply || "").match(/E2E-ECHO-STATS (\{.*\})/);
  let ok = false, detail = `code=${r.code}`;
  if (stats) {
    const s = JSON.parse(stats[1]);
    ok = s.len === msg.length && s.head.startsWith("E2E-CMD-ECHO") && s.tail.endsWith("TAIL-MARKER-9999");
    detail = `sent=${msg.length} recv=${s.len} head=${JSON.stringify(s.head.slice(0, 20))} tail=${JSON.stringify(s.tail.slice(-20))}`;
  }
  record("L1 上行100KB完整转发", ok, detail);
}

// ── L2 下行长文本：50KB 分块流式回复完整接收 ──
{
  const r = await chat("stress-long", "E2E-CMD-LONG");
  const rep = r.reply || "";
  const lines = rep.split("\n").filter(Boolean);
  record(
    "L2 下行50KB完整接收",
    r.code === 200 && rep.length >= 50040 && rep.length <= 50050 && lines[0].startsWith("L0000:") && lines[49].startsWith("L0049:"),
    `code=${r.code} len=${rep.length}`
  );
}

// ── F1 流中断：上游不发 [DONE] 直接断开 → BIT 自动回退非流式重试，保证完整答复+干净收尾 ──
{
  const sse = await chatStreamSSE("stress-drop-sse", "E2E-CMD-DROP");
  record(
    "F1 流中断回退恢复",
    sse.includes("[DONE]") && sse.includes("E2E-DROP-需要流式请求"),
    `len=${sse.length} 回退=${sse.includes("E2E-DROP-需要流式请求") ? "已触发" : "未触发"}`
  );
}

// ── F2 max_tokens 截断：finish_reason=length → 正文尾部显式标注 ──
{
  const r = await chat("stress-length", "E2E-CMD-LENGTH");
  const rep = r.reply || "";
  record(
    "F2 截断显式标注",
    r.code === 200 && rep.includes("回答的前半部分") && rep.includes("达到最大输出长度被截断"),
    `code=${r.code} tail=${rep.slice(-30)}`
  );
}

// ── S1 顺序压测：20 连发（每发独立会话） ──
{
  const t0 = Date.now();
  let okN = 0;
  for (let i = 0; i < 20; i++) {
    const r = await chat(`stress-seq-${i}`, `普通对话第${i}轮`);
    if (r.code === 200 && (r.reply || "").length > 0) okN++;
  }
  const ms = Date.now() - t0;
  record("S1 顺序20连发", okN === 20, `${okN}/20 ${ms}ms (avg ${Math.round(ms / 20)}ms)`);
}

// ── S2 并发压测：8 路同时发 ──
{
  const t0 = Date.now();
  const rs = await Promise.all(
    Array.from({ length: 8 }, (_, i) => chat(`stress-par-${i}`, `并发对话第${i}轮`))
  );
  const okN = rs.filter((r) => r.code === 200 && (r.reply || "").length > 0).length;
  record("S2 并发8路", okN === 8, `${okN}/8 ${Date.now() - t0}ms`);
}

// ── S3 长会话：单会话 40 轮 ──
{
  const sid = "stress-marathon";
  let okN = 0, msgs = 0;
  const t0 = Date.now();
  for (let i = 0; i < 40; i++) {
    const r = await chat(sid, `马拉松对话第${i}轮`);
    if (r.code === 200) { okN++; msgs = (r.messages || []).length; }
  }
  record("S3 单会话40轮", okN === 40 && msgs >= 40 && msgs <= 80, `${okN}/40 历史${msgs}条（含自动沉淀压缩）${Date.now() - t0}ms`);

  const pass = results.filter((r) => r.ok).length;
  console.log(`\n${pass}/${results.length} 通过`);
  process.exit(pass === results.length ? 0 : 1);
}
}

main();
