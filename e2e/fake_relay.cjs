// e2e/fake_relay.cjs — 模拟 Cloudflare 中继 Worker（bit-relay）的长轮询隧道协议
// 与 relay-worker/src/index.js 语义一致，便于本地 E2E 无外网验证：
//   POST /relay/poll/{rid}    应用端长轮询（最多持 3s，测试用短保持）→ 隧道请求 JSON 或 204
//                             必须携带 X-BIT-Dev（设备指纹哈希 16hex）：首次 poll 的设备
//                             绑定该 rid，其它设备 403；单设备绑定 rid 数上限 429
//   ANY  /relay/req/{rid}/…   手机端隧道请求 → 等应用端回传（最多 15s）→ 原样响应
//   GET  /relay/ka/{rid}      手机端保活心跳：许可续期 + BIT 在线状态（不进隧道不耗配额；
//                             签名路径约定 "/relay/ka"，与 Worker 同语义）
//   POST /relay/answer/{rid}  应用端回传 { rid, s, h, b }（一次性）或 { rid, seq, last, b }（流式分块）
//   GET  /relay/status/{rid}  在线状态 + 绑定设备指纹前 8 hex（与 Worker 语义一致）
//   GET  /relay/health        健康检查
// 蜜罐（与 Worker 一致）：诱捕路径（/relay/admin 等）触碰即封禁 30 分钟并回警告
// 信道防护（bitsign-v2，与 Worker 一致）：/relay/req 必须携带合法格式的
//   X-BIT-Sign / X-BIT-Ts / X-BIT-Nonce，缺失/格式非法 403 并计入坏签名；
//   单 IP 窗口内坏签名达 12 次 → 封禁 30 分钟（密码学验证在 BIT 端）
// 流式（与 Worker 一致）：请求体含 "stream":true 或 Accept: text/event-stream →
//   TransformStream 增量响应，BIT 逐块 answer
const http = require("http");
const { Readable } = require("stream");
const PORT = Number(process.env.FAKE_RELAY_PORT) || 9802;

const SIGN_RE = /^[0-9a-f]{64}$/;
const TS_RE = /^\d{1,13}$/;
const NONCE_RE = /^[0-9a-zA-Z]{8,128}$/;
const DEV_RE = /^[0-9a-f]{16}$/;
const DEV_RID_MAX = 8; // 与 Worker 一致：单设备识别码上限
const BODY_MAX = 4 * 1024 * 1024; // 与 Worker 一致：文字信道体上限（JSON 内 base64 小图仍可用）
// 连接许可协议常量（与 Worker 一致，协议见 relay-worker/src/index.js 头部注释）
const BIND_RE = /^[0-9a-f]{64}$/;
const PROOF_RE = /^[0-9a-f]{64}$/;
const CHALLENGE_TTL_MS = 60_000;
const PERMIT_TTL_MS = 15 * 60_000;
const PERMIT_MAX = 8;
// 许可签发滚动窗口（与 Worker 一致，防 IP 变动滥用）
const MINT_WINDOW_MS = 60 * 60_000;
const MINT_MAX_H = 24; // 单 rid 每小时签发总量
const MINT_IPS_H = 6; // 单 rid 每小时不同 IP 段数
// 保活硬上限（与 Worker 一致）：许可时代超过 7 天整体失效。FAKE_PERMIT_ERA_MS 可缩短供测试
const PERMIT_ERA_MS = Number(process.env.FAKE_PERMIT_ERA_MS) || 7 * 24 * 60 * 60_000;
const BADSIGN_WINDOW_MS = 10 * 60_000;
const BADSIGN_MAX = 12;
const BAN_MS = 30 * 60_000;
// 蜜罐诱捕路径（与 Worker HONEYPOT 一致）：正常客户端永不请求，触碰即封禁
const HONEYPOT = new Set([
  "admin", "login", "signin", "wp-login.php", "wp-admin", "administrator",
  ".env", ".git", "config", "configuration", "setup", "install", "install.php",
  "phpmyadmin", "pma", "console", "shell", "terminal", "actuator", "debug",
  "flag", "secret", "backup", "db.sql", "database", "eval", "telescope",
]);

const boxes = new Map(); // rid → { inbox, waiters, answers, reqTimes, dev }
const box = (rid) => {
  if (!boxes.has(rid)) boxes.set(rid, { inbox: [], waiters: [], answers: new Map(), reqTimes: [], dev: null,
    bind: null, challenges: new Map(), permits: new Map(), mints: [], eraStart: 0 });
  return boxes.get(rid);
};
// 连接许可（permit）逻辑：与 RelayDO 同语义（协议见 relay-worker/src/index.js 头部注释）
const ipPrefix = (ip) => {
  if (!ip) return "unknown";
  if (ip === "::1" || ip === "127.0.0.1") return ip;
  if (ip.includes(":")) return ip.split(":").slice(0, 4).join(":");
  const o = ip.split(".");
  return o.length === 4 ? o.slice(0, 3).join(".") : ip;
};
const pruneChallenges = (bx, now) => { for (const [c, exp] of bx.challenges) if (exp <= now) bx.challenges.delete(c); };
const prunePermits = (bx, now) => {
  for (const [t, v] of bx.permits) if (v.exp <= now) bx.permits.delete(t);
  if (bx.permits.size >= PERMIT_MAX) {
    let oldest = null, oldestExp = Infinity;
    for (const [t, v] of bx.permits) if (v.exp < oldestExp) { oldest = t; oldestExp = v.exp; }
    if (oldest) bx.permits.delete(oldest);
  }
};
const permitOk = (bx, tok, ip) => {
  const now = Date.now();
  prunePermits(bx, now);
  if (bx.eraStart && now - bx.eraStart > PERMIT_ERA_MS) {
    // 保活硬上限到点：整条许可链拒绝续期并清场（与 Worker 同语义）
    bx.permits.clear();
    bx.eraStart = 0;
    return false;
  }
  const v = bx.permits.get(tok);
  if (!v) return false;
  if (v.ip !== ipPrefix(ip)) return false;
  v.exp = now + PERMIT_TTL_MS;
  return true;
};
const mintPermit = (bx, ip) => {
  // 与 Worker 同语义：签发受 1h 滚动窗口双闸约束（总量 24 / 新网段 6），超限返回 null；
  // 时代到点清场后重新锚定；许可从零重建时记新时代起点
  const now = Date.now();
  prunePermits(bx, now);
  if (bx.eraStart && now - bx.eraStart > PERMIT_ERA_MS) { bx.permits.clear(); bx.eraStart = 0; }
  if (!bx.mints) bx.mints = [];
  bx.mints = bx.mints.filter((m) => now - m.ts < MINT_WINDOW_MS);
  if (bx.mints.length >= MINT_MAX_H) return null;
  const pfx = ipPrefix(ip);
  const distinct = new Set(bx.mints.map((m) => m.ip));
  if (!distinct.has(pfx) && distinct.size >= MINT_IPS_H) return null;
  if (bx.permits.size === 0) bx.eraStart = now;
  bx.mints.push({ ip: pfx, ts: now });
  const token = require("crypto").randomBytes(16).toString("hex");
  bx.permits.set(token, { ip: pfx, exp: now + PERMIT_TTL_MS });
  return token;
};
const checkProof = (bx, chal, proof) => {
  const now = Date.now();
  pruneChallenges(bx, now);
  if (!PROOF_RE.test(proof) || !bx.challenges.has(chal)) return false;
  bx.challenges.delete(chal);
  const expect = require("crypto").createHmac("sha256", Buffer.from(bx.bind, "hex")).update(chal).digest("hex");
  let diff = 0;
  for (let i = 0; i < 64; i++) diff |= expect.charCodeAt(i) ^ proof.charCodeAt(i);
  return diff === 0;
};
const json = (res, code, obj) => {
  const b = JSON.stringify(obj);
  res.writeHead(code, { "Content-Type": "application/json", "Access-Control-Allow-Origin": "*" });
  res.end(b);
};
const reqId = () => Math.random().toString(16).slice(2, 10);

// 单设备识别码上限（与 Worker devRidCap 一致，isolate/进程内存 best-effort）
const devRids = new Map(); // dev → Set(rid)
function devRidCap(dev, rid) {
  let s = devRids.get(dev);
  if (!s) { s = new Set(); devRids.set(dev, s); }
  s.add(rid);
  return s.size <= DEV_RID_MAX;
}
// 坏签名自动封禁（与 Worker noteBadSign/banned 一致）
const badSigns = new Map(); // ip → [ts]
const bans = new Map(); // ip → banUntil
const clientIp = (req) => req.socket.remoteAddress || "unknown";
function noteBadSign(ip) {
  const now = Date.now();
  const q = (badSigns.get(ip) || []).filter((t) => now - t < BADSIGN_WINDOW_MS);
  q.push(now);
  badSigns.set(ip, q);
  if (q.length >= BADSIGN_MAX) bans.set(ip, now + BAN_MS);
}
const banned = (ip) => (bans.get(ip) || 0) > Date.now();

const server = http.createServer((req, res) => {
  const parts = req.url.split("?")[0].split("/").filter(Boolean);
  if (req.method === "OPTIONS") return res.writeHead(204, { "Access-Control-Allow-Origin": "*" }).end();
  if (parts[1] === "health") return json(res, 200, { ok: true, service: "fake-relay",
    notice: "限流限速 + 蜜罐 + 自动封禁已开启，攻击无效，请勿尝试 / Rate-limited, honeypotted and auto-banned — attacks are futile, don't bother" });
  // 测试专用：清空封禁/坏签名/设备绑定状态（仅监听 127.0.0.1，E2E 幂等用）
  if (parts[1] === "_reset") {
    boxes.clear(); devRids.clear(); badSigns.clear(); bans.clear();
    return json(res, 200, { ok: true, reset: true });
  }
  // 蜜罐（与 Worker 同语义）：触碰诱捕路径 → 立即封禁 + 警告，不进任何协议逻辑
  // （必须在 rid 校验之前：/relay/admin 只有两段，parts[2] 为空，先到 404 就套不住）
  if (parts[1] && HONEYPOT.has(parts[1].toLowerCase())) {
    const hip = req.socket.remoteAddress || "unknown";
    bans.set(hip, Date.now() + BAN_MS);
    return json(res, 403, {
      flag: "BIT{honeypot_trapped_nice_try}",
      banned_until: new Date(Date.now() + BAN_MS).toISOString(),
      warning:
        "本站已部署蜜罐：你已因触碰诱捕路径被封禁 30 分钟。限流限速 + 蜜罐 + 自动封禁全部开启，" +
        "隧道不转发非 BIT App 流量，攻击只会浪费你自己的 IP。请勿尝试攻击。" +
        " Honeypot triggered: banned for 30 minutes. Rate limiting, honeypots and auto-bans are all active — " +
        "attacks are futile and will only burn your own IP. Don't bother.",
    });
  }
  if (parts[0] !== "relay" || !parts[2]) return json(res, 404, { error: "bad path" });
  const [, action, rid] = parts;
  const bx = box(rid);

  let raw = "";
  req.on("data", (c) => (raw += c));
  req.on("end", () => {
    console.error(`[fake-relay] ${ts()} ${req.method} ${req.url} body=${raw.length}B`);
    if (action === "poll" && req.method === "POST") {
      // rid↔设备绑定 + 单设备识别码上限（与 Worker 一致；BIT 端 poll 携带 X-BIT-Dev）
      const dev = req.headers["x-bit-dev"] || "";
      if (!DEV_RE.test(dev)) return json(res, 403, { error: "device fingerprint required (x-bit-dev, bitsign-v2 client)" });
      if (!devRidCap(dev, rid)) return json(res, 429, { error: "too many relay ids for this device" });
      if (bx.dev === null) bx.dev = dev;
      if (bx.dev !== dev) return json(res, 403, { error: "relay id bound to another device" });
      // 信道握手密钥（X-BIT-Bind）：就位后许可强制生效（与 Worker /do/poll 同语义）；
      // 密钥轮换（bind 变更）即吊销旧许可与挑战（与 Worker 同语义）
      const bind = req.headers["x-bit-bind"] || "";
      if (BIND_RE.test(bind) && bind !== bx.bind) {
        bx.bind = bind;
        bx.permits.clear();
        bx.challenges.clear();
        bx.eraStart = 0; // 新密钥 = 新信任时代（与 Worker 同语义）
      }
      bx.lastPoll = Date.now();
      if (bx.inbox.length) { console.error(`[fake-relay] ${ts()} poll->pickup (inbox=${bx.inbox.length})`); return json(res, 200, bx.inbox.shift()); }
      const timer = setTimeout(() => {
        const i = bx.waiters.indexOf(waiter);
        if (i >= 0) bx.waiters.splice(i, 1);
        res.writeHead(204).end();
      }, 3000);
      const waiter = { res, timer };
      bx.waiters.push(waiter);
      return;
    }
    if (action === "status") {
      return json(res, 200, { online: Date.now() - (bx.lastPoll || 0) < 35_000, pending: bx.inbox.length, dev: bx.dev ? bx.dev.slice(0, 8) : null });
    }
    if (action === "answer" && req.method === "POST") {
      const a = JSON.parse(raw || "{}");
      const w = bx.answers.get(a.rid);
      if (!w) return json(res, 410, { error: "stale" });
      if (w.writer) {
        // 分块模式：写出当前块，last=true 收口；有块流动就重置空闲计时
        const write = () => {
          if (a.b) w.writer.write(Buffer.from(a.b, "base64"));
          if (a.last) {
            clearTimeout(w.timer);
            bx.answers.delete(a.rid);
            w.writer.close().catch(() => {});
          } else {
            clearTimeout(w.timer);
            w.timer = setTimeout(() => expire(bx, a.rid), 15000);
          }
          json(res, 200, { ok: true });
        };
        return write();
      }
      if (a.seq !== undefined) return json(res, 400, { error: "not a stream answer" });
      clearTimeout(w.timer);
      bx.answers.delete(a.rid);
      const aHeaders = {
        "Content-Type": a.h?.["content-type"] || "application/octet-stream",
        "Access-Control-Allow-Origin": "*",
      };
      if (w.permit) aHeaders["x-bit-permit"] = w.permit; // 新签发/续期的许可随响应头带回
      w.res.writeHead(a.s || 502, aHeaders);
      w.res.end(Buffer.from(a.b || "", "base64"));
      // 必须回 BIT 的 answer 请求本身（与 Worker /do/answer 一致返回 {ok:true}），
      // 否则 BIT 的 answer POST 挂到超时，中继循环停摆、后续隧道请求全部延迟
      return json(res, 200, { ok: true });
    }
    if (action === "challenge") {
      // 连接许可握手第一步：发一次性挑战（与 Worker /do/challenge 同语义，含封禁检查）
      if (banned(clientIp(req))) return json(res, 403, { error: "temporarily banned" });
      const c = require("crypto").randomBytes(16).toString("hex");
      pruneChallenges(bx, Date.now());
      bx.challenges.set(c, Date.now() + CHALLENGE_TTL_MS);
      return json(res, 200, { c, ttl: CHALLENGE_TTL_MS / 1000 });
    }
    if (action === "ka") {
      // 保活心跳（与 Worker /do/ka 同语义）：验证/续期许可（或现场握手）+ BIT 在线状态，
      // 不进隧道队列不耗配额。签名路径约定 "/relay/ka"，仅格式门槛
      const ip = clientIp(req);
      if (banned(ip)) return json(res, 403, { error: "temporarily banned" });
      const sign = req.headers["x-bit-sign"] || "";
      const tsH = req.headers["x-bit-ts"] || "";
      const nonce = req.headers["x-bit-nonce"] || "";
      if (!SIGN_RE.test(sign) || !TS_RE.test(tsH) || !NONCE_RE.test(nonce)
        || Math.abs(Date.now() / 1000 - Number(tsH)) > 300) {
        noteBadSign(ip);
        return json(res, 403, { error: "channel signature required (bitsign-v2)" });
      }
      const ipEnv = req.headers["x-fake-client-ip"] || ip || "";
      let permit = null;
      if (bx.bind) {
        const tok = req.headers["x-bit-permit"] || "";
        if (tok && permitOk(bx, tok, ipEnv)) {
          permit = tok;
        } else {
          const chal = req.headers["x-bit-challenge"] || "";
          const proof = req.headers["x-bit-proof"] || "";
          if (!chal || !proof || !checkProof(bx, chal, proof)) {
            return json(res, 403, { error: "connection permit required (handshake first)" });
          }
          permit = mintPermit(bx, ipEnv);
          if (!permit) return json(res, 429, { error: "permit mint rate limited (ip churn or flood)" });
        }
      }
      const kaHeaders = { "Access-Control-Allow-Origin": "*" };
      if (permit) kaHeaders["x-bit-permit"] = permit;
      res.writeHead(200, kaHeaders);
      return res.end(JSON.stringify({
        ok: true,
        online: Date.now() - (bx.lastPoll || 0) < 35_000,
        pending: bx.inbox.length,
      }));
    }
    if (action === "req") {
      // 防滥用：封禁名单 + 坏签名记账（与 Worker 站点层一致；真伪由 BIT 验）
      const ip = clientIp(req);
      if (banned(ip)) return json(res, 403, { error: "temporarily banned" });
      // 信道防护：签名三件套格式校验（与 Worker 门槛一致；真伪由 BIT 验）
      const sign = req.headers["x-bit-sign"] || "";
      const tsH = req.headers["x-bit-ts"] || "";
      const nonce = req.headers["x-bit-nonce"] || "";
      if (!SIGN_RE.test(sign) || !TS_RE.test(tsH) || !NONCE_RE.test(nonce)) {
        noteBadSign(ip);
        return json(res, 403, { error: "channel signature required (bitsign-v2)" });
      }
      if (Math.abs(Date.now() / 1000 - Number(tsH)) > 300) {
        noteBadSign(ip);
        return json(res, 403, { error: "channel signature stale" });
      }
      const path = "/" + parts.slice(3).join("/");
      if (path === "/") return json(res, 404, { error: "missing tunnel path" });
      // 敏感路径拒绝 + 文字信道 + 体上限（与 Worker 站点层语义一致）
      if (path === "/api/qr" || path.startsWith("/api/debug")) {
        return json(res, 403, { error: "sensitive path blocked on relay channel" });
      }
      const ct = (req.headers["content-type"] || "").toLowerCase();
      const ctOk = !ct || ct.startsWith("application/json")
        || ct.startsWith("application/x-www-form-urlencoded") || ct.startsWith("text/");
      if (!ctOk) return json(res, 415, { error: "relay channel is text-only (json/text payloads only)" });
      if (raw.length > BODY_MAX) return json(res, 413, { error: "payload too large" });
      // 连接许可闸门（与 Worker /do/req 同语义）：握手密钥就位后，持有效许可 → 续期放行；
      // 否则须带一次性挑战 + 正确应答现场换许可；两者皆无 → 403，请求不进 BIT 队列
      const ipEnv = req.headers["x-fake-client-ip"] || req.socket.remoteAddress || "";
      let permit = null;
      if (bx.bind) {
        const tok = req.headers["x-bit-permit"] || "";
        if (tok && permitOk(bx, tok, ipEnv)) {
          permit = tok;
        } else {
          const chal = req.headers["x-bit-challenge"] || "";
          const proof = req.headers["x-bit-proof"] || "";
          if (!chal || !proof || !checkProof(bx, chal, proof)) {
            noteBadSign(ip);
            return json(res, 403, { error: "connection permit required (handshake first)" });
          }
          permit = mintPermit(bx, ipEnv);
          // IP 变动闸门：1h 内新网段数/签发总量超限 → 429，请求不入队（与 Worker 一致）
          if (!permit) return json(res, 429, { error: "permit mint rate limited (ip churn or flood)" });
        }
      }
      const envelope = {
        rid: reqId(),
        m: req.method,
        p: path,
        h: {
          authorization: req.headers.authorization || "",
          "x-access-password": req.headers["x-access-password"] || "",
          "content-type": req.headers["content-type"] || "",
          "x-bit-sign": sign,
          "x-bit-ts": tsH,
          "x-bit-nonce": nonce,
        },
        b: raw ? Buffer.from(raw).toString("base64") : "",
        // 手机端真实 IP（E2E 用 x-fake-client-ip 模拟 Worker 的 cf-connecting-ip 注入；
        // 黑名单用例据此把伪造源 IP 送到 BIT 端计数）
        ip: req.headers["x-fake-client-ip"] || req.socket.remoteAddress || "",
        // 来源一致性盖章（与 Worker /do/req 一致）：许可解析通过后，把"许可签发时
        // 绑定的来源网段"（许可记录值，非现算）写入信封 pfx——BIT 端比对该网段与
        // 本次请求真实 IP 的网段，一致才放行。x-fake-stamp-pfx 为测试专用覆盖
        // （模拟中继被替换 / 盖章被破坏；生产 Worker 无此头）
        pfx: req.headers["x-fake-stamp-pfx"] || (permit ? ((bx.permits.get(permit) || {}).ip || "") : ""),
        // 流式探测：与 Worker looksStreaming 同口径
        stream: /"stream"\s*:\s*true/.test(raw.slice(0, 8192)) ||
          String(req.headers.accept || "").toLowerCase().includes("text/event-stream"),
      };
      bx.inbox.push(envelope);
      // 唤醒等待中的 poll
      while (bx.inbox.length && bx.waiters.length) {
        const w = bx.waiters.shift();
        clearTimeout(w.timer);
        console.error(`[fake-relay] ${ts()} poll->wake`);
        json(w.res, 200, bx.inbox.shift());
      }
      if (envelope.stream) {
        // 流式：TransformStream 增量响应（Node web stream → http res）
        const { readable, writable } = new TransformStream();
        const timer = setTimeout(() => expire(bx, envelope.rid), 15000);
        bx.answers.set(envelope.rid, { writer: writable.getWriter(), timer });
        const sHeaders = {
          "Content-Type": "text/event-stream",
          "Cache-Control": "no-cache",
          "Access-Control-Allow-Origin": "*",
        };
        if (permit) sHeaders["x-bit-permit"] = permit; // 新签发/续期的许可随响应头带回
        res.writeHead(200, sHeaders);
        Readable.fromWeb(readable).pipe(res);
        return;
      }
      // 非流式：等应用端 answer（超时 → 504，与 Worker 语义一致）
      const promise = new Promise((resolve) => {
        const timer = setTimeout(() => {
          bx.answers.delete(envelope.rid);
          res.writeHead(504, { "Content-Type": "text/plain" });
          res.end("relay: BIT offline or timeout");
          resolve(null);
        }, 15000);
        bx.answers.set(envelope.rid, { res, timer, permit }); // permit 随 answer 回放
      });
      return promise;
    }
    json(res, 404, { error: "unknown action" });
  });
});

// 空闲/等待超时：非流式 504；流式关闭 writer（客户端感知断流）
function expire(bx, rid) {
  const w = bx.answers.get(rid);
  if (!w) return;
  bx.answers.delete(rid);
  if (w.writer) {
    w.writer.close().catch(() => {});
  } else {
    w.res.writeHead(504, { "Content-Type": "text/plain" });
    w.res.end("relay: BIT offline or timeout");
  }
}

server.listen(PORT, "127.0.0.1", () => console.log(`[fake-relay] listening on 127.0.0.1:${PORT}`));
// keep-alive 超时拉长：node 默认 5s 会主动断开空闲连接，reqwest 复用该连接回传 answer
// 时偶发 connection reset（真实 Cloudflare 边缘不会），导致 E2E 偶发假阴性
server.keepAliveTimeout = 65_000;
server.headersTimeout = 66_000;
const ts = () => new Date().toISOString().slice(11, 23);
