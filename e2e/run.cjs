// E2E 驱动脚本：通过 BIT 远程 API 驱动完整 AI 链路（mock 上游），逐场景断言
// 默认从 BIT 数据目录 config.json 读取 client_key / access_password，可用 E2E_PORT / E2E_KEY / E2E_PASSWORD 覆盖
const http = require("http");
const fs = require("fs");
const os = require("os");
const { signHeaders, deviceMaterial, workerBind, proofOf } = require("./sign.cjs");

const BASE = "127.0.0.1";
const PORT = Number(process.env.E2E_PORT) || 8600;
let cfg = {};
try {
  cfg = JSON.parse(fs.readFileSync(os.homedir() + "/Library/Application Support/com.bit.hub/config.json", "utf8"));
} catch {}
const KEY = process.env.E2E_KEY || cfg.client_key || "";
const PASSWORD = process.env.E2E_PASSWORD || cfg.access_password || "";
// 禁用 keep-alive，排除连接复用导致的瞬态竞态
const agent = new http.Agent({ keepAlive: false });

// 设备签名材料（bitsign-v2）：读实例 config.json 的 device_key（启动自动注册），
// 按 device.rs::sig_material 口径派生。启动注册含公网 IP 竞速（最长 ~5s），这里轮询等待
let MAT_CACHE = null;
async function devMat() {
  if (MAT_CACHE) return MAT_CACHE;
  const st = JSON.parse((await callGet("/api/debug/state")).body || "{}");
  const dd = String(st.data_dir || "");
  const deadline = Date.now() + 20_000;
  while (Date.now() < deadline) {
    try {
      const c = JSON.parse(fs.readFileSync(dd + "/config.json", "utf8"));
      if (typeof c.device_key === "string" && c.device_key.startsWith("bitdev_")) {
        MAT_CACHE = deviceMaterial(c.device_key);
        return MAT_CACHE;
      }
    } catch {}
    await new Promise((r) => setTimeout(r, 500));
  }
  throw new Error("device_key not registered within 20s");
}

function call(path, body) {
  return new Promise((resolve, reject) => {
    const data = JSON.stringify(body);
    const req = http.request(
      { host: BASE, port: PORT, path, method: "POST",
        headers: { "Content-Type": "application/json", Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD, "Content-Length": Buffer.byteLength(data) },
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

function getJson(path) {
  return new Promise((resolve, reject) => {
    const req = http.request(
      { host: BASE, port: PORT, path, method: "GET",
        headers: { Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD }, timeout: 15000, agent },
      (res) => { let b = ""; res.on("data", (c) => (b += c)); res.on("end", () => resolve(JSON.parse(b))); }
    );
    req.on("error", reject);
    req.end();
  });
}

// GET 原始响应（不解析 JSON）：断言需要自行处理状态码 / 非 JSON 体的场景
function callGet(path) {
  return new Promise((resolve, reject) => {
    const req = http.request(
      { host: BASE, port: PORT, path, method: "GET",
        headers: { Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD }, timeout: 15000, agent },
      (res) => { let b = ""; res.on("data", (c) => (b += c)); res.on("end", () => resolve({ code: res.statusCode, body: b })); }
    );
    req.on("error", reject);
    req.end();
  });
}

// 自定义凭据的 GET：用于鉴权负例（期望 401/403，响应体不必是 JSON）
function getStatus(path, headers) {
  return new Promise((resolve, reject) => {
    const req = http.request(
      { host: BASE, port: PORT, path, method: "GET", headers, timeout: 15000, agent },
      (res) => { let b = ""; res.on("data", (c) => (b += c)); res.on("end", () => resolve({ code: res.statusCode, body: b })); }
    );
    req.on("error", reject);
    req.end();
  });
}

const results = [];
// 保护真实 ai_config.json：备份 → 进程退出时恢复（含失败路径），杜绝 E2E 污染日常使用的提供方配置。
// 双保险：备份同时落盘（/tmp），进程被 kill -9 等异常退出时也能手动找回
const AI_CFG = os.homedir() + "/Library/Application Support/com.bit.hub/ai_config.json";
const AI_CFG_DISK_BACKUP = "/tmp/bit-ai-config-backup.json";
const AI_CFG_BACKUP = fs.existsSync(AI_CFG) ? fs.readFileSync(AI_CFG) : null;
if (AI_CFG_BACKUP !== null) {
  try { fs.writeFileSync(AI_CFG_DISK_BACKUP, AI_CFG_BACKUP); } catch {}
}
process.on("exit", () => {
  if (AI_CFG_BACKUP === null) return;
  try { fs.writeFileSync(AI_CFG, AI_CFG_BACKUP); console.log("(ai_config.json restored)"); } catch {}
});
function record(name, ok, detail) {
  results.push({ name, ok, detail });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}  ${ok ? "" : "| " + detail}`);
}

(async () => {
  // 每次运行使用全新会话 id，避免历史残留干扰断言
  const RUN = Date.now().toString(36);
  const sid = (n) => `e2e-${RUN}-t${n}`;
  // 等服务就绪
  for (let i = 0; i < 20; i++) {
    try { await new Promise((res, rej) => { const q = http.get({ host: BASE, port: PORT, path: "/api/health", timeout: 2000 }, (r) => { r.resume(); res(); }); q.on("error", rej); q.on("timeout", () => { q.destroy(); rej(new Error("t")); }); }); break; } catch { await new Promise((r) => setTimeout(r, 2000)); }
  }

  // 隔离预检：拒绝对真实数据目录跑 E2E（曾污染 goals/todos/ai_config）
  try {
    const st = await new Promise((resolve, reject) => {
      const q = http.get({ host: BASE, port: PORT, path: "/api/debug/state",
        headers: { Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD }, timeout: 5000 },
        (r) => { let b = ""; r.on("data", (c) => (b += c)); r.on("end", () => { try { resolve(JSON.parse(b)); } catch { reject(new Error("bad json")); } }); });
      q.on("error", reject); q.on("timeout", () => { q.destroy(); reject(new Error("timeout")); });
    });
    const dd = String(st.data_dir || "");
    if (process.env.E2E_ALLOW_REAL_DATA !== "1" && !/e2e/i.test(dd)) {
      console.error(`\n[拒绝执行] 被测实例数据目录不是 E2E 隔离目录: ${dd || "(未知)"}`);
      console.error("请用隔离目录启动实例，例如: BIT_DATA_DIR=/tmp/bit-e2e ... ；确需对真实目录跑请设 E2E_ALLOW_REAL_DATA=1");
      process.exit(1);
    }
    console.log(`(data_dir: ${dd})`);
  } catch (e) {
    console.error(`\n[拒绝执行] 无法确认被测实例数据目录（${e.message}）——可能实例过旧，请升级后重试`);
    process.exit(1);
  }

  // E2E 配置基线：对话限速关闭（用例节奏不可控，T36 自行验证限速）、审批模式归位 allow_all（T37-39 自行切换验证）
  try { await call("/api/debug/config", { chat_rpm_max: 0, chat_rate_reset: true, tool_approval: "allow_all" }); } catch {}

  // 协议族基线：T1-T58 按「兼容模式 = 文本约定」编排——mock 通用工具标记以正文单行 JSON
  // 输出调用（标准协议自 0.5.36 起不再兜底解析正文 JSON，那是兼容模式的专属职责）。
  // 原生 tools 专项集中在 T59-64，各块开头已显式切换 compat_mode
  try { await call("/api/debug/config", { compat_mode: true }); } catch {}

  // 上游基线：active_provider 是持久配置——被其它矩阵/失败用例切到 Claude/Gemini 后必须归位
  // 到默认 OpenAI mock，否则 T1-T58 的 mock 通用工具关键词全落在别的协议 fallback（只见「好的。」）
  try { await call("/api/debug/config", { active_provider: "e2e-mock-provider" }); } catch {}

  // 中继测试态清零：防上一轮崩溃遗留的封禁/盒子状态跨轮泄漏（T53 自身的 _reset 只覆盖它前后）
  {
    const rport = Number(process.env.FAKE_RELAY_PORT) || 9802;
    await new Promise((resolve) => {
      const q = http.get({ host: BASE, port: rport, path: "/relay/_reset", timeout: 5000 },
        (r) => { r.resume(); r.on("end", resolve); });
      q.on("error", () => resolve()); q.on("timeout", () => { q.destroy(); resolve(); });
    });
  }

  // T1 普通对话
  try {
    const r = await chat(sid(1), "E2E-PLAIN ping");
    record("T1 plain-chat", /E2E-FINAL-PLAIN/.test(r.reply || ""), `reply=${(r.reply || "").slice(0, 80)}`);
  } catch (e) { record("T1 plain-chat", false, e.message); }

  // T2 shell 工具调用（完整两轮：调用→反馈→最终答案）
  try {
    const r = await chat(sid(2), "E2E-CMD-SHELL run it");
    record("T2 tool-shell", /E2E-FINAL-OK.*e2e-shell-ok/.test(r.reply || ""), `reply=${(r.reply || "").slice(0, 120)}`);
  } catch (e) { record("T2 tool-shell", false, e.message); }

  // T3 自创标记 + 裸对象解析（v0.1.9 兼容性，经 /api/chat 即 chat_turn 路径）
  try {
    const r = await chat(sid(3), "E2E-CMD-MARKUP run");
    record("T3 tool-markup-bare-object", /E2E-FINAL-OK.*e2e-markup-ok/.test(r.reply || ""), `reply=${(r.reply || "").slice(0, 120)}`);
  } catch (e) { record("T3 tool-markup-bare-object", false, e.message); }

  // T4 单轮双工具
  try {
    const r = await chat(sid(4), "E2E-CMD-MULTI run both");
    record("T4 tool-multi", /E2E-FINAL-OK.*e2e-multi-a/.test(r.reply || ""), `reply=${(r.reply || "").slice(0, 120)}`);
  } catch (e) { record("T4 tool-multi", false, e.message); }

  // T5 write_file → edit 跨轮连续工具调用 + 文件内容验证
  // 应用工作目录可能是仓库根或 src-tauri（tauri dev），两处都找
  const fs = require("fs");
  const tmpPaths = [".e2e-tmp.txt", "src-tauri/.e2e-tmp.txt"];
  const readTmp = () => { for (const p of tmpPaths) { try { return fs.readFileSync(p, "utf8"); } catch {} } return null; };
  const delTmp = () => tmpPaths.forEach((p) => { try { fs.unlinkSync(p); } catch {} });
  try {
    delTmp();
    const r = await chat(sid(5), "E2E-CMD-FILES go");
    const content = readTmp();
    record("T5 tool-files-roundtrip", /E2E-FINAL-FILES/.test(r.reply || "") && content === "alpha-beta", `reply=${(r.reply || "").slice(0, 60)} file=${content}`);
  } catch (e) { record("T5 tool-files-roundtrip", false, e.message); } finally { delTmp(); }

  // T6 plan 待办沉淀：plan 建目标后由 auto-drive 自动收尾。
  // BIT 现在会把目标推进到全部待办完成并自动归档，最终确认语可能是 plan 的 E2E-FINAL-OK，
  // 也可能是自动收尾统一的 E2E-AUTODRIVE-DONE（两种都代表「计划落地并被自动完成/归档」）
  try {
    const r = await chat(sid(6), "E2E-CMD-PLAN todo");
    const ok6 = /E2E-FINAL-OK/.test(r.reply || "") || /E2E-AUTODRIVE-DONE/.test(r.reply || "");
    record("T6 tool-plan", ok6, `reply=${(r.reply || "").slice(0, 80)}`);
  } catch (e) { record("T6 tool-plan", false, e.message); }

  // T7 skill save → search 跨轮连续调用
  try {
    const r = await chat(sid(7), "E2E-CMD-SKILL go");
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
    const r = await chat(sid(9), "E2E-CMD-ADDTOOL go");
    record("T9 add-tool-roundtrip", /E2E-FINAL-ADDTOOL doubled=42/.test(r.reply || ""), `reply=${(r.reply || "").slice(0, 100)}`);
  } catch (e) { record("T9 add-tool-roundtrip", false, e.message); }

  // T10 新工具已进入工具清单（后续对话 AI 可见可用），并直接调用验证执行正确性
  try {
    const q = await new Promise((resolve, reject) => {
      const req = http.request(
        { host: BASE, port: PORT, path: "/api/tools", method: "GET",
          headers: { Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD }, timeout: 15000, agent },
        (res) => { let b = ""; res.on("data", (c) => (b += c)); res.on("end", () => resolve(JSON.parse(b))); }
      );
      req.on("error", reject);
      req.end();
    });
    const t = (q.tools || []).find((x) => x.name === "e2e-doubler");
    if (!t) {
      record("T10 new-tool-in-manifest", false, "e2e-doubler 未出现在工具清单");
    } else {
      // 远程 invoke 现已受审批模式门禁（T37-T39），E2E 默认归位 allow_all 保证既有用例语义
      await call("/api/debug/config", { tool_approval: "allow_all" });
      const inv = JSON.parse(JSON.stringify({ params: { a: 100 } }));
      const r = await new Promise((resolve, reject) => {
        const data = JSON.stringify(inv);
        const req = http.request(
          { host: BASE, port: PORT, path: `/api/tools/${t.id}/invoke`, method: "POST",
            headers: { "Content-Type": "application/json", Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD, "Content-Length": Buffer.byteLength(data) },
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

  // T11 MCP 服务端全流程（标准 Streamable HTTP 客户端行为）：
  // initialize 从响应头取 Mcp-Session-Id → tools/list / tools/call 均携带会话
  let mcpSid = "";
  try {
    const rpc = (method, params, sid) => new Promise((resolve, reject) => {
      const data = JSON.stringify({ jsonrpc: "2.0", id: 1, method, ...(params ? { params } : {}) });
      const headers = { "Content-Type": "application/json", Authorization: `Bearer ${KEY}`, "Content-Length": Buffer.byteLength(data) };
      if (sid) headers["Mcp-Session-Id"] = sid;
      const req = http.request(
        { host: BASE, port: PORT, path: "/mcp", method: "POST",
          headers, timeout: 60000, agent },
        (res) => { let b = ""; res.on("data", (c) => (b += c)); res.on("end", () => resolve({ code: res.statusCode, body: b, sid: res.headers["mcp-session-id"] || "" })); }
      );
      req.on("error", reject);
      req.write(data);
      req.end();
    });
    const init = await rpc("initialize", { protocolVersion: "2025-03-26", capabilities: {}, clientInfo: { name: "e2e", version: "0" } });
    mcpSid = init.sid;
    const list = await rpc("tools/list", null, mcpSid);
    const hasShell = (JSON.parse(list.body).result?.tools || []).some((t) => t.name === "shell");
    const call = await rpc("tools/call", { name: "shell", arguments: { command: "echo e2e-mcp-ok" } }, mcpSid);
    const okMcp =
      init.code === 200 && mcpSid.length > 0 &&
      list.code === 200 && hasShell &&
      call.code === 200 && call.body.includes("e2e-mcp-ok") && call.body.includes('"isError":false');
    record("T11 mcp-server-roundtrip", okMcp, `init=${init.code} sid=${mcpSid ? "yes" : "no"} list=${list.code} hasShell=${hasShell} call=${call.code} ${call.body.slice(0, 80)}`);
  } catch (e) { record("T11 mcp-server-roundtrip", false, e.message); }

  // T12 MCP 错误分支（带合法会话）：未知 method → JSON-RPC -32601；未知工具 → -32602
  try {
    if (!mcpSid) {
      const data = JSON.stringify({ jsonrpc: "2.0", id: 0, method: "initialize", params: { protocolVersion: "2025-03-26", capabilities: {}, clientInfo: { name: "e2e", version: "0" } } });
      mcpSid = await new Promise((resolve, reject) => {
        const req = http.request(
          { host: BASE, port: PORT, path: "/mcp", method: "POST",
            headers: { "Content-Type": "application/json", Authorization: `Bearer ${KEY}`, "Content-Length": Buffer.byteLength(data) },
            timeout: 15000, agent },
          (res) => { res.resume(); res.on("end", () => resolve(res.headers["mcp-session-id"] || "")); }
        );
        req.on("error", reject);
        req.write(data);
        req.end();
      });
    }
    const rpc = (body) => new Promise((resolve, reject) => {
      const data = JSON.stringify(body);
      const req = http.request(
        { host: BASE, port: PORT, path: "/mcp", method: "POST",
          headers: { "Content-Type": "application/json", Authorization: `Bearer ${KEY}`, "Content-Length": Buffer.byteLength(data), "Mcp-Session-Id": mcpSid },
          timeout: 15000, agent },
        (res) => { let b = ""; res.on("data", (c) => (b += c)); res.on("end", () => resolve({ code: res.statusCode, body: b })); }
      );
      req.on("error", reject);
      req.write(data);
      req.end();
    });
    const unknown = await rpc({ jsonrpc: "2.0", id: 2, method: "resources/list" });
    const noTool = await rpc({ jsonrpc: "2.0", id: 3, method: "tools/call", params: { name: "no-such-tool", arguments: {} } });
    const okErr =
      unknown.code === 200 && unknown.body.includes("-32601") &&
      noTool.code === 200 && noTool.body.includes("-32602");
    record("T12 mcp-error-branches", okErr, `unknown=${unknown.code}:${unknown.body.slice(0, 60)} noTool=${noTool.code}:${noTool.body.slice(0, 60)}`);
  } catch (e) { record("T12 mcp-error-branches", false, e.message); }

  // T21 MCP 协议符合性（标准 MCP 客户端视角）：会话缺失 400 / 未知 404 / 版本协商回显 /
  // ping 空 result / 通知 202 / JSON-RPC batch / DELETE 终止后 404 / GET 405
  try {
    const post = (bodyObj, sid) => new Promise((resolve, reject) => {
      const data = JSON.stringify(bodyObj);
      const headers = { "Content-Type": "application/json", Authorization: `Bearer ${KEY}`, "Content-Length": Buffer.byteLength(data) };
      if (sid) headers["Mcp-Session-Id"] = sid;
      const req = http.request(
        { host: BASE, port: PORT, path: "/mcp", method: "POST", headers, timeout: 15000, agent },
        (res) => { let b = ""; res.on("data", (c) => (b += c)); res.on("end", () => resolve({ code: res.statusCode, body: b, sid: res.headers["mcp-session-id"] || "" })); }
      );
      req.on("error", reject);
      req.write(data);
      req.end();
    });
    const noSid = await post({ jsonrpc: "2.0", id: 1, method: "tools/list" });
    const badSid = await post({ jsonrpc: "2.0", id: 2, method: "tools/list" }, "mcp-does-not-exist");
    const init2 = await post({ jsonrpc: "2.0", id: 3, method: "initialize", params: { protocolVersion: "2024-11-05", capabilities: {}, clientInfo: { name: "e2e", version: "0" } } });
    const sid2 = init2.sid;
    const negOk = init2.code === 200 && sid2.length > 0 && JSON.parse(init2.body).result?.protocolVersion === "2024-11-05";
    const ping = await post({ jsonrpc: "2.0", id: 4, method: "ping" }, sid2);
    const pingOk = ping.code === 200 && JSON.stringify(JSON.parse(ping.body).result) === "{}";
    const notif = await post({ jsonrpc: "2.0", method: "notifications/initialized" });
    const batch = await post([
      { jsonrpc: "2.0", id: 5, method: "ping" },
      { jsonrpc: "2.0", method: "notifications/initialized" },
    ], sid2);
    let batchOk = batch.code === 200;
    if (batchOk) {
      const arr = JSON.parse(batch.body);
      batchOk = Array.isArray(arr) && arr.length === 1 && JSON.stringify(arr[0].result) === "{}";
    }
    const del = await new Promise((resolve, reject) => {
      const req = http.request(
        { host: BASE, port: PORT, path: "/mcp", method: "DELETE",
          headers: { Authorization: `Bearer ${KEY}`, "Mcp-Session-Id": sid2 }, timeout: 15000, agent },
        (res) => { res.resume(); res.on("end", () => resolve(res.statusCode)); }
      );
      req.on("error", reject);
      req.end();
    });
    const afterDel = await post({ jsonrpc: "2.0", id: 6, method: "tools/list" }, sid2);
    const getMcp = await getStatus("/mcp", { Authorization: `Bearer ${KEY}` });
    const ok21 =
      noSid.code === 400 && badSid.code === 404 && negOk &&
      pingOk && notif.code === 202 && batchOk && del === 200 &&
      afterDel.code === 404 && getMcp.code === 405;
    record("T21 mcp-protocol-conformance", ok21,
      `noSid=${noSid.code} badSid=${badSid.code} neg=${negOk} ping=${ping.code} notif=${notif.code} batch=${batchOk} del=${del} after=${afterDel.code} get=${getMcp.code}`);
  } catch (e) { record("T21 mcp-protocol-conformance", false, e.message); }

  // T13 视觉：/api/chat 携带图片（data URL），mock 上游确认看到图片
  try {
    const r = await call("/api/chat", {
      session_id: sid(13),
      message: "E2E-CMD-IMG 描述这张图片",
      images: ["data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=="],
    });
    const reply = r.code === 200 ? (JSON.parse(r.body).reply || "") : r.body;
    record("T13 image-via-remote-chat", /E2E-IMAGE-SEEN count=1/.test(reply), `code=${r.code} reply=${reply.slice(0, 80)}`);
  } catch (e) { record("T13 image-via-remote-chat", false, e.message); }

  // T14 视觉：OpenAI 兼容端点 /v1/chat/completions 多模态 content 数组
  try {
    const r = await call("/v1/chat/completions", {
      model: "mock-model-a",
      messages: [{
        role: "user",
        content: [
          { type: "text", text: "E2E-CMD-IMG-V1 这是什么" },
          { type: "image_url", image_url: { url: "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==" } },
        ],
      }],
    });
    const reply = r.code === 200 ? (JSON.parse(r.body).choices?.[0]?.message?.content || "") : r.body;
    record("T14 image-via-openai-endpoint", /E2E-IMAGE-SEEN count=1/.test(reply), `code=${r.code} reply=${reply.slice(0, 80)}`);
  } catch (e) { record("T14 image-via-openai-endpoint", false, e.message); }

  // T15 工具覆盖：AI 同名覆盖更新自建工具（翻倍 → 三倍），立即调用验证新代码生效
  try {
    const r = await chat(sid(15), "E2E-CMD-RETOOL start");
    record("T15 tool-overwrite", /E2E-FINAL-RETOOL tripled=15/.test(r.reply || ""), `reply=${(r.reply || "").slice(0, 100)}`);
  } catch (e) { record("T15 tool-overwrite", false, e.message); }

  // T16 智能体发文件：write_file 生成 → send_file 发送 → 助手消息带文件卡片数据
  {
    const sendPaths = [".e2e-send.txt", "src-tauri/.e2e-send.txt"];
    const delSend = () => sendPaths.forEach((p) => { try { fs.unlinkSync(p); } catch {} });
    try {
      delSend();
      const r = await chat(sid(16), "E2E-CMD-SEND go");
      const send = (r.messages || [])
        .flatMap((m) => m.tool_calls || [])
        .find((c) => c.tool === "send_file");
      const okSend = !!send && send.ok && !!send.params?.path && send.result?.sent === true;
      record("T16 agent-send-file", okSend, `reply=${(r.reply || "").slice(0, 60)} send=${JSON.stringify(send || null).slice(0, 140)}`);
    } catch (e) { record("T16 agent-send-file", false, e.message); } finally { delSend(); }
  }

  // T17 AI 自管理工具：add_tool 自建 → delete_tool 删内置（被拒） → delete_tool 删自建（成功）
  {
    try {
      const r = await chat(sid(17), "E2E-CMD-DELTOOL go");
      const calls = (r.messages || []).flatMap((m) => m.tool_calls || []);
      const blockedCall = calls.find((c) => c.tool === "delete_tool" && c.params?.name === "shell");
      const delCall = calls.find((c) => c.tool === "delete_tool" && c.params?.name === "e2e-temp-tool");
      const okBlocked = !!blockedCall && blockedCall.ok === false && /cannot be deleted|不允许删除/.test(JSON.stringify(blockedCall.result || ""));
      const okDeleted = !!delCall && delCall.ok === true && delCall.result?.deleted === "e2e-temp-tool";
      // 清单校验：自建工具已消失，内置 shell 仍在，delete_tool 本身在内置清单里
      const q = await getJson("/api/tools");
      const names = (q.tools || []).map((x) => x.name);
      const okManifest = !names.includes("e2e-temp-tool") && names.includes("shell") && names.includes("delete_tool");
      record(
        "T17 ai-delete-tool",
        okBlocked && okDeleted && okManifest,
        `blocked=${okBlocked} deleted=${okDeleted} manifest=${okManifest} reply=${(r.reply || "").slice(0, 80)}`
      );
    } catch (e) { record("T17 ai-delete-tool", false, e.message); }
  }

  // T18 看图工具：view_image 读取本地图片 → 图片注入下一轮请求（mock 确认收到 image_url）→ 记录脱敏无 base64
  {
    const PNG_B64 =
      "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";
    const viewPaths = [".e2e-view.png", "src-tauri/.e2e-view.png"];
    const delView = () => viewPaths.forEach((p) => { try { fs.unlinkSync(p); } catch {} });
    try {
      delView();
      // BIT 进程工作目录为 src-tauri，相对路径 ./ 解析在那里；两处都放一份兜底
      fs.writeFileSync("src-tauri/.e2e-view.png", Buffer.from(PNG_B64, "base64"));
      fs.writeFileSync(".e2e-view.png", Buffer.from(PNG_B64, "base64"));
      const r = await chat(sid(18), "E2E-CMD-VIEWIMG go");
      const v = (r.messages || [])
        .flatMap((m) => m.tool_calls || [])
        .find((c) => c.tool === "view_image");
      const okCalled = !!v && v.ok === true && v.result?.seen === true;
      // result 可能是字符串错误（如 File not found），in 运算符前先确认是对象
      const okSanitized = !!v && typeof v.result === "object" && v.result !== null && !("data_url" in v.result);
      const okSeen = /E2E-IMAGE-SEEN count=1/.test(r.reply || "");
      record(
        "T18 view-image",
        okCalled && okSanitized && okSeen,
        `called=${okCalled} sanitized=${okSanitized} seen=${okSeen} reply=${(r.reply || "").slice(0, 60)}`
      );
    } catch (e) { record("T18 view-image", false, e.message); } finally { delView(); }
  }

  // T19 主动压缩对话：compact_history 用摘要替换全部历史（保留尾部现场）
  {
    try {
      const r = await chat(sid(19), "E2E-CMD-COMPACT go");
      const c = (r.messages || [])
        .flatMap((m) => m.tool_calls || [])
        .find((x) => x.tool === "compact_history");
      const msgs = r.messages || [];
      const okCall = !!c && c.ok === true;
      const okSummary = msgs.length >= 1 && String(msgs[0].content || "").includes("E2E-SUMMARY-MARK");
      const okShrunk = msgs.length <= 4;
      record(
        "T19 compact-history",
        okCall && okSummary && okShrunk,
        `call=${okCall} summary=${okSummary} len=${msgs.length} reply=${(r.reply || "").slice(0, 60)}`
      );
    } catch (e) { record("T19 compact-history", false, e.message); }
  }

  // T20 远程访问鉴权：缺失密码 / 错误密码 / 错误 Key 均拒绝（负例在路由前被 auth 中间件拦截）
  {
    try {
      // 密码校验关闭时缺失/错误密码应放行（仅靠 Client Key）
      const noPwdCode = cfg.password_enabled ? 401 : 200;
      const noPwd = await getStatus("/api/tools", { Authorization: `Bearer ${KEY}` });
      const badPwd = await getStatus("/api/tools", { Authorization: `Bearer ${KEY}`, "X-Access-Password": "00000000" });
      const badKey = await getStatus("/api/tools", { Authorization: "Bearer bit_wrongkey_e2e", "X-Access-Password": PASSWORD });
      const ok = noPwd.code === noPwdCode && badPwd.code === noPwdCode && badKey.code === 401;
      record("T20 remote-auth", ok, `noPwd=${noPwd.code}(exp ${noPwdCode}) badPwd=${badPwd.code} badKey=${badKey.code}`);
    } catch (e) { record("T20 remote-auth", false, e.message); }
  }

  // T22 思考过程流式转发：/v1 端点把上游 reasoning_content 以思考增量转发，
  // 且思考标记只出现在 reasoning_content 块、不混入正文 content 块
  try {
    const r = await getSSE("/v1/chat/completions", { messages: [{ role: "user", content: "E2E-STREAM-THINK 检查思考转发" }], stream: true });
    const lines = r.sse.split("\n").filter((l) => l.startsWith("data: "));
    const thinkLine = lines.find((l) => l.includes("E2E-THINK-MARK"));
    const textLine = lines.find((l) => l.includes("E2E-THINK-FINAL"));
    const okThink = r.code === 200 && !!thinkLine && thinkLine.includes("reasoning_content");
    const okText = !!textLine && textLine.includes("content") && !textLine.includes("reasoning_content");
    record("T22 think-sse-forward", okThink && okText, `code=${r.code} think=${okThink} text=${okText} body=${r.sse.slice(0, 80)}`);
  } catch (e) { record("T22 think-sse-forward", false, e.message); }

  // T23 思考过程落库：/api/chat 聚合回合内 reasoning_content 随助手消息持久化，
  // 思考与正文分离（思考进 thinking 字段，不混入 content）
  try {
    const r = await chat(sid(23), "E2E-STREAM-THINK 验证思考落库");
    const asst = (r.messages || []).filter((m) => m.role === "assistant").pop();
    const okReply = /E2E-THINK-FINAL/.test(r.reply || "");
    const okThink = !!asst?.thinking && asst.thinking.includes("E2E-THINK-MARK");
    const notMixed = !asst || !/E2E-THINK-MARK/.test(asst.content || "");
    record("T23 think-persist", okReply && okThink && notMixed, `reply=${(r.reply || "").slice(0, 60)} thinking=${(asst?.thinking || "(无)").slice(0, 60)}`);
  } catch (e) { record("T23 think-persist", false, e.message); }

  // T24 中断执行中的工具：先发慢命令（sleep 2）→ 600ms 后远程置位中断标志 →
  // 回合以「对话已中断」结束，会话现场保留
  {
    try {
      const s = sid(24);
      const pending = call("/api/chat", { session_id: s, message: "E2E-CMD-SLEEP slow" });
      await new Promise((r) => setTimeout(r, 600));
      const ir = await call("/api/debug/interrupt", { session_id: s });
      const hit = ir.code === 200 && JSON.parse(ir.body).interrupted === true;
      let aborted = false;
      try {
        const r = await pending;
        aborted = r.code !== 200 && /对话已中断/.test(r.body);
      } catch {
        aborted = true;
      }
      record("T24 interrupt-running-tool", hit && aborted, `hit=${hit} aborted=${aborted}`);
    } catch (e) {
      record("T24 interrupt-running-tool", false, e.message);
    }
  }

  // T25 中断后下一回合正常：确认暂停不破坏原逻辑（无脏标志残留，新回合完整跑完）
  try {
    const r = await chat(sid(24), "E2E-PLAIN 中断后的新消息");
    record("T25 post-interrupt-turn-alive", /E2E-FINAL-PLAIN/.test(r.reply || ""), `reply=${(r.reply || "").slice(0, 80)}`);
  } catch (e) {
    record("T25 post-interrupt-turn-alive", false, e.message);
  }

  // T26 截断自动续发：文本协议轮输出半截工具 JSON → looks_truncated 命中 →
  // 自动补发「继续」→ 拿到完整答案；截断轮可见片段已清洗（不含 JSON 残尾）并落库
  try {
    const r = await chat(sid(26), "E2E-CMD-CONTINUE go");
    const okReply = /E2E-CONTINUE-OK/.test(r.reply || "");
    const msgs = r.messages || [];
    const okCleanup = msgs.some(
      (m) => m.role === "assistant" && m.content.includes("好的我先把文件写上") && !m.content.includes("write_file")
    );
    record("T26 truncated-auto-continue", okReply && okCleanup, `reply=${(r.reply || "").slice(0, 60)} cleanup=${okCleanup}`);
  } catch (e) {
    record("T26 truncated-auto-continue", false, e.message);
  }

  // T27 同会话回合互斥：回合执行中第二个请求快速失败（明确报错）、零痕迹（不落消息），
  // 首回合不受影响正常完成
  try {
    const s = sid(27);
    const first = call("/api/chat", { session_id: s, message: "E2E-CMD-SLEEP slow" });
    await new Promise((r) => setTimeout(r, 800));
    const second = await call("/api/chat", { session_id: s, message: "E2E-PLAIN concurrent" });
    const busyRejected = second.code !== 200 && /正在执行/.test(second.body);
    const fr0 = await first;
    const fr = fr0.code === 200 ? JSON.parse(fr0.body) : { reply: "", messages: [] };
    const firstOk = fr0.code === 200 && /e2e-slept/.test(fr.reply || "");
    const noLeak = !(fr.messages || []).some((m) => m.content === "E2E-PLAIN concurrent");
    record("T27 busy-session-guard", busyRejected && firstOk && noLeak, `busy=${busyRejected} firstOk=${firstOk} noLeak=${noLeak} second=${second.code}`);
  } catch (e) {
    record("T27 busy-session-guard", false, e.message);
  }

  // T28 无 index 的流式 tool_calls（网关形态）：两个调用按 id 分槽执行、都不丢
  try {
    const r = await chat(sid(28), "E2E-NOINDEX go");
    const ok = /alpha-one/.test(r.reply || "") && /beta-two/.test(r.reply || "");
    record("T28 noindex-toolcalls", ok, `reply=${(r.reply || "").slice(0, 100)}`);
  } catch (e) {
    record("T28 noindex-toolcalls", false, e.message);
  }

  // T29 智能引号/全角标点跑偏 JSON（文本协议）：jsonish_repair 兜底后工具正常执行
  try {
    const r = await chat(sid(29), "E2E-SMART-JSON go");
    record("T29 smart-quote-json", /smart-ok/.test(r.reply || ""), `reply=${(r.reply || "").slice(0, 100)}`);
  } catch (e) {
    record("T29 smart-quote-json", false, e.message);
  }

  // T30 网络瞬断自动重试：首轮流式半截断开（瞬态错误）→ 自动重走本轮 → 重试拿到完整答案
  try {
    const r = await chat(sid(30), "E2E-NETFLAP go");
    const ok = /E2E-NETFLAP-OK/.test(r.reply || "") && !/半截/.test(r.reply || "");
    record("T30 net-flap-retry", ok, `reply=${(r.reply || "").slice(0, 100)}`);
  } catch (e) {
    record("T30 net-flap-retry", false, e.message);
  }

  // T31 上游硬错误不重试：200 + error body 属业务错误 → 直接失败、不烧重试
  try {
    await chat(sid(31), "E2E-NET-HARD go");
    record("T31 upstream-error-no-retry", false, "should have failed");
  } catch (e) {
    record("T31 upstream-error-no-retry", /上游返回错误|mock hard upstream failure/.test(e.message), e.message.slice(0, 120));
  }

  // T32 目标自动推进：plan 建 2 步目标 → 系统自动把规划的下一步发给 AI 续跑，
  // 直至目标标记 achieved，全程无需人工「继续」；断言最终回复 + 落库的自动推进消息数
  try {
    const s32 = sid(32);
    const r = await chat(s32, "E2E-AUTODRIVE go");
    const okReply = /E2E-AUTODRIVE-DONE/.test(r.reply || "");
    let autoMsgs = -1;
    let goalStatus = "?";
    try {
      const det = JSON.parse((await callGet(`/api/debug/sessions/${s32}`)).body);
      autoMsgs = (det.messages || []).filter((m) => m.role === "user" && String(m.content || "").includes("继续（自动推进）")).length;
    } catch {}
    try {
      const gl = JSON.parse((await callGet("/api/debug/goals")).body);
      // 按 session_id 限定：历史 run 残留的同名目标不得干扰断言
      goalStatus = ((gl.goals || []).find((g) => /E2E-AUTODRIVE/.test(g.title || "") && g.session_id === s32) || {}).status || "missing";
    } catch {}
    record("T32 auto-drive-goal", okReply && autoMsgs >= 3 && goalStatus === "achieved", `reply=${(r.reply || "").slice(0, 60)} autoMsgs=${autoMsgs} goal=${goalStatus}`);
  } catch (e) {
    record("T32 auto-drive-goal", false, e.message);
  }

  // T33 幻觉防护-词重复熔断：mock 回复里同词 25 次（默认阈值 20）→
  // BIT 在回复尾部附 [repetition-guard] 标记（同时暂停该会话 auto-drive）
  try {
    const r = await chat(sid(33), "E2E-REPEAT go");
    const reply = (r.reply || "").replace(/\n/g, " ");
    const okGuard = /\[repetition-guard\] word "测试" repeated \d+ times \(limit \d+\)/.test(reply);
    record("T33 word-repeat-guard", okGuard, `reply=${reply.slice(0, 100)}`);
  } catch (e) {
    record("T33 word-repeat-guard", false, e.message);
  }

  // T34 幻觉防护-工具死循环熔断：/api/debug/config 运行时下调 tool_loop_max=3 →
  // mock 每轮都继续调工具 → 第 4 轮起拒绝执行，回复附 [tool-loop-guard] stopped after 3 (limit 3)；
  // 结束后恢复默认阈值，避免影响其它用例
  try {
    const c34 = await call("/api/debug/config", { tool_loop_max: 3 });
    const okSet = c34.code === 200 && JSON.parse(c34.body).tool_loop_max === 3;
    const r = await chat(sid(34), "E2E-TOOLLOOP go");
    const reply = (r.reply || "").replace(/\n/g, " ");
    const shells = (r.messages || []).flatMap((m) => m.tool_calls || []).filter((c) => c.tool === "shell");
    const okGuard = /\[tool-loop-guard\] stopped after 3 tool rounds \(limit 3\)/.test(reply);
    const okRan = shells.length === 3 && shells.every((c) => c.ok);
    record("T34 tool-loop-guard", okSet && okGuard && okRan, `set=${okSet} guard=${okGuard} shells=${shells.length} reply=${reply.slice(0, 80)}`);
  } catch (e) {
    record("T34 tool-loop-guard", false, e.message);
  } finally {
    try { await call("/api/debug/config", { tool_loop_max: 20, word_repeat_max: 20 }); } catch {}
  }

  // T35 模型最大上下文自动获取：实例启动时后台拉 mock 的 /v1/models（context_length=8192，
  // 激活模型 mock-model-a）→ /api/context/metrics 应返回 max_context=8192；轮询等待后台拉取完成
  try {
    let ok35 = false;
    let last35 = "";
    for (let i = 0; i < 20; i++) {
      const m = await callGet("/api/context/metrics");
      if (m.code === 200) {
        const body = JSON.parse(m.body);
        last35 = `est=${body.est_tokens} max=${body.max_context}`;
        if (body.max_context === 8192) {
          ok35 = Number.isFinite(body.est_tokens);
          break;
        }
      }
      await new Promise((r) => setTimeout(r, 500));
    }
    record("T35 model-max-context", ok35, last35);
  } catch (e) {
    record("T35 model-max-context", false, e.message);
  }

  // T36 远程对话限速：chat_rpm_max 运行时下调为 3 并清空计数窗口 →
  // 第 4 个 /api/chat 请求应 429（英文提示）；测完恢复 E2E 默认（0=不限）并清窗
  try {
    const s36 = sid(36);
    await call("/api/debug/config", { chat_rpm_max: 3, chat_rate_reset: true });
    const codes = [];
    let body429 = "";
    for (let i = 0; i < 4; i++) {
      const r = await call("/api/chat", { session_id: s36, message: `E2E-ECHO rate-${i}` });
      codes.push(r.code);
      if (r.code === 429) body429 = r.body || "";
    }
    const ok36 =
      codes.slice(0, 3).every((c) => c === 200) &&
      codes[3] === 429 &&
      body429.includes("rate limited");
    record("T36 chat-rate-limit", ok36, `codes=${codes.join(",")}`);
  } catch (e) {
    record("T36 chat-rate-limit", false, e.message);
  } finally {
    try { await call("/api/debug/config", { chat_rpm_max: 0, chat_rate_reset: true }); } catch {}
  }

  // ── T37-T39 审批真实链路：ask / auto / allow_all 三种模式对远程工具调用的真实门禁 ──
  // 远程 invoke 与 AI 工具调用共用同一张审批表（本地 UI 弹卡片 / 远程轮询 POST /api/approvals/{id} 应答）
  const pollApproval = async (tool, ms = 6000) => {
    const t0 = Date.now();
    while (Date.now() - t0 < ms) {
      const r = await callGet("/api/approvals");
      const arr = (JSON.parse(r.body || "{}").approvals) || [];
      const hit = arr.find((a) => a.tool === tool);
      if (hit) return hit;
      await new Promise((res) => setTimeout(res, 200));
    }
    return null;
  };
  const approvalsLeft = async () =>
    (JSON.parse((await callGet("/api/approvals")).body || "{}").approvals) || [];
  let shellId = "", viewId = "";
  try {
    const tl = JSON.parse((await callGet("/api/tools")).body || "{}");
    shellId = (tl.tools || []).find((x) => x.name === "shell")?.id || "";
    viewId = (tl.tools || []).find((x) => x.name === "view_image")?.id || "";
  } catch {}

  // T37 ask 模式 + 允许：invoke shell 挂起等审批 → 审批列表可见 → allow=true → 拿到真实执行输出
  try {
    await call("/api/debug/config", { tool_approval: "ask" });
    const pend = call(`/api/tools/${shellId}/invoke`, { params: { command: "echo e2e-approval-ok" } });
    const ap = await pollApproval("shell");
    if (!ap) throw new Error("ask 模式下 shell 未进入审批列表");
    const ans = await call(`/api/approvals/${ap.id}`, { allow: true });
    const inv = await pend;
    const left = await approvalsLeft();
    const ok37 =
      ans.code === 200 && inv.code === 200 &&
      (inv.body || "").includes("e2e-approval-ok") && left.length === 0;
    record("T37 approval-ask-allow", ok37,
      `ans=${ans.code} inv=${inv.code} body=${(inv.body || "").slice(0, 80)} left=${left.length}`);
  } catch (e) {
    record("T37 approval-ask-allow", false, e.message);
  }

  // T38 ask 模式 + 拒绝：allow=false → invoke 返回 403 User rejected，工具不执行
  try {
    await call("/api/debug/config", { tool_approval: "ask" });
    const pend = call(`/api/tools/${shellId}/invoke`, { params: { command: "echo e2e-should-not-run" } });
    const ap = await pollApproval("shell");
    if (!ap) throw new Error("ask 模式下 shell 未进入审批列表");
    await call(`/api/approvals/${ap.id}`, { allow: false });
    const inv = await pend;
    const ok38 = inv.code === 403 && (inv.body || "").includes("rejected");
    record("T38 approval-ask-deny", ok38, `inv=${inv.code} body=${(inv.body || "").slice(0, 90)}`);
  } catch (e) {
    record("T38 approval-ask-deny", false, e.message);
  }

  // T39 auto / allow_all 语义：auto 下安全工具（view_image）免审直接执行（快速报错=未进审批），
  // 危险工具（shell）仍须审批；allow_all 下全部直接放行且审批表保持为空
  try {
    await call("/api/debug/config", { tool_approval: "auto" });
    const safe = await call(`/api/tools/${viewId}/invoke`, { params: { path: "/tmp/e2e-no-such-image.png" } });
    const leftA = await approvalsLeft();
    const safeNoGate = safe.code === 400 && (safe.body || "").includes("File not found") && leftA.length === 0;

    const pend = call(`/api/tools/${shellId}/invoke`, { params: { command: "echo e2e-auto-approval" } });
    const ap = await pollApproval("shell");
    if (!ap) throw new Error("auto 模式未拦下危险工具 shell");
    await call(`/api/approvals/${ap.id}`, { allow: true });
    const inv = await pend;
    const dangerousGated = inv.code === 200 && (inv.body || "").includes("e2e-auto-approval");

    await call("/api/debug/config", { tool_approval: "allow_all" });
    const direct = await call(`/api/tools/${shellId}/invoke`, { params: { command: "echo e2e-allow-all-direct" } });
    const leftB = await approvalsLeft();
    const allowAll = direct.code === 200 && (direct.body || "").includes("e2e-allow-all-direct") && leftB.length === 0;

    record("T39 approval-modes", safeNoGate && dangerousGated && allowAll,
      `safe=${safe.code}/${safeNoGate} gated=${inv.code}/${dangerousGated} all=${direct.code}/${allowAll}`);
  } catch (e) {
    record("T39 approval-modes", false, e.message);
  } finally {
    try { await call("/api/debug/config", { tool_approval: "allow_all" }); } catch {}
  }

  // ── T40 云中继隧道：128 位识别码生成/持久化 + 二维码三种连接方式 + fake_relay 全链路转发 ──
  // fake_relay.cjs（默认 127.0.0.1:9802，可用 FAKE_RELAY_PORT 换口）模拟 Cloudflare Worker
  // 协议（poll/req/answer），BIT 中继循环主动出站长轮询，隧道请求回环打本地 API 后原样回传
  const RPORT = Number(process.env.FAKE_RELAY_PORT) || 9802;
  // 原始隧道请求（不带许可逻辑；负例/握手测试用）；port 可选覆盖（T57 专用时代子中继 9803）
  const rawReq = (rid, path, method, headers, body, port) =>
    new Promise((resolve, reject) => {
      const req = http.request(
        { host: BASE, port: port || RPORT, path: `/relay/req/${rid}${path}`, method,
          headers: headers || {}, timeout: 25000, agent },
        (res) => { let b = ""; res.on("data", (c) => (b += c)); res.on("end", () => resolve({ code: res.statusCode, body: b, headers: res.headers })); }
      );
      req.on("error", reject);
      req.on("timeout", () => { req.destroy(); reject(new Error("tunnel timeout")); });
      if (body) req.write(body);
      req.end();
    });
  // 取一次性挑战（连接许可握手第一步）；无 c 视为硬错误（封禁/限流等，响应体带原因）
  const challenge = (rid, port) =>
    new Promise((resolve, reject) => {
      const q = http.get({ host: BASE, port: port || RPORT, path: `/relay/challenge/${rid}`, agent, timeout: 10000 },
        (r) => { let b = ""; r.on("data", (c) => (b += c)); r.on("end", () => {
          try {
            const c = JSON.parse(b).c;
            if (!c) return reject(new Error(`challenge no-c code=${r.statusCode} body=${b.slice(0, 80)}`));
            resolve(c);
          } catch (e) { reject(new Error(`challenge bad code=${r.statusCode} body=${b.slice(0, 80)}`)); }
        }); });
      q.on("error", reject); q.on("timeout", () => { q.destroy(); reject(new Error("challenge timeout")); });
    });
  // 连接许可（permit）缓存（rid → token）：模拟手机端"取挑战 → 应答 → 持许可请求"闭环。
  // 握手密钥（X-BIT-Bind）随 BIT poll 下发就位后，隧道请求必须持许可或现场握手；
  // helper 自动完成，收到 "permit required" 403 时清缓存重新握手一次（过期/驱逐自愈）
  const permitCache = new Map();
  const tunnelReq = async (rid, path, method, headers, body) => {
    const b = body == null ? null : (typeof body === "string" || Buffer.isBuffer(body) ? body : JSON.stringify(body));
    // 调用方自带 content-type（如 T50 二进制 415 用例的 octet-stream）时原样保留
    const hasCT = Object.keys(headers || {}).some((k) => k.toLowerCase() === "content-type");
    const ct = b && !hasCT ? { "content-type": "application/json" } : {};
    const authHeaders = { ...headers, ...ct };
    if (permitCache.has(rid)) {
      authHeaders["x-bit-permit"] = permitCache.get(rid);
    } else {
      const c = await challenge(rid);
      Object.assign(authHeaders, { "x-bit-challenge": c, "x-bit-proof": proofOf(workerBind(KEY, rid), c) });
    }
    let r = await rawReq(rid, path, method, authHeaders, b);
    if (r.headers?.["x-bit-permit"]) permitCache.set(rid, r.headers["x-bit-permit"]);
    if (r.code === 403 && /permit required/.test(r.body)) {
      permitCache.delete(rid);
      const c = await challenge(rid);
      const retry = { ...headers, ...ct, "x-bit-challenge": c, "x-bit-proof": proofOf(workerBind(KEY, rid), c) };
      r = await rawReq(rid, path, method, retry, b);
      if (r.headers?.["x-bit-permit"]) permitCache.set(rid, r.headers["x-bit-permit"]);
    }
    return r;
  };

  // 设备凭证就绪：bitsign-v2 材料依赖 device_key（启动自动注册），后续全部隧道用例共用
  const MAT = await devMat();

  try {
    // 1) 配置云中继入口 → 中继循环热生效（无需重启）
    const setRelay = await call("/api/debug/config", { cloud_relay_url: `http://127.0.0.1:${RPORT}` });
    if (setRelay.code !== 200) throw new Error(`set cloud_relay_url: ${setRelay.code}`);
    // 2) 二维码 payload：v2 + 128 位识别码 + 三种连接方式 + NAT 探测字段
    const p1 = JSON.parse((await callGet("/api/qr")).body || "{}");
    const ridOk = /^[0-9a-f]{32}$/.test(p1.rid || "");
    const methodsOk =
      Array.isArray(p1.methods?.lan) && p1.methods.lan.length > 0 &&
      Array.isArray(p1.methods?.direct6) &&
      p1.methods?.relay === `http://127.0.0.1:${RPORT}/relay/${p1.rid}`;
    const natOk = ["none", "cone", "symmetric", "unknown"].includes(p1.nat);
    // 3) 识别码持久化：二次获取一致 + 实例数据目录 config.json 落盘一致
    const p2 = JSON.parse((await callGet("/api/qr")).body || "{}");
    let persisted = p2.rid === p1.rid;
    if (persisted) {
      try {
        const dd = String(JSON.parse((await callGet("/api/debug/state")).body).data_dir || "");
        persisted = JSON.parse(fs.readFileSync(dd + "/config.json", "utf8")).relay_id === p1.rid;
      } catch { persisted = false; }
    }
    // 4) 隧道 GET：/api/health 经中继转发回本地 API（真鉴权头原样透传 + bitsign-v2 签名）
    const rGet = await tunnelReq(p1.rid, "/api/health", "GET", {
      Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD,
      ...signHeaders(KEY, MAT, p1.rid, "GET", "/api/health"),
    });
    let statsOk = false;
    try { statsOk = rGet.code === 200 && JSON.parse(rGet.body).ok === true; } catch {}
    // 5) 隧道 POST：对话请求体 base64 往返（体转发 + 最终回复断言）
    const rPost = await tunnelReq(p1.rid, "/api/chat", "POST", {
      "Content-Type": "application/json", Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD,
      ...signHeaders(KEY, MAT, p1.rid, "POST", "/api/chat"),
    }, JSON.stringify({ session_id: sid(40), message: "E2E-PLAIN via-relay" }));
    let postOk = false;
    try { postOk = rPost.code === 200 && /E2E-FINAL-PLAIN/.test(JSON.parse(rPost.body).reply || ""); } catch {}
    // 6) 隧道负例×2（/api/health 免鉴权、/api/qr 被敏感路径拦截，故 badkey 用需鉴权的 /api/tools）：
    //    无签名 → 中继层 403（第三方 App 借道被网站门槛拦截）；
    //    签名但密钥错 → 本地 API 401（签名只是信道门槛，不是万能钥匙）
    const rNoSign = await tunnelReq(p1.rid, "/api/health", "GET", { Authorization: "Bearer e2e-wrong-key" });
    const rBadKey = await tunnelReq(p1.rid, "/api/tools", "GET", {
      Authorization: "Bearer e2e-wrong-key", "X-Access-Password": PASSWORD,
      ...signHeaders(KEY, MAT, p1.rid, "GET", "/api/tools"),
    });
    const badOk = rNoSign.code === 403 && rBadKey.code === 401;
    record("T40 relay-tunnel", ridOk && methodsOk && natOk && persisted && statsOk && postOk && badOk,
      `rid=${ridOk} methods=${methodsOk} nat=${p1.nat} persist=${persisted} get=${rGet.code} post=${rPost.code} nosign=${rNoSign.code} badkey=${rBadKey.code}`);
  } catch (e) {
    record("T40 relay-tunnel", false, e.message);
  } finally {
    try { await call("/api/debug/config", { cloud_relay_url: "" }); } catch {}
  }

  // ── T41 多设备会话隔离：空 session_id 拒绝 + 双设备并发对话不串线 ──
  try {
    const sA = sid(41) + "-devA";
    const sB = sid(41) + "-devB";
    // 1) 负例：缺 session_id 必须 400（绝不容许落入桌面激活会话）
    const noSid = await call("/api/chat", { message: "E2E-PLAIN no-sid" });
    const noSidOk = noSid.code === 400 && /session_id/.test(noSid.body);
    // 2) 双设备并发：同一时刻 A/B 各发一条（不同标记），回复互属
    const [ra, rb] = await Promise.all([
      call("/api/chat", { session_id: sA, message: "E2E-PLAIN devA-ping" }),
      call("/api/chat", { session_id: sB, message: "E2E-PLAIN devB-ping" }),
    ]);
    const bothOk = ra.code === 200 && rb.code === 200;
    // 3) 历史隔离：A 的消息流只含 devA 标记，B 只含 devB；互不出现对方文本
    const ha = await getJson(`/api/debug/sessions/${encodeURIComponent(sA)}`);
    const hb = await getJson(`/api/debug/sessions/${encodeURIComponent(sB)}`);
    const ta = JSON.stringify(ha.messages || []);
    const tb = JSON.stringify(hb.messages || []);
    const isolated = bothOk &&
      ta.includes("devA-ping") && !ta.includes("devB-ping") &&
      tb.includes("devB-ping") && !tb.includes("devA-ping") &&
      (ha.messages || []).filter((m) => m.role === "user").length === 1 &&
      (hb.messages || []).filter((m) => m.role === "user").length === 1;
    // 4) 追写隔离：仅 A 追加一轮 → B 消息数不变（A/B 无共享状态）
    const before = (hb.messages || []).length;
    await chat(sA, "E2E-PLAIN devA-second");
    const hb2 = await getJson(`/api/debug/sessions/${encodeURIComponent(sB)}`);
    const noCross = (hb2.messages || []).length === before;
    record("T41 session-isolation", noSidOk && isolated && noCross,
      `noSid=${noSid.code} concurrent=${bothOk} isolated=${isolated} noCross=${noCross}`);
  } catch (e) {
    record("T41 session-isolation", false, e.message);
  }

  // ── T42 敏感词审核：内置词库拦输入（403）+ 自定词表拦输出（回复替换）──
  try {
    const s42 = sid(42);
    // 1) 输入命中内置词库（空白/全角绕过已被归一化拦截）
    const blocked = await call("/api/chat", { session_id: s42, message: "说说冰 毒制作的流程" });
    const inOk = blocked.code === 403 && /moderation blocked/.test(blocked.body);
    // 2) 输出命中：自定词表把 mock 固定回复标记为敏感词 → 回复被替换为拒绝语
    await call("/api/debug/config", { blocked_words: "E2E-FINAL-PLAIN" });
    const outResp = await call("/api/chat", { session_id: s42, message: "E2E-PLAIN ping" });
    let outJson = {};
    try { outJson = JSON.parse(outResp.body); } catch {}
    const outOk = outResp.code === 200 && /withheld by moderation/.test(outJson.reply || "");
    // 3) 恢复内置词库后同一消息放行
    await call("/api/debug/config", { blocked_words: [] });
    const clean = await call("/api/chat", { session_id: s42, message: "E2E-PLAIN ping" });
    const cleanOk = clean.code === 200 && /E2E-FINAL-PLAIN/.test(JSON.parse(clean.body || "{}").reply || "");
    record("T42 moderation", inOk && outOk && cleanOk,
      `in=${inOk} out=${outOk} clean=${cleanOk} code=${blocked.code}`);
  } catch (e) {
    record("T42 moderation", false, e.message);
  }

  // ── T43 每 IP 并发上限：max_active_per_ip=1 时第二个在途请求 429 ──
  try {
    const s43 = sid(43);
    await call("/api/debug/config", { max_active_per_ip: 1, chat_rate_reset: true });
    const slow = call("/api/chat", { session_id: s43, message: "E2E-CMD-SLEEP slow" }); // sleep 2，占住在途名额
    await new Promise((r) => setTimeout(r, 500));
    const second = await call("/api/chat", { session_id: s43, message: "E2E-PLAIN quick" });
    const slowBody = await slow;
    await call("/api/debug/config", { max_active_per_ip: 3, chat_rate_reset: true });
    const guardOk = second.code === 429 && /concurrent/.test(second.body);
    record("T43 concurrency-limit", guardOk && slowBody.code === 200,
      `second=${second.code}/exp429 slow=${slowBody.code}`);
  } catch (e) {
    record("T43 concurrency-limit", false, e.message);
  } finally {
    try { await call("/api/debug/config", { max_active_per_ip: 3 }); } catch {}
  }

  // ── T44 IP 黑名单：中继透传伪造源 IP 命中黑名单 → 403；清空后恢复 ──
  try {
    await call("/api/debug/config", { cloud_relay_url: `http://127.0.0.1:${RPORT}` });
    const rid44 = JSON.parse((await callGet("/api/qr")).body).rid;
    await call("/api/debug/config", { ip_blocklist: "203.0.113.7" });
    // 伪造源 IP = 黑名单中的地址（fake_relay 把 x-fake-client-ip 写进 envelope.ip）
    const hit = await tunnelReq(rid44, "/api/health", "GET", {
      Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD, "x-fake-client-ip": "203.0.113.7",
      ...signHeaders(KEY, MAT, rid44, "GET", "/api/health"),
    });
    // 不带伪造 IP（loopback）→ 不在黑名单 → 放行
    const clean = await tunnelReq(rid44, "/api/health", "GET", {
      Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD,
      ...signHeaders(KEY, MAT, rid44, "GET", "/api/health"),
    });
    await call("/api/debug/config", { ip_blocklist: [] });
    record("T44 ip-blocklist", hit.code === 403 && clean.code === 200,
      `hit=${hit.code}/exp403 clean=${clean.code}`);
  } catch (e) {
    record("T44 ip-blocklist", false, e.message);
  } finally {
    try { await call("/api/debug/config", { ip_blocklist: [], cloud_relay_url: "" }); } catch {}
  }

  // ── T45 信道防护：错误签名 403 + 重放 nonce 403 + 站点门槛不可绕过 ──
  try {
    await call("/api/debug/config", { cloud_relay_url: `http://127.0.0.1:${RPORT}` });
    const rid45 = JSON.parse((await callGet("/api/qr")).body).rid;
    // 1) 错误密钥算出的签名 → BIT 验签失败 403
    const badSign = await tunnelReq(rid45, "/api/health", "GET", {
      Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD,
      ...signHeaders("wrong-key-material", MAT, rid45, "GET", "/api/health"),
    });
    // 2) 重放：同一 ts+nonce+sign 第二次使用 → 403 nonce replayed
    //    nonce 每轮唯一（含 RUN）：BIT 端 nonce 缓存窗口 = 签名时间窗（120s），
    //    120s 内连续两轮跑 E2E 时固定 nonce 会被上一轮残留误判为重放
    const fixed = signHeaders(KEY, MAT, rid45, "GET", "/api/health", { ts: Math.floor(Date.now() / 1000), nonce: `replay${RUN}0f` });
    const first = await tunnelReq(rid45, "/api/health", "GET", { Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD, ...fixed });
    const replay = await tunnelReq(rid45, "/api/health", "GET", { Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD, ...fixed });
    // 3) 关闭本地防护后无签名请求 → 仍 403（中继站门槛是服务器强制的，客户端开关绕不过——
    //    开源客户端的防滥用只能靠服务器，这正是 bitsign 站点层存在的意义）
    await call("/api/debug/config", { channel_guard: false });
    const offNoSign = await tunnelReq(rid45, "/api/health", "GET", { Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD });
    // 4) 关闭本地防护 + 合法签名 → 放行（本地豁免不影响合法信道）
    const offSigned = await tunnelReq(rid45, "/api/health", "GET", {
      Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD,
      ...signHeaders(KEY, MAT, rid45, "GET", "/api/health"),
    });
    await call("/api/debug/config", { channel_guard: true });
    record("T45 channel-guard", badSign.code === 403 && first.code === 200 && replay.code === 403 && offNoSign.code === 403 && offSigned.code === 200,
      `badsign=${badSign.code} first=${first.code} replay=${replay.code}/exp403 offNoSign=${offNoSign.code}/exp403 offSigned=${offSigned.code}`);
  } catch (e) {
    record("T45 channel-guard", false, e.message);
  } finally {
    try { await call("/api/debug/config", { channel_guard: true, cloud_relay_url: "" }); } catch {}
  }

  // ── T46 二维码加密块：enc=BIT1: 密文（不含明文 key/rid）+ alg 标识（bitsign-v2）──
  try {
    const p46 = JSON.parse((await callGet("/api/qr")).body);
    const encOk = typeof p46.enc === "string" && p46.enc.startsWith("BIT1:") && p46.enc.length > 64 &&
      !p46.enc.includes(KEY) && !p46.enc.includes(p46.rid);
    const algOk = p46.alg === "bitsign-v2";
    record("T46 qr-encrypted", encOk && algOk, `enc=${encOk} alg=${p46.alg}`);
  } catch (e) {
    record("T46 qr-encrypted", false, e.message);
  }

  // ── T47 中继流式：OpenAI stream:true 经隧道 → SSE 增量完整到达 ──
  try {
    await call("/api/debug/config", { cloud_relay_url: `http://127.0.0.1:${RPORT}` });
    const rid47 = JSON.parse((await callGet("/api/qr")).body).rid;
    const r = await tunnelReq(rid47, "/v1/chat/completions", "POST", {
      "Content-Type": "application/json", Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD,
      ...signHeaders(KEY, MAT, rid47, "POST", "/v1/chat/completions"),
    }, JSON.stringify({ model: "x", stream: true, messages: [{ role: "user", content: "E2E-PLAIN stream via relay" }] }));
    const okSSE = r.code === 200 && r.body.includes("data:") && r.body.includes("[DONE]") &&
      /E2E-FINAL-PLAIN/.test(r.body);
    record("T47 relay-stream", okSSE, `code=${r.code} sse=${okSSE} len=${r.body.length}`);
  } catch (e) {
    record("T47 relay-stream", false, e.message);
  } finally {
    try { await call("/api/debug/config", { cloud_relay_url: "" }); } catch {}
  }

  // ── T48 设备凭证：自动注册落盘（bitdev_* + 时间戳 + 指纹哈希）且幂等稳定 ──
  try {
    const dd = String(JSON.parse((await callGet("/api/debug/state")).body).data_dir || "");
    const read = () => JSON.parse(fs.readFileSync(dd + "/config.json", "utf8"));
    const c1 = read();
    const keyOk = /^bitdev_[0-9a-f]{32}$/.test(c1.device_key || "");
    const tsOk = Number.isInteger(c1.device_registered_at) && c1.device_registered_at > 1_600_000_000;
    const fpOk = /^[0-9a-f]{16}$/.test(c1.device_fp_hash || "");
    // 幂等：再取一致（稳定锚点，不轮换）
    const c2 = read();
    const stable = c1.device_key === c2.device_key && c1.device_registered_at === c2.device_registered_at &&
      c1.device_fp_hash === c2.device_fp_hash;
    // 材料口径：E2E 派生材料与 sign.cjs/device.rs 同源（MAT 已由 devMat() 成功派生即为佐证）
    const matOk = /^[0-9a-f]{16}$/.test(MAT);
    record("T48 device-credential", keyOk && tsOk && fpOk && stable && matOk,
      `key=${keyOk} ts=${tsOk} fp=${fpOk} stable=${stable} mat=${matOk}`);
  } catch (e) {
    record("T48 device-credential", false, e.message);
  }

  // ── T49 中继设备绑定：rid↔设备指纹绑定（首次 poll 独占）+ 无指纹/异设备 poll 拒绝 ──
  try {
    await call("/api/debug/config", { cloud_relay_url: `http://127.0.0.1:${RPORT}` });
    // BIT poller 已用本机 device_fp_hash poll 过 → 绑定生效；status 应返回绑定设备前 8 hex
    const rid49 = JSON.parse((await callGet("/api/qr")).body).rid;
    const dd = String(JSON.parse((await callGet("/api/debug/state")).body).data_dir || "");
    const fp = JSON.parse(fs.readFileSync(dd + "/config.json", "utf8")).device_fp_hash || "";
    const statusRaw = await new Promise((resolve, reject) => {
      const req = http.request({ host: BASE, port: RPORT, path: `/relay/status/${rid49}`, method: "GET", agent },
        (res) => { let b = ""; res.on("data", (c) => (b += c)); res.on("end", () => resolve({ code: res.statusCode, body: b })); });
      req.on("error", reject); req.end();
    });
    let status = JSON.parse(statusRaw.body || "{}");
    // 绑定需要 BIT poller 至少完成一次 poll（配置热生效 ~3s），轮询等待最多 12s
    const deadline49 = Date.now() + 12_000;
    while (!status.dev && Date.now() < deadline49) {
      await new Promise((r) => setTimeout(r, 1000));
      const again = await new Promise((resolve, reject) => {
        const req = http.request({ host: BASE, port: RPORT, path: `/relay/status/${rid49}`, method: "GET", agent },
          (res) => { let b = ""; res.on("data", (c) => (b += c)); res.on("end", () => resolve({ code: res.statusCode, body: b })); });
        req.on("error", reject); req.end();
      });
      status = JSON.parse(again.body || "{}");
    }
    const boundOk = status.dev === fp.slice(0, 8);
    // 无指纹 poll → 403；异设备指纹 poll → 403（绑定不可抢占）
    const rawPoll = (dev) => new Promise((resolve, reject) => {
      const headers = { "Content-Type": "application/json" };
      if (dev) headers["x-bit-dev"] = dev;
      const req = http.request({ host: BASE, port: RPORT, path: `/relay/poll/${rid49}`, method: "POST", headers, agent },
        (res) => { let b = ""; res.on("data", (c) => (b += c)); res.on("end", () => resolve({ code: res.statusCode, body: b })); });
      req.on("error", reject); req.end();
    });
    const noDev = await rawPoll("");
    const otherDev = await rawPoll("aaaaaaaaaaaaaaaa");
    // 保持绑定不变：本机指纹再 poll 仍可用（200/204，非 403）
    const selfDev = await rawPoll(fp);
    record("T49 relay-dev-binding", boundOk && noDev.code === 403 && otherDev.code === 403 && selfDev.code !== 403,
      `bound=${boundOk}(${status.dev}) noDev=${noDev.code} other=${otherDev.code}/exp403 self=${selfDev.code}`);
  } catch (e) {
    record("T49 relay-dev-binding", false, e.message);
  } finally {
    try { await call("/api/debug/config", { cloud_relay_url: "" }); } catch {}
  }

  // ── T50 中继文字信道：二进制 Content-Type 415 + 敏感路径 403（服务器强制）──
  // 信道只传文字：图片/文件/流类型在站点层直接拒绝，/api/qr（凭据回读）与
  // /api/debug/*（可远程改配置）不允许经中继触达——带合法签名也一样
  try {
    await call("/api/debug/config", { cloud_relay_url: `http://127.0.0.1:${RPORT}` });
    const rid50 = JSON.parse((await callGet("/api/qr")).body).rid;
    const bin = await tunnelReq(rid50, "/api/health", "POST", {
      Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD, "Content-Type": "application/octet-stream",
      ...signHeaders(KEY, MAT, rid50, "POST", "/api/health"),
    }, "raw-bytes-not-allowed");
    const sQr = await tunnelReq(rid50, "/api/qr", "GET", {
      Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD,
      ...signHeaders(KEY, MAT, rid50, "GET", "/api/qr"),
    });
    const sDbg = await tunnelReq(rid50, "/api/debug/state", "GET", {
      Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD,
      ...signHeaders(KEY, MAT, rid50, "GET", "/api/debug/state"),
    });
    record("T50 relay-text-only", bin.code === 415 && sQr.code === 403 && sDbg.code === 403,
      `bin=${bin.code}/exp415 qr=${sQr.code}/exp403 debug=${sDbg.code}/exp403`);
  } catch (e) {
    record("T50 relay-text-only", false, e.message);
  } finally {
    try { await call("/api/debug/config", { cloud_relay_url: "" }); } catch {}
  }

  // ── T51 中继体上限：>4MB 文字体 → 站点层 413（防文件管道滥用），正常请求不受影响 ──
  try {
    await call("/api/debug/config", { cloud_relay_url: `http://127.0.0.1:${RPORT}` });
    const rid51 = JSON.parse((await callGet("/api/qr")).body).rid;
    const big = JSON.stringify({ x: "a".repeat(5 * 1024 * 1024) });
    const over = await tunnelReq(rid51, "/api/health", "POST", {
      Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD, "Content-Type": "application/json",
      ...signHeaders(KEY, MAT, rid51, "POST", "/api/health"),
    }, big);
    const within = await tunnelReq(rid51, "/api/health", "GET", {
      Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD,
      ...signHeaders(KEY, MAT, rid51, "GET", "/api/health"),
    });
    record("T51 relay-body-cap", over.code === 413 && within.code === 200,
      `over=${over.code}/exp413 within=${within.code}/exp200`);
  } catch (e) {
    record("T51 relay-body-cap", false, e.message);
  } finally {
    try { await call("/api/debug/config", { cloud_relay_url: "" }); } catch {}
  }

  // ── T52 连接许可：动态握手密钥 + 挑战应答，App 正确沟通才允许连接 ──
  // 握手密钥 S = HMAC(client_key, "bit-worker-bind:{rid}") 随 BIT poll（X-BIT-Bind）
  // 下发服务器后就位；无许可请求从 200 翻转为 403，正确应答一次性挑战才签发许可
  try {
    await call("/api/debug/config", { cloud_relay_url: `http://127.0.0.1:${RPORT}` });
    const rid52 = JSON.parse((await callGet("/api/qr")).body).rid;
    const BIND = workerBind(KEY, rid52);
    // a) 等 BIT poller 送达握手密钥（许可强制生效开关）：无许可请求翻转为 403
    const noPermitHeaders = { Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD,
      ...signHeaders(KEY, MAT, rid52, "GET", "/api/health") };
    let gated = { code: 0, body: "" };
    const deadline52 = Date.now() + 20000;
    for (;;) {
      // 每次尝试重新签名（nonce 一次性，复用会被 BIT 判 replay 干扰判断）；
      // 只认中继侧 "permit required" 403（许可闸门生效信号），BIT 侧 403 继续等
      gated = await rawReq(rid52, "/api/health", "GET", { ...noPermitHeaders,
        ...signHeaders(KEY, MAT, rid52, "GET", "/api/health") });
      if ((gated.code === 403 && /permit required/i.test(gated.body)) || Date.now() > deadline52) break;
      await new Promise((r) => setTimeout(r, 1000));
    }
    // b) 挑战 + 错误应答（用错密钥派生）→ 403，且计坏签名
    const cBad = await challenge(rid52);
    const badProof = await rawReq(rid52, "/api/health", "GET", {
      Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD,
      ...signHeaders(KEY, MAT, rid52, "GET", "/api/health"),
      "x-bit-challenge": cBad, "x-bit-proof": proofOf("ab".repeat(32), cBad),
    });
    // c) 正确握手（S 派生应答）→ 200 + 许可签发（x-bit-permit 响应头带回）
    const cOk = await challenge(rid52);
    const handshake = await rawReq(rid52, "/api/health", "GET", {
      Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD,
      ...signHeaders(KEY, MAT, rid52, "GET", "/api/health"),
      "x-bit-challenge": cOk, "x-bit-proof": proofOf(BIND, cOk),
    });
    const permitTok = handshake.headers?.["x-bit-permit"] || "";
    const permitOk = handshake.code === 200 && /^[0-9a-f]{32}$/.test(permitTok);
    // d) 持许可后续请求直接放行（无需再握手）
    const withPermit = await rawReq(rid52, "/api/health", "GET", {
      Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD,
      ...signHeaders(KEY, MAT, rid52, "GET", "/api/health"),
      "x-bit-permit": permitTok,
    });
    // e) 保活心跳（长连接场景）：持许可 → 200 + online=true + 许可续期；无许可 → 403。
    //    /relay/ka/{rid} 不进隧道队列，App 长时间运行时定期调用保持许可滑动 TTL
    const kaReq = (rid, headers) =>
      new Promise((resolve, reject) => {
        http.get({ host: BASE, port: RPORT, path: `/relay/ka/${rid}`, headers: headers || {}, agent, timeout: 10000 },
          (r) => { let b = ""; r.on("data", (c) => (b += c)); r.on("end", () => resolve({ code: r.statusCode, body: b, headers: r.headers })); })
          .on("error", reject);
      });
    const kaOk = await kaReq(rid52, { Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD,
      ...signHeaders(KEY, MAT, rid52, "GET", "/relay/ka"), "x-bit-permit": permitTok });
    const kaRenew = /^[0-9a-f]{32}$/.test(kaOk.headers?.["x-bit-permit"] || "");
    let kaOnline = false;
    try { kaOnline = JSON.parse(kaOk.body).online === true; } catch {}
    const kaGate = await kaReq(rid52, { Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD,
      ...signHeaders(KEY, MAT, rid52, "GET", "/relay/ka") });
    // f) 来源一致性（BIT 端 IP 比对闸门，确保真实性才放行）：先从新网段 203.0.113.9
    //    正常握手取许可（Worker 端许可本就绑定网段），再模拟"中继被替换 / 盖章被破坏"
    //    ——x-fake-stamp-pfx 强制盖章为不一致网段 → BIT 端比对真实 IP 网段不符 → 403；
    //    盖章恢复一致 → 放行。两个头均由 poller 从信封注入，手机端伪造不了
    const chalF = await challenge(rid52);
    const ipF = "203.0.113.9";
    const mintF = await rawReq(rid52, "/api/health", "GET", {
      Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD,
      ...signHeaders(KEY, MAT, rid52, "GET", "/api/health"),
      "x-bit-challenge": chalF, "x-bit-proof": proofOf(BIND, chalF), "x-fake-client-ip": ipF,
    });
    const permitF = mintF.headers?.["x-bit-permit"] || "";
    const stampBad = await rawReq(rid52, "/api/health", "GET", {
      Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD,
      ...signHeaders(KEY, MAT, rid52, "GET", "/api/health"),
      "x-bit-permit": permitF, "x-fake-client-ip": ipF, "x-fake-stamp-pfx": "198.51.100",
    });
    const stampOk = await rawReq(rid52, "/api/health", "GET", {
      Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD,
      ...signHeaders(KEY, MAT, rid52, "GET", "/api/health"),
      "x-bit-permit": permitF, "x-fake-client-ip": ipF,
    });
    record("T52 channel-permit", gated.code === 403 && /permit required/i.test(gated.body || "")
      && badProof.code === 403 && permitOk && withPermit.code === 200
      && kaOk.code === 200 && kaOnline && kaRenew && kaGate.code === 403
      && permitF && stampBad.code === 403 && /source ip inconsistent/i.test(stampBad.body || "")
      && stampOk.code === 200,
      `gated=${gated.code}/exp403 badProof=${badProof.code}/exp403 handshake=${handshake.code} permit=${permitOk} withPermit=${withPermit.code}/exp200 ka=${kaOk.code}/online=${kaOnline}/renew=${kaRenew} kaGate=${kaGate.code}/exp403 stampBad=${stampBad.code}/exp403 stampOk=${stampOk.code}/exp200`);
  } catch (e) {
    record("T52 channel-permit", false, e.message);
  } finally {
    try { await call("/api/debug/config", { cloud_relay_url: "" }); } catch {}
  }

  // ── T53 蜜罐：诱捕路径触碰即封禁 + 警告（与 Worker 站点层同语义）──
  try {
    const rid53 = "f".repeat(32); // 蜜罐测试自用 rid（_reset 清态，与 T52 盒子无关）
    const rawGet = (p) =>
      new Promise((resolve, reject) => {
        http.get({ host: BASE, port: RPORT, path: p, agent, timeout: 10000 },
          (r) => { let b = ""; r.on("data", (c) => (b += c)); r.on("end", () => resolve({ code: r.statusCode, body: b })); })
          .on("error", reject);
      });
    // 幂等前置：清历史封禁（上一批 T53 遗留），再触碰蜜罐
    await rawGet("/relay/_reset");
    const trap = await rawGet("/relay/admin");
    const trapped = trap.code === 403 && /honeypot|don't bother/i.test(trap.body);
    // 封禁生效：蜜罐命中后同一 IP 连挑战接口都拿不到
    const chalBanned = await rawGet(`/relay/challenge/${rid53}`);
    const bannedOk = chalBanned.code === 403 && /banned/i.test(chalBanned.body);
    // 收尾再清，蜜罐封禁不泄漏到后续 E2E 批次
    await rawGet("/relay/_reset");
    record("T53 honeypot", trapped && bannedOk,
      `trap=${trap.code}/warn=${trapped} chalAfter=${chalBanned.code}/banned=${bannedOk}`);
  } catch (e) {
    record("T53 honeypot", false, e.message);
  }

  // ── T54 多用户不串线：多手机共用一个 BIT + 跨网段/跨 rid/密钥轮换隔离 ──
  try {
    await call("/api/debug/config", { cloud_relay_url: `http://127.0.0.1:${RPORT}` });
    const rid54 = JSON.parse((await callGet("/api/qr")).body).rid;
    const BIND54 = workerBind(KEY, rid54);
    // 等 bind 送达（许可闸门生效，信号 = 中继侧 permit required 403）；20s 超时兜底
    const deadline54 = Date.now() + 20000;
    let gated54 = false;
    while (Date.now() < deadline54) {
      const probe = await rawReq(rid54, "/api/health", "GET", {
        Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD,
        ...signHeaders(KEY, MAT, rid54, "GET", "/api/health") });
      if (probe.code === 403 && /permit required/i.test(probe.body)) { gated54 = true; break; }
      await new Promise((r) => setTimeout(r, 1000));
    }
    if (!gated54) throw new Error("bind not delivered within 20s (permit gate inactive)");

    // 模拟第二位用户（另一台 BIT）：独立 rid + 独立握手密钥（其 poller 即本人）
    const rid54b = "e".repeat(32);
    const pollRid = (rid, bindHex) => new Promise((resolve, reject) => {
      const q = http.request({ host: BASE, port: RPORT, path: `/relay/poll/${rid}`, method: "POST",
        headers: { "x-bit-dev": "1234567890abcdef", "x-bit-bind": bindHex, "content-type": "application/json" }, timeout: 10000, agent },
        (r) => { let b = ""; r.on("data", (c) => (b += c)); r.on("end", () => resolve(b)); });
      q.on("error", reject); q.end("{}");
    });
    await pollRid(rid54b, "cd".repeat(32));

    // 手机端模拟器：ip 模拟真实公网来源（fake_relay 经 x-fake-client-ip 透传给 BIT 计数）
    const phone = (rid, ip, bindHex) => ({
      handshake: async () => {
        const c = await challenge(rid);
        const r = await rawReq(rid, "/api/health", "GET", {
          Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD, "x-fake-client-ip": ip,
          ...signHeaders(KEY, MAT, rid, "GET", "/api/health"),
          "x-bit-challenge": c, "x-bit-proof": proofOf(bindHex, c) });
        return { code: r.code, permit: r.headers?.["x-bit-permit"] || "" };
      },
      req: (path, method, permit, body) => {
        const b = body == null ? null : (typeof body === "string" ? body : JSON.stringify(body));
        return rawReq(rid, path, method, {
          Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD, "x-fake-client-ip": ip,
          ...(b ? { "content-type": "application/json" } : {}),
          ...signHeaders(KEY, MAT, rid, method, path), "x-bit-permit": permit }, b);
      },
    });
    const A = phone(rid54, "203.0.113.10", BIND54);
    const B = phone(rid54, "198.51.100.20", BIND54);

    // a) 两台手机各自握手：各持独立许可（同 rid 多许可并存）
    const hsA = await A.handshake();
    const hsB = await B.handshake();
    const twoPermits = hsA.code === 200 && hsB.code === 200
      && /^[0-9a-f]{32}$/.test(hsA.permit) && /^[0-9a-f]{32}$/.test(hsB.permit) && hsA.permit !== hsB.permit;

    // b) 并发对话：各响应只含自己的标记（内容不串线）
    const [chatA, chatB] = await Promise.all([
      A.req("/v1/chat/completions", "POST", hsA.permit, { model: "x", stream: false, messages: [{ role: "user", content: "E2E-CMD-ECHO T54-TAG-A" }] }),
      B.req("/v1/chat/completions", "POST", hsB.permit, { model: "x", stream: false, messages: [{ role: "user", content: "E2E-CMD-ECHO T54-TAG-B" }] }),
    ]);
    const noCross = chatA.code === 200 && chatB.code === 200
      && chatA.body.includes("T54-TAG-A") && !chatA.body.includes("T54-TAG-B")
      && chatB.body.includes("T54-TAG-B") && !chatB.body.includes("T54-TAG-A");

    // c) 许可跨网段盗用：B 拿 A 的许可（不同网络）→ 403；A 的许可不受影响
    const steal = await B.req("/api/health", "GET", hsA.permit);
    const aStill = await A.req("/api/health", "GET", hsA.permit);

    // d) 跨 rid 隔离：rid54 的挑战应答 / 许可拿到 rid54b（另一用户）→ 403
    const cX = await challenge(rid54);
    const acrossChal = await rawReq(rid54b, "/api/health", "GET", {
      Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD, "x-fake-client-ip": "203.0.113.10",
      ...signHeaders(KEY, MAT, rid54b, "GET", "/api/health"),
      "x-bit-challenge": cX, "x-bit-proof": proofOf(BIND54, cX) });
    const acrossPermit = await rawReq(rid54b, "/api/health", "GET", {
      Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD, "x-fake-client-ip": "203.0.113.10",
      ...signHeaders(KEY, MAT, rid54b, "GET", "/api/health"),
      "x-bit-permit": hsA.permit });

    // e) 密钥轮换即吊销：rid54b 换 bind 后旧许可立即失效
    // rid54b 无真实 BIT poller —— 以"第二台 BIT 模拟器"持续 poll 并代答 200，
    // 让握手请求走完完整回路（admission 签发许可 → poll 取走 → answer 回给手机端）
    const answerReq = (t) => new Promise((resolve, reject) => {
      const data = JSON.stringify({ rid: t.rid, s: 200, h: { "content-type": "application/json" },
        b: Buffer.from(JSON.stringify({ ok: true, via: "second-bit" })).toString("base64") });
      const q = http.request({ host: BASE, port: RPORT, path: `/relay/answer/${rid54b}`, method: "POST",
        headers: { "content-type": "application/json", "Content-Length": Buffer.byteLength(data) }, timeout: 10000, agent },
        (r) => { r.resume(); r.on("end", resolve); });
      q.on("error", reject); q.write(data); q.end();
    });
    let pollerBind = "cd".repeat(32);
    let pollerStop = false;
    const pollerLoop = (async () => {
      while (!pollerStop) {
        try {
          const b = await pollRid(rid54b, pollerBind);
          if (b) { try { await answerReq(JSON.parse(b)); } catch {} }
        } catch { await new Promise((r) => setTimeout(r, 200)); }
      }
    })();
    const C = phone(rid54b, "198.51.100.20", "cd".repeat(32));
    const hsC = await C.handshake();
    pollerBind = "ef".repeat(32);      // 密钥轮换：下一次 poll 携带新 bind → 清全部许可/挑战
    await pollRid(rid54b, pollerBind); // 同步送达新 bind（无排队请求 → 204，≤3s）
    pollerStop = true;
    await pollerLoop.catch(() => {});
    const afterRotate = await C.req("/api/health", "GET", hsC.permit);

    const rotated = hsC.code === 200 && /^[0-9a-f]{32}$/.test(hsC.permit) && afterRotate.code === 403;
    record("T54 multi-user-isolation",
      twoPermits && noCross && steal.code === 403 && aStill.code === 200
        && acrossChal.code === 403 && acrossPermit.code === 403 && rotated,
      `permits=${twoPermits} noCross=${noCross} steal=${steal.code}/exp403 aStill=${aStill.code}/exp200`
        + ` acrossChal=${acrossChal.code}/exp403 acrossPermit=${acrossPermit.code}/exp403`
        + ` hsC=${hsC.code}/permit=${/^[0-9a-f]{32}$/.test(hsC.permit)} afterRotate=${afterRotate.code}/exp403`);
  } catch (e) {
    record("T54 multi-user-isolation", false, e.message);
  } finally {
    try { await call("/api/debug/config", { cloud_relay_url: "" }); } catch {}
  }

  // ── T55 每用户带宽限速：中继响应按手机端 IP 令牌桶（relay_kbps_per_user，0=不限）──
  try {
    await call("/api/debug/config", { cloud_relay_url: `http://127.0.0.1:${RPORT}`, relay_kbps_per_user: 100 });
    const rid55 = JSON.parse((await callGet("/api/qr")).body).rid;
    const BIND55 = workerBind(KEY, rid55);
    // 等 bind 送达（信号 = 中继侧 permit required 403；tunnelReq 自带握手自愈）
    const deadline55 = Date.now() + 20000;
    let gated55 = false;
    while (Date.now() < deadline55) {
      const probe = await rawReq(rid55, "/api/health", "GET", {
        Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD,
        ...signHeaders(KEY, MAT, rid55, "GET", "/api/health") });
      if (probe.code === 403 && /permit required/i.test(probe.body)) { gated55 = true; break; }
      await new Promise((r) => setTimeout(r, 1000));
    }
    if (!gated55) throw new Error("bind not delivered within 20s (permit gate inactive)");

    // a) 限速生效：1MB 下行（E2E-CMD-LONG-1000）超过 2s 突发余量，理想等待 ≈ (1001KB-200KB)/100KB/s ≈ 8s
    const bigMsg = (n) => ({ model: "x", stream: false, messages: [{ role: "user", content: `E2E-CMD-LONG-${n}` }] });
    const t0 = Date.now();
    const big1P = tunnelReq(rid55, "/v1/chat/completions", "POST",
      { Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD, ...signHeaders(KEY, MAT, rid55, "POST", "/v1/chat/completions") }, bigMsg(1000));

    // b) 每用户独立预算 + 并发服务：big1 进入限速等待期间，另一 IP 用户的握手与小请求
    // 不被阻塞（隧道请求逐个并发回传，一个用户的限速等待不卡 poller）
    await new Promise((r) => setTimeout(r, 300));
    const cB = await challenge(rid55);
    const hsB = await rawReq(rid55, "/api/health", "GET", {
      Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD, "x-fake-client-ip": "198.51.100.99",
      ...signHeaders(KEY, MAT, rid55, "GET", "/api/health"),
      "x-bit-challenge": cB, "x-bit-proof": proofOf(BIND55, cB) });
    const tB = Date.now();
    const bSmall = await rawReq(rid55, "/api/health", "GET", {
      Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD, "x-fake-client-ip": "198.51.100.99",
      ...signHeaders(KEY, MAT, rid55, "GET", "/api/health"),
      "x-bit-permit": hsB.headers?.["x-bit-permit"] || "" });
    const dtB = Date.now() - tB;

    const big1 = await big1P;
    const dt1 = Date.now() - t0;
    const paced = big1.code === 200 && big1.body.includes("L0999") && dt1 >= 6000;

    // c) 解除限速（0=不限）：同样 1MB 秒回
    await call("/api/debug/config", { relay_kbps_per_user: 0 });
    const t2 = Date.now();
    const big2 = await tunnelReq(rid55, "/v1/chat/completions", "POST",
      { Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD, ...signHeaders(KEY, MAT, rid55, "POST", "/v1/chat/completions") }, bigMsg(1000));
    const dt2 = Date.now() - t2;

    record("T55 relay-user-bandwidth",
      paced && big2.code === 200 && big2.body.includes("L0999") && dt2 < 5000
        && hsB.code === 200 && bSmall.code === 200 && dtB < 3000,
      `paced=${paced} big1=${big1.code}/len=${big1.body.length}/dt1=${dt1}ms/exp≥6000`
        + ` big2=${big2.code}/len=${big2.body.length}/dt2=${dt2}ms/exp<5000`
        + ` otherUser=${bSmall.code}/dtB=${dtB}ms/exp<3000 hsB=${hsB.code}`);
  } catch (e) {
    record("T55 relay-user-bandwidth", false, e.message);
  } finally {
    try { await call("/api/debug/config", { cloud_relay_url: "", relay_kbps_per_user: 200 }); } catch {}
  }

  // ── T56 中继面最小化：chat-only 路径白名单 + 文本长度上限 + IP 变动签发闸门 ──
  try {
    await call("/api/debug/config", { cloud_relay_url: `http://127.0.0.1:${RPORT}`, relay_max_text_chars: 5000 });
    const rid56 = JSON.parse((await callGet("/api/qr")).body).rid;
    const BIND56 = workerBind(KEY, rid56);
    // 等 bind 送达（信号 = 中继侧 permit required 403）
    const deadline56 = Date.now() + 20000;
    let gated56 = false;
    while (Date.now() < deadline56) {
      const probe = await rawReq(rid56, "/api/health", "GET", {
        Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD,
        ...signHeaders(KEY, MAT, rid56, "GET", "/api/health") });
      if (probe.code === 403 && /permit required/i.test(probe.body)) { gated56 = true; break; }
      await new Promise((r) => setTimeout(r, 1000));
    }
    if (!gated56) throw new Error("bind not delivered within 20s (permit gate inactive)");

    // a) chat-only：全凭据 + 合法许可打非聊天路径 → BIT 403（签名与许可再合法也只有对话可用）
    const cOnly = await tunnelReq(rid56, "/api/tools", "GET", {
      Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD,
      ...signHeaders(KEY, MAT, rid56, "GET", "/api/tools") });
    const chatOnly = cOnly.code === 403 && /chat-only/i.test(cOnly.body);
    // b) 白名单内放行：/v1/models（手机端拉模型列表）经中继 200
    const mOk = await tunnelReq(rid56, "/v1/models", "GET", {
      Authorization: `Bearer ${KEY}`, ...signHeaders(KEY, MAT, rid56, "GET", "/v1/models") });
    // c) 文本上限：6000 字符输入超 5000 档 → 413（只算文本；经中继才受限）
    const longMsg = { model: "x", stream: false, messages: [{ role: "user", content: "E2E-PLAIN long " + "字".repeat(6000) }] };
    const over = await tunnelReq(rid56, "/v1/chat/completions", "POST",
      { Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD, ...signHeaders(KEY, MAT, rid56, "POST", "/v1/chat/completions") }, longMsg);
    // d) LAN 直连同体量输入不受限：200
    const direct = await call("/api/chat", { session_id: sid(56), message: "E2E-PLAIN long direct " + "字".repeat(6000) });

    // e) IP 变动闸门：连续新网段握手签发许可 → 撞到每小时新网段数上限（6）后 429；
    //    已签发许可不受影响（最后成功网段签发的许可，同网段复用仍 200）
    let churnOk = 0, churn429 = 0, still = null, lastGood = "", lastGoodIp = "";
    for (let i = 101; i <= 110 && churn429 === 0; i++) {
      const c = await challenge(rid56);
      const r = await rawReq(rid56, "/api/health", "GET", {
        Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD, "x-fake-client-ip": `198.51.${i}.1`,
        ...signHeaders(KEY, MAT, rid56, "GET", "/api/health"),
        "x-bit-challenge": c, "x-bit-proof": proofOf(BIND56, c) });
      if (r.code === 200 && r.headers?.["x-bit-permit"]) { churnOk++; lastGood = r.headers["x-bit-permit"]; lastGoodIp = `198.51.${i}.1`; }
      else if (r.code === 429) { churn429++; break; }
    }
    if (lastGood) {
      still = await rawReq(rid56, "/api/health", "GET", {
        Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD, "x-fake-client-ip": lastGoodIp,
        ...signHeaders(KEY, MAT, rid56, "GET", "/api/health"), "x-bit-permit": lastGood });
    }
    const churn = churnOk >= 1 && churn429 === 1 && (!lastGood || still.code === 200);

    record("T56 relay-chat-only",
      chatOnly && mOk.code === 200 && over.code === 413 && direct.code === 200 && churn,
      `chatOnly=${chatOnly}/${cOnly.code} models=${mOk.code} over=${over.code}/exp413`
        + ` direct=${direct.code}/exp200 churnOk=${churnOk} churn429=${churn429} still=${still?.code}/exp200`);
  } catch (e) {
    record("T56 relay-chat-only", false, e.message);
  } finally {
    try { await call("/api/debug/config", { cloud_relay_url: "", relay_max_text_chars: 65536 }); } catch {}
  }

  // ── T57 保活硬上限：许可时代（连续保活链）超期自动整体失效，重新握手恢复 ──
  // 用 3 秒时代窗口的专用 fake_relay（9805）模拟 7 天到点：旧许可拒绝续期 →
  // 只有持 S 重新握手才能开新时代（泄漏的令牌到点即死，合法设备无感自愈）
  try {
    const { spawn } = require("child_process");
    let eraErr = "";
    const eraChild = spawn(process.execPath, [require("path").join(__dirname, "fake_relay.cjs")],
      { env: { ...process.env, FAKE_RELAY_PORT: "9805", FAKE_PERMIT_ERA_MS: "3000" },
        stdio: ["ignore", "ignore", "pipe"] });
    eraChild.stderr.on("data", (c) => (eraErr += c));
    try {
      await call("/api/debug/config", { cloud_relay_url: "http://127.0.0.1:9805" });
      const rid57 = JSON.parse((await callGet("/api/qr")).body).rid;
      const BIND57 = workerBind(KEY, rid57);
      // 等子中继就绪 + bind 送达（信号 = 中继侧 permit required 403；探测不带握手材料，不触发签发）
      const ready57 = Date.now() + 20000;
      let gated57 = false;
      while (Date.now() < ready57) {
        const probe = await new Promise((resolve) => {
          const q = http.get({ host: BASE, port: 9805, path: `/relay/health`, timeout: 1500, agent },
            (r) => { r.resume(); resolve(r.statusCode === 200); });
          q.on("error", () => resolve(false)); q.on("timeout", () => { q.destroy(); resolve(false); });
        });
        if (!probe) { await new Promise((r) => setTimeout(r, 300)); continue; }
        const p = await rawReq(rid57, "/api/health", "GET", {
          Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD,
          ...signHeaders(KEY, MAT, rid57, "GET", "/api/health") }, null, 9805);
        if (p.code === 403 && /permit required/i.test(p.body)) { gated57 = true; break; }
        await new Promise((r) => setTimeout(r, 500));
      }
      if (!gated57) throw new Error(`era relay not ready / bind not delivered within 20s (child exit=${eraChild.exitCode} stderr=${eraErr.slice(0, 120) || "-"})`);

      // 时代内：握手签发 P1 → 许可复用放行
      const c1 = await challenge(rid57, 9805);
      const hs1 = await rawReq(rid57, "/api/health", "GET", {
        Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD,
        ...signHeaders(KEY, MAT, rid57, "GET", "/api/health"),
        "x-bit-challenge": c1, "x-bit-proof": proofOf(BIND57, c1) }, null, 9805);
      const P1 = hs1.headers?.["x-bit-permit"] || "";
      const inEra = await rawReq(rid57, "/api/health", "GET", {
        Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD,
        ...signHeaders(KEY, MAT, rid57, "GET", "/api/health"), "x-bit-permit": P1 }, null, 9805);
      // 时代到点（3s）：旧许可续期被拒——保活不能永生
      await new Promise((r) => setTimeout(r, 3200));
      const afterEra = await rawReq(rid57, "/api/health", "GET", {
        Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD,
        ...signHeaders(KEY, MAT, rid57, "GET", "/api/health"), "x-bit-permit": P1 }, null, 9805);
      // 自愈：重新握手（需 S）开启新时代 → 新许可放行且与旧许可不同
      const c2 = await challenge(rid57, 9805);
      const hs2 = await rawReq(rid57, "/api/health", "GET", {
        Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD,
        ...signHeaders(KEY, MAT, rid57, "GET", "/api/health"),
        "x-bit-challenge": c2, "x-bit-proof": proofOf(BIND57, c2) }, null, 9805);
      const P2 = hs2.headers?.["x-bit-permit"] || "";
      const healed = await rawReq(rid57, "/api/health", "GET", {
        Authorization: `Bearer ${KEY}`, "X-Access-Password": PASSWORD,
        ...signHeaders(KEY, MAT, rid57, "GET", "/api/health"), "x-bit-permit": P2 }, null, 9805);

      record("T57 keepalive-era-expiry",
        hs1.code === 200 && inEra.code === 200 && afterEra.code === 403
          && hs2.code === 200 && P2 !== P1 && healed.code === 200,
        `hs1=${hs1.code} inEra=${inEra.code}/exp200 afterEra=${afterEra.code}/exp403`
          + ` hs2=${hs2.code} rotated=${P2 !== P1} healed=${healed.code}/exp200`);
    } finally {
      try { await call("/api/debug/config", { cloud_relay_url: "" }); } catch {}
      eraChild.kill("SIGKILL");
    }
  } catch (e) {
    record("T57 keepalive-era-expiry", false, e.message);
  }

  // ── T59-T64 多协议原生工具调用（标准协议） + 兼容模式（文本约定） ──
  // /api/debug/config 钩子运行时切换激活提供方与全局兼容模式，协议族由兼容模式全局开关决定：
  //   compat_mode=false = 标准原生 function calling（请求带 tools，各家原生格式解析）；
  //   compat_mode=true  = 文本约定（提示词注入 JSON 契约、识别正文单行 JSON 数组工具调用）
  try {
    // T59 OpenAI 原生 tool_calls 流式（标准 index 形态）：delta 增量聚合 → 执行 → role=tool 反馈 → 最终
    await call("/api/debug/config", { active_provider: "e2e-mock-openai-native", compat_mode: false });
    const r59 = await chat(sid(59), "E2E-NAT-OPENAI go");
    record("T59 openai-native-toolcalls", /E2E-FINAL-OK stdout=「.*e2e-native-openai-ok」/s.test(r59.reply || ""), `reply=${(r59.reply || "").slice(0, 120)}`);
  } catch (e) { record("T59 openai-native-toolcalls", false, e.message); }

  try {
    // T60 Claude 原生 tool_use 流式：content_block_start(tool_use) + input_json_delta 增量 → 桥接执行
    // → 下一轮 tool_result 块回传 → 最终回复（mock 确认收到 tool_result，标记 tool_result=true）
    await call("/api/debug/config", { active_provider: "e2e-mock-claude", compat_mode: false });
    const r60 = await chat(sid(60), "E2E-CLAUDE-NAT go");
    record("T60 claude-native-tooluse", /E2E-FINAL-CLAUDE-NAT stdout=「e2e-claude-native-ok」 tool_result=true/.test(r60.reply || ""), `reply=${(r60.reply || "").slice(0, 140)}`);
  } catch (e) { record("T60 claude-native-tooluse", false, e.message); }

  try {
    // T61 Gemini 原生 functionCall 流式：独立 functionCall part → 桥接执行 → 下一轮 functionResponse 回传
    await call("/api/debug/config", { active_provider: "e2e-mock-gemini", compat_mode: false });
    const r61 = await chat(sid(61), "E2E-GEMINI-NAT go");
    record("T61 gemini-native-functioncall", /E2E-FINAL-GEMINI-NAT stdout=「e2e-gemini-native-ok」 functionResponse=true/.test(r61.reply || ""), `reply=${(r61.reply || "").slice(0, 140)}`);
  } catch (e) { record("T61 gemini-native-functioncall", false, e.message); }

  try {
    // T62 兼容模式（文本约定）：开启后不再发送 tools 参数，从首轮就注入 JSON 契约走文本协议
    // → 正文单行 JSON 数组被识别执行；claude 协议同理（/v1/messages 纯文本流）
    await call("/api/debug/config", { active_provider: "e2e-mock-claude", compat_mode: true });
    const r62 = await chat(sid(62), "E2E-CLAUDE-DEGRADE go");
    const ok62 = /E2E-FINAL-CLAUDE-DEGRADE stdout=「e2e-claude-degrade-ok」/.test(r62.reply || "");
    record("T62 compat-mode-text-protocol", ok62, `reply=${(r62.reply || "").slice(0, 110)}`);
  } catch (e) { record("T62 compat-mode-text-protocol", false, e.message); }

  try {
    // T63 标准协议不自动降级：兼容模式关闭时端点拒绝 tools → 明确报错指路「兼容模式」，不做探测重试
    await call("/api/debug/config", { active_provider: "e2e-mock-strict", compat_mode: false });
    let body63 = "";
    try { await chat(sid(63), "E2E-NAT-STRICT go"); } catch (e) { body63 = e.message; }
    // BIT 的业务错误通过 HTTP body 返回（code=200 但 body 含 error 字段），
    // chat 函数会 JSON.parse 它并返回含 error 键的对象；此处同时覆盖异常与 body-error 两种形态
    if (!body63) {
      try {
        const r63 = await call("/api/chat", { session_id: sid(63) + "-b", message: "E2E-NAT-STRICT go" });
        body63 = r63.body;
      } catch (e) { body63 = e.message; }
    }
    record("T63 no-auto-degrade-error", /原生工具调用/.test(body63) && /兼容模式/.test(body63), `body=${body63.slice(0, 200)}`);
  } catch (e) { record("T63 no-auto-degrade-error", false, e.message); }

  try {
    // T64 协议族互不串扰：兼容模式关回原生后，新会话直接走原生普通对话正常
    await call("/api/debug/config", { active_provider: "e2e-mock-openai-native", compat_mode: false });
    const r64 = await chat(sid(64), "E2E-PLAIN after mode switch");
    record("T64 native-after-compat-off", /E2E-FINAL-PLAIN/.test(r64.reply || ""), `reply=${(r64.reply || "").slice(0, 100)}`);
  } catch (e) { record("T64 native-after-compat-off", false, e.message); }

  // ── T58 守护复活：kill -9 主进程 → 守护进程接力拉起（签名握手，删除/篡改不能解除布防）──
  // 放在最后：复活后实例继续存活，但接力事件转存审计在下次主进程启动才发生
  try {
    const st = JSON.parse((await callGet("/api/debug/state")).body);
    const oldPid = st.pid;
    if (!oldPid) throw new Error("debug/state missing pid");
    // 守护未启用时（macOS 默认关闭 guardian；见 guardian::enabled）不做 kill 复活自测：
    // 此时强杀只会误杀被测实例且无人拉起，后续一切请求全挂——表现为“跑完全量服务消失”。
    if (!st.guardian_enabled) {
      record("T58 guardian-revive", true, "skip（guardian_enabled=false，本环境不执行 kill/复活自测）");
    } else {
      // 平台感知强杀：只杀主进程本体——守护进程是主进程子进程，Windows 绝不能带 /T（会把守护一起杀掉）
      if (process.platform === "win32") {
        require("child_process").execSync(`taskkill /F /PID ${oldPid}`);
      } else {
        try { process.kill(oldPid, "SIGKILL"); } catch (e) { if (e.code !== "ESRCH") throw e; }
      }
      // 守护 tick 2s + 应用启动：/api/health 免鉴权，恢复 200 即复活
      const deadline = Date.now() + 25_000;
      let back = false;
      while (Date.now() < deadline) {
        try { if ((await callGet("/api/health")).code === 200) { back = true; break; } } catch {}
        await new Promise((r) => setTimeout(r, 800));
      }
      // 复活确证：pid 已更换（防 kill 未生效时 health 仍 200 的假阳性）
      let newPid = 0;
      for (let i = 0; i < 20 && !newPid; i++) {
        try { newPid = JSON.parse((await callGet("/api/debug/state")).body).pid || 0; } catch {}
        if (!newPid) await new Promise((r) => setTimeout(r, 800));
      }
      record("T58 guardian-revive", back && newPid > 0 && newPid !== oldPid,
        `health=${back} oldPid=${oldPid} newPid=${newPid}`);
    }
  } catch (e) {
    record("T58 guardian-revive", false, e.message);
  }

  const pass = results.filter((x) => x.ok).length;
  console.log(`\n==== ${pass}/${results.length} passed ====`);
  process.exit(pass === results.length ? 0 : 1);
})();
