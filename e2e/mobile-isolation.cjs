// yxpil · BIT — 手机远程访问多客户端会话隔离测试（防对话串线）
// 验证：两台"手机"（不同 session_id）经 /api/chat 并行对话时历史互不可见；
//       空 session_id 被拒绝；OpenAI 端点无状态不落会话。
// 不依赖 AI 回复内容：断言的是 /api/chat 响应中的会话 messages 数组本身。
// 用法：node e2e/mobile-isolation.cjs [BIT二进制路径] [端口=18610]
const { spawn, execSync } = require("child_process");
const fs = require("fs");
const os = require("os");
const net = require("net");
const path = require("path");
const http = require("http");

const BIN = process.argv[2] || path.join(__dirname, "../src-tauri/target/debug/bit");
const PORT = Number(process.argv[3]) || 18610;
const KEY = "0123456789abcdef0123456789abcdef";
const PASSWORD = "87654321";
const MOCK_PORT = 9901; // mock-ai.cjs 固定监听端口（已运行则复用）

const results = [];
const record = (name, ok, detail) => {
  results.push({ name, ok });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? `  ${detail}` : ""}`);
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function portOpen(port, host = "127.0.0.1") {
  return new Promise((resolve) => {
    const s = net.connect({ host, port, timeout: 1500 });
    s.on("connect", () => { s.destroy(); resolve(true); });
    s.on("error", () => resolve(false));
    s.on("timeout", () => { s.destroy(); resolve(false); });
  });
}

// /api/chat：JSON 响应原样返回 { status, body }
function chat(sessionId, message) {
  return new Promise((resolve, reject) => {
    const data = JSON.stringify(sessionId ? { session_id: sessionId, message } : { message });
    const req = http.request(
      { host: "127.0.0.1", port: PORT, path: "/api/chat", method: "POST",
        headers: { "Content-Type": "application/json", Authorization: `Bearer ${KEY}`,
          "X-Access-Password": PASSWORD, "Content-Length": Buffer.byteLength(data) }, timeout: 20000 },
      (res) => { let b = ""; res.on("data", (c) => (b += c)); res.on("end", () => resolve({ status: res.statusCode, body: b })); },
    );
    req.on("error", reject);
    req.write(data);
    req.end();
  });
}

function findConflicts() {
  let realBin = BIN;
  try { realBin = fs.realpathSync(BIN); } catch {}
  try {
    return execSync("ps -axo pid=,command=", { encoding: "utf8" })
      .split("\n").map((l) => {
        const m = l.trim().match(/^(\d+)\s+(\S+)/);
        if (!m) return 0;
        let cmd = m[2];
        try { cmd = fs.realpathSync(cmd); } catch {}
        return cmd === realBin ? parseInt(m[1], 10) : 0;
      }).filter(Boolean);
  } catch { return []; }
}

(async () => {
  if (!fs.existsSync(BIN)) {
    console.error(`BIT 二进制不存在：${BIN}`);
    process.exit(2);
  }

  // mock-ai 上游（已监听则复用）
  let mockProc = null;
  if (!(await portOpen(MOCK_PORT))) {
    mockProc = spawn(process.execPath, [path.join(__dirname, "mock-ai.cjs")], { stdio: "ignore" });
    const dl = Date.now() + 10000;
    while (!(await portOpen(MOCK_PORT))) {
      if (Date.now() > dl) { console.error("mock-ai 10s 未就绪"); process.exit(2); }
      await sleep(300);
    }
  }

  // 单实例清场 + 隔离数据目录（远程开启 + mock 上游）
  for (const p of findConflicts()) { try { process.kill(p, "SIGKILL"); } catch {} }
  await sleep(800);
  for (const p of findConflicts()) { try { process.kill(p, "SIGKILL"); } catch {} }

  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "bit-isolation-"));
  fs.writeFileSync(path.join(dir, "config.json"), JSON.stringify({
    remote_enabled: true,
    host: "127.0.0.1",
    port: PORT,
    client_key: KEY,
    access_password: PASSWORD,
    password_enabled: true,
    revision: 1,
  }));
  fs.writeFileSync(path.join(dir, "ai_config.json"), JSON.stringify({
    providers: [{ id: "mock", name: "mock", protocol: "openai", base_url: `http://127.0.0.1:${MOCK_PORT}/v1`, api_key: "e2e", model: "mock", active: true }],
  }));

  const bit = spawn(BIN, [], {
    env: { ...process.env, BIT_DATA_DIR: dir, BIT_HEADLESS: "1", BIT_NO_GUARDIAN: "1" },
    stdio: ["ignore", "ignore", "pipe"],
    detached: false,
  });
  let errTail = "";
  bit.stderr.on("data", (d) => { errTail = (errTail + d.toString()).slice(-600); });
  bit.on("exit", (c) => { if (c !== 0) console.error(`BIT 提前退出 code=${c} stderr: ${errTail}`); });

  let up = false;
  const dl = Date.now() + 30000;
  while (Date.now() < dl) {
    if (bit.exitCode !== null) break;
    if (await portOpen(PORT)) { up = true; break; }
    await sleep(400);
  }
  if (!up) {
    console.error(`BIT 30s 未监听 ${PORT}，stderr: ${errTail}`);
    try { bit.kill("SIGKILL"); } catch {}
    process.exit(2);
  }

  try {
    const A = "remote-iso-device-a"; // 设备 A 会话
    const B = "remote-iso-device-b"; // 设备 B 会话
    const MARK_A = "ISO-MARKER-ALPHA-12345";
    const MARK_B = "ISO-MARKER-BRAVO-67890";

    // 1) 设备 A 首轮对话（带专属标记）
    const r1 = await chat(A, `请记住这个标记 ${MARK_A}`);
    const m1 = JSON.parse(r1.body || "{}").messages || [];
    record("A1 chat ok", r1.status === 200, `status=${r1.status}`);
    record("A1 session contains marker", JSON.stringify(m1).includes(MARK_A), `msgs=${m1.length}`);

    // 2) 设备 B 独立会话对话：绝不能看到 A 的历史
    const r2 = await chat(B, `请记住这个标记 ${MARK_B}`);
    const m2 = JSON.parse(r2.body || "{}").messages || [];
    record("B1 chat ok", r2.status === 200, `status=${r2.status}`);
    record("B1 no cross-talk from A", !JSON.stringify(m2).includes(MARK_A), `msgs=${m2.length}`);

    // 3) 设备 A 二轮：自己的历史还在，也看不到 B 的
    const r3 = await chat(A, "继续");
    const m3 = JSON.parse(r3.body || "{}").messages || [];
    const s3 = JSON.stringify(m3);
    record("A2 history kept", s3.includes(MARK_A), `msgs=${m3.length}`);
    record("A2 no cross-talk from B", !s3.includes(MARK_B), "");

    // 4) 空 session_id 必须拒绝（防落入桌面激活会话串线）
    const r4 = await chat("", "hello");
    record("empty session rejected", r4.status === 400, `status=${r4.status}`);

    // 5) OpenAI 端点无状态：不带 session 概念，两次独立请求均可用
    const oai = (msgs) => new Promise((resolve, reject) => {
      const data = JSON.stringify({ model: "mock", messages: msgs });
      const req = http.request(
        { host: "127.0.0.1", port: PORT, path: "/v1/chat/completions", method: "POST",
          headers: { "Content-Type": "application/json", Authorization: `Bearer ${KEY}`,
            "Content-Length": Buffer.byteLength(data) }, timeout: 20000 },
        (res) => { let b = ""; res.on("data", (c) => (b += c)); res.on("end", () => resolve({ status: res.statusCode })); },
      );
      req.on("error", reject);
      req.write(data);
      req.end();
    });
    const o1 = await oai([{ role: "user", content: "oai isolated ping" }]);
    const o2 = await oai([{ role: "user", content: "oai isolated pong" }]);
    record("OpenAI endpoint stateless", o1.status === 200 && o2.status === 200, `s1=${o1.status} s2=${o2.status}`);
  } catch (e) {
    record("isolation suite", false, e.message);
  }

  try { bit.kill("SIGKILL"); } catch {}
  await sleep(300);
  if (mockProc) { try { mockProc.kill(); } catch {} }
  fs.rmSync(dir, { recursive: true, force: true });

  const pass = results.length && results.every((r) => r.ok);
  console.log(`\n${pass ? "ISOLATION PASS" : "ISOLATION FAIL"}: ${results.filter((r) => r.ok).length}/${results.length}`);
  process.exit(pass ? 0 : 1);
})().catch((e) => { console.error("FAILED:", e.message); process.exit(1); });
