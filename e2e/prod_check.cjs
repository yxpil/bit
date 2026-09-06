// yxpil · BIT — 生产 Worker 真机全链路验证（osbt.space ↔ 本地 BIT）
// 含连接许可（permit）协议：动态握手密钥 + 挑战-应答，App 正确沟通才允许连接
// 用法：node e2e/prod_check.cjs [bitPort=8600] [bitKey] [bitPwd]
const http = require("http");
const https = require("https");
const { signHeaders, deviceMaterial, workerBind, proofOf } = require("./sign.cjs");
const cfg = require("os").homedir() + "/Library/Application Support/com.bit.hub/config.json";

const HOST = "osbt.space";
const PORT = Number(process.argv[2]) || 8600;
let KEY = process.argv[3] || "";
let PWD = process.argv[4] || "";
if (!KEY) { try { const c = JSON.parse(require("fs").readFileSync(cfg, "utf8")); KEY = c.client_key; PWD = c.access_password; } catch {} }

const call = (path, body) =>
  new Promise((resolve, reject) => {
    const data = body ? JSON.stringify(body) : null;
    const req = http.request(
      { host: "127.0.0.1", port: PORT, path, method: "POST",
        headers: { "Content-Type": "application/json", Authorization: `Bearer ${KEY}`, "X-Access-Password": PWD,
          "Content-Length": data ? Buffer.byteLength(data) : 0 }, timeout: 15000 },
      (res) => { let b = ""; res.on("data", (c) => (b += c)); res.on("end", () => resolve({ code: res.statusCode, body: b })); });
    req.on("error", reject);
    if (data) req.write(data);
    req.end();
  });

const tunnel = (rid, path, method, extra, body, raw) =>
  new Promise((resolve, reject) => {
    const req = https.request(
      { host: HOST, path: `/relay/req/${rid}${path}`, method,
        headers: { ...extra, ...(body && !raw ? { "Content-Type": "application/json" } : {}) }, timeout: 30000 },
      (res) => {
        let b = ""; const chunks = [];
        res.on("data", (c) => { b += c; chunks.push(Date.now()); });
        res.on("end", () => resolve({ code: res.statusCode, body: b, ct: res.headers["content-type"],
          chunks: chunks.length, permit: res.headers["x-bit-permit"] || "" }));
      });
    req.on("error", reject);
    req.on("timeout", () => { req.destroy(); reject(new Error("timeout")); });
    if (body) req.write(raw ? body : JSON.stringify(body));
    req.end();
  });

// 连接许可协议：挑战获取 + 应答计算（与 relay-worker/src/index.js 头部注释同口径）
const challenge = (rid) =>
  new Promise((resolve, reject) => {
    const q = https.get({ host: HOST, path: `/relay/challenge/${rid}`, timeout: 15000 },
      (r) => { let b = ""; r.on("data", (c) => (b += c)); r.on("end", () => { try { resolve(JSON.parse(b).c); } catch (e) { reject(e); } }); });
    q.on("error", reject); q.on("timeout", () => { q.destroy(); reject(new Error("challenge timeout")); });
  });

const permitCache = new Map(); // rid → 许可 token（15 分钟滑动 TTL，复用避免重复握手）
const tunnelReq = async (rid, path, method, headers, body, raw) => {
  const h = { ...headers };
  if (permitCache.has(rid)) h["x-bit-permit"] = permitCache.get(rid);
  let r = await tunnel(rid, path, method, h, body, raw);
  if (r.permit) permitCache.set(rid, r.permit);
  if (r.code === 403 && /permit required/i.test(r.body)) {
    permitCache.delete(rid);
    const c = await challenge(rid);
    const retry = { ...headers, "x-bit-challenge": c, "x-bit-proof": proofOf(workerBind(KEY, rid), c) };
    r = await tunnel(rid, path, method, retry, body, raw);
    if (r.permit) permitCache.set(rid, r.permit);
  }
  return r;
};

// 等 BIT 至少完成一次 poll（bind 已送达 Worker、许可门槛生效），避免竞态误判
const waitBind = async (rid) => {
  const deadline = Date.now() + 30000;
  while (Date.now() < deadline) {
    const st = await new Promise((resolve, reject) => {
      const q = https.get({ host: HOST, path: `/relay/status/${rid}`, timeout: 15000 },
        (r) => { let b = ""; r.on("data", (c) => (b += c)); r.on("end", () => { try { resolve(JSON.parse(b)); } catch (e) { resolve({}); } }); });
      q.on("error", reject); q.on("timeout", () => { q.destroy(); reject(new Error("status timeout")); });
    });
    if (st.online) { await new Promise((r) => setTimeout(r, 1500)); return st; }
    await new Promise((r) => setTimeout(r, 1000));
  }
  throw new Error("relay not online within 30s");
};

(async () => {
  // 0) 设备签名材料（bitsign-v2）：从被测实例 config.json 读 device_key 派生
  const st = JSON.parse((await new Promise((resolve, reject) => {
    const q = http.get({ host: "127.0.0.1", port: PORT, path: "/api/debug/state",
      headers: { Authorization: `Bearer ${KEY}`, "X-Access-Password": PWD }, timeout: 15000 },
      (r) => { let b = ""; r.on("data", (c) => (b += c)); r.on("end", () => resolve({ code: r.statusCode, body: b })); });
    q.on("error", reject);
  })).body || "{}");
  const dcfg = JSON.parse(require("fs").readFileSync(String(st.data_dir) + "/config.json", "utf8"));
  const MAT = deviceMaterial(dcfg.device_key || "");

  // 1) 配置真实中继入口 + 取识别码
  await call("/api/debug/config", { cloud_relay_url: "https://osbt.space" });
  const qr = JSON.parse((await callGet("/api/qr")).body);
  const rid = qr.rid;
  console.log("rid:", rid, " relay:", qr.methods?.relay);

  // 2) 等 BIT 首次 poll 把握手密钥送达 Worker（此后许可门槛生效）
  const stt = await waitBind(rid);
  console.log("relay online, dev:", stt.dev || "(n/a)");

  // 3) 负例一：无签名 → 网站层直接 403
  const noSign = await tunnel(rid, "/api/health", "GET", { Authorization: `Bearer ${KEY}`, "X-Access-Password": PWD });
  console.log("no-sign      :", noSign.code, noSign.body.slice(0, 60));

  // 4) 负例二：签名格式合法但无许可/无应答 → 网站层 403（permit required）
  const noPermit = await tunnel(rid, "/api/health", "GET", {
    Authorization: `Bearer ${KEY}`, "X-Access-Password": PWD, ...signHeaders(KEY, MAT, rid, "GET", "/api/health"),
  });
  console.log("no-permit    :", noPermit.code, noPermit.body.slice(0, 60));

  // 5) 正例：签名 + 首次握手（挑战应答）→ 签发许可 → 达本地 API
  const hs = await tunnelReq(rid, "/api/health", "GET", {
    Authorization: `Bearer ${KEY}`, "X-Access-Password": PWD, ...signHeaders(KEY, MAT, rid, "GET", "/api/health"),
  });
  console.log("handshake    :", hs.code, hs.body.slice(0, 60), " permit:", hs.permit ? "minted" : "none");

  // 6) 持许可复用：不再握手，直接 200
  const reuse = await tunnelReq(rid, "/api/health", "GET", {
    Authorization: `Bearer ${KEY}`, "X-Access-Password": PWD, ...signHeaders(KEY, MAT, rid, "GET", "/api/health"),
  });
  console.log("permit reuse :", reuse.code, reuse.body.slice(0, 60));

  // 7) OpenAI 格式非流式对话（经中继）
  const chat = await tunnelReq(rid, "/v1/chat/completions", "POST", {
    Authorization: `Bearer ${KEY}`, "X-Access-Password": PWD, ...signHeaders(KEY, MAT, rid, "POST", "/v1/chat/completions"),
  }, { model: "x", stream: false, messages: [{ role: "user", content: "E2E-PLAIN prod" }] });
  console.log("chat non-stream:", chat.code, chat.body.slice(0, 80));

  // 8) OpenAI 格式流式对话：SSE 增量到达（chunks>1 即证明逐块推送）
  const t0 = Date.now();
  const stream = await tunnelReq(rid, "/v1/chat/completions", "POST", {
    Authorization: `Bearer ${KEY}`, "X-Access-Password": PWD, ...signHeaders(KEY, MAT, rid, "POST", "/v1/chat/completions"),
  }, { model: "x", stream: true, messages: [{ role: "user", content: "E2E-PLAIN prod stream" }] });
  console.log("chat stream  :", stream.code, "ct=", stream.ct, "chunks=", stream.chunks, "dur=", Date.now() - t0, "ms");
  console.log("  body head  :", JSON.stringify(stream.body.slice(0, 120)));

  // 9) 保活心跳（长连接场景）：持许可 → 200 + online（不进隧道，不耗配额，许可续期）
  const ka = await new Promise((resolve, reject) => {
    const q = https.get({ host: HOST, path: `/relay/ka/${rid}`, timeout: 15000, headers: {
      ...signHeaders(KEY, MAT, rid, "GET", "/relay/ka"), "x-bit-permit": permitCache.get(rid) || "",
    } }, (r) => { let b = ""; r.on("data", (c) => (b += c)); r.on("end", () => resolve({ code: r.statusCode, body: b, permit: r.headers["x-bit-permit"] || "" })); });
    q.on("error", reject); q.on("timeout", () => { q.destroy(); reject(new Error("ka timeout")); });
  });
  let kaOk = null; try { kaOk = JSON.parse(ka.body); } catch {}
  console.log("keepalive    :", ka.code, "ok=", kaOk?.ok, "online=", kaOk?.online, "permitRenewed=", /^[0-9a-f]{32}$/.test(ka.permit));

  // 10) 可选蜜罐验证（PROD_HONEYPOT=1 时执行）：触碰诱捕路径 → 403 警告 + 立即封禁。
  //     注意：会把本机出口 IP 在生产 Worker 上封 30 分钟（isolate 内存 best-effort），
  //     影响后续 30 分钟内的中继测试，故默认关闭、放在所有测试之后
  let hp = null;
  if (process.env.PROD_HONEYPOT === "1") {
    hp = await new Promise((resolve, reject) => {
      const q = https.get({ host: HOST, path: "/relay/admin", timeout: 15000 },
        (r) => { let b = ""; r.on("data", (c) => (b += c)); r.on("end", () => resolve({ code: r.statusCode, body: b })); });
      q.on("error", reject); q.on("timeout", () => { q.destroy(); reject(new Error("honeypot timeout")); });
    });
    console.log("honeypot     :", hp.code, /honeypot|don't bother/i.test(hp.body) ? "warning ok (IP banned 30min)" : "NO WARNING");
  }

  // 11) 清理：关掉真实中继（避免实例一直挂在外网上）
  await call("/api/debug/config", { cloud_relay_url: "" });
  console.log("relay url cleared");

  const pass = noSign.code === 403 && noPermit.code === 403 && /permit required/i.test(noPermit.body)
    && hs.code === 200 && reuse.code === 200 && chat.code === 200 && stream.code === 200 && stream.chunks > 1
    && ka.code === 200 && kaOk?.ok === true && kaOk?.online === true
    && (hp === null || (hp.code === 403 && /honeypot|don't bother/i.test(hp.body)));
  console.log(pass ? "PROD CHECK PASS" : "PROD CHECK FAIL");
  if (!pass) process.exitCode = 1;

  function callGet(path) {
    return new Promise((resolve, reject) => {
      const q = http.get({ host: "127.0.0.1", port: PORT, path,
        headers: { Authorization: `Bearer ${KEY}`, "X-Access-Password": PWD }, timeout: 15000 },
        (r) => { let b = ""; r.on("data", (c) => (b += c)); r.on("end", () => resolve({ code: r.statusCode, body: b })); });
      q.on("error", reject);
    });
  }
})().catch((e) => { console.error("FAILED:", e.message); process.exit(1); });
