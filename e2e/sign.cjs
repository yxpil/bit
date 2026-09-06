// yxpil · BIT — bitsign-v2 签名工具（与 src-tauri/src/security.rs 算法一致，E2E/真机验证用）
// canonical = "bitsign-v2\n{rid}\n{ts}\n{nonce}\n{method}\n{path}\n{device_material}"
// device_material = 设备凭证的加盐 SHA256 前 16 hex（device.rs::sig_material 同口径：
//                   sha256("bitdev-material:" + device_key) 前 16 hex）
// mac = HMAC-SHA256(key=client_key, canonical)；k = SHA256("bitsign-v2:"+client_key)
// out[i] = rotl(mac[i], (i%7)+1) ^ k[i%16]；sign = hex(out)
const crypto = require("crypto");

function bitsign(clientKey, deviceMaterial, rid, ts, nonce, method, path) {
  const canonical = `bitsign-v2\n${rid}\n${ts}\n${nonce}\n${method}\n${path}\n${deviceMaterial}`;
  const mac = crypto.createHmac("sha256", clientKey).update(canonical).digest();
  const k = crypto.createHash("sha256").update(`bitsign-v2:${clientKey}`).digest();
  const out = Buffer.alloc(32);
  for (let i = 0; i < 32; i++) {
    const n = (i % 7) + 1;
    const rot = ((mac[i] << n) | (mac[i] >>> (8 - n))) & 0xff;
    out[i] = rot ^ k[i % 16];
  }
  return out.toString("hex");
}

/** 与 device.rs::sig_material 同口径：设备 key → 信道签名材料（16 hex） */
function deviceMaterial(deviceKey) {
  return crypto.createHash("sha256").update(`bitdev-material:${deviceKey}`).digest().subarray(0, 8).toString("hex");
}

/** 生成签名三件套请求头（nonce 随机 16 hex；opts={ts,nonce} 用于重放测试固定值） */
function signHeaders(clientKey, deviceMaterial, rid, method, path, opts = {}) {
  const ts = opts.ts ?? Math.floor(Date.now() / 1000);
  const nonce = opts.nonce ?? crypto.randomBytes(8).toString("hex");
  return {
    "x-bit-sign": bitsign(clientKey, deviceMaterial, rid, ts, nonce, method, path),
    "x-bit-ts": String(ts),
    "x-bit-nonce": nonce,
  };
}

// ── 连接许可（permit）协议（与 relay-worker/src/index.js 头部注释同口径）──

/** 信道握手密钥：S = HMAC-SHA256(key=client_key, msg="bit-worker-bind:{rid}")（64 hex）。
 *  BIT 端每次 poll 经 X-BIT-Bind 下发给服务器；手机端从二维码密文内的 client_key 自行派生 */
function workerBind(clientKey, rid) {
  return crypto.createHmac("sha256", clientKey).update(`bit-worker-bind:${rid}`).digest("hex");
}

/** 挑战应答：proof = HMAC-SHA256(key=bytes(S), msg=challenge)（64 hex，与 challenge 一同携带） */
function proofOf(bindHex, challenge) {
  return crypto.createHmac("sha256", Buffer.from(bindHex, "hex")).update(challenge).digest("hex");
}

/** 完整握手（生产 osbt.space）：取挑战 → 算应答，返回许可请求头（x-bit-challenge / x-bit-proof）。
 *  本地 E2E 走 http，由 run.cjs 自行实现同流程 */
async function handshakeHeaders(rid, bindHex) {
  const c = await new Promise((resolve, reject) => {
    const q = require("https").get({ host: "osbt.space", path: `/relay/challenge/${rid}`, timeout: 15000 },
      (r) => { let b = ""; r.on("data", (c2) => (b += c2)); r.on("end", () => { try { resolve(JSON.parse(b).c); } catch (e) { reject(e); } }); });
    q.on("error", reject); q.on("timeout", () => { q.destroy(); reject(new Error("challenge timeout")); });
  });
  return { "x-bit-challenge": c, "x-bit-proof": proofOf(bindHex, c) };
}

module.exports = { bitsign, deviceMaterial, signHeaders, workerBind, proofOf, handshakeHeaders };
