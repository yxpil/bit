// E2E 驱动脚本：通过 BIT 远程 API 驱动完整 AI 链路（mock 上游），逐场景断言
// 默认从 BIT 数据目录 config.json 读取 client_key / access_password，可用 E2E_PORT / E2E_KEY / E2E_PASSWORD 覆盖
const http = require("http");
const fs = require("fs");
const os = require("os");

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

  // T6 plan 待办沉淀
  try {
    const r = await chat(sid(6), "E2E-CMD-PLAN todo");
    record("T6 tool-plan", /E2E-FINAL-OK/.test(r.reply || ""), `reply=${(r.reply || "").slice(0, 80)}`);
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
      const okSanitized = !!v && !("data_url" in (v.result || {}));
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

  const pass = results.filter((x) => x.ok).length;
  console.log(`\n==== ${pass}/${results.length} passed ====`);
  process.exit(pass === results.length ? 0 : 1);
})();
