// yxpil · BIT — 三服务商扮演 AI 的功能矩阵测试
// 虚拟 3 个上游 AI 服务商（均由 mock-ai:9901 承担），分别对应三种不同的 API 格式：
//   openai-native → OpenAI  /v1/chat/completions（原生 function calling）
//   e2e-mock-claude  → Anthropic  /v1/messages（原生 tool_use）
//   e2e-mock-gemini  → Google  :generateContent（原生 functionCall）
// 每个服务商「扮演同一个 AI」跑一遍同一组功能：普通对话 → 原生工具调用（一轮 shell + 结果回传）。
// 通过 /api/debug/config 的 active_provider 热切换上游；BIT 侧统一走标准原生协议（compat_mode=false）。
// 前置：实例已用隔离目录启动（BIT_DATA_DIR=/tmp/bit-e2e…），且已 node e2e/activate.cjs 注册三家 mock 提供方。
// 用法：E2E_KEY=… E2E_PASSWORD=… node e2e/providers-matrix.cjs
const http = require("http");
const BASE = "127.0.0.1", PORT = 8600;
const KEY = process.env.E2E_KEY || "bit_e2e_client_key_0000000000000000";
const PW = process.env.E2E_PASSWORD || "87654321";

const results = [];
const record = (name, ok, detail = "") => {
  results.push({ name, ok, detail });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
};

function req(method, path, body, timeoutMs = 60000) {
  return new Promise((resolve) => {
    const data = body == null ? null : JSON.stringify(body);
    const h = {
      authorization: `Bearer ${KEY}`,
      "x-access-password": PW,
      "content-type": "application/json",
    };
    if (data != null) h["content-length"] = Buffer.byteLength(data);
    const r = http.request(
      { host: BASE, port: PORT, method, path, headers: h, timeout: timeoutMs },
      (res) => {
        let buf = "";
        res.on("data", (c) => (buf += c));
        res.on("end", () => resolve({ code: res.statusCode, body: buf }));
      }
    );
    r.on("timeout", () => { r.destroy(); resolve({ code: 0, body: "TIMEOUT" }); });
    r.on("error", (e) => resolve({ code: 0, body: "ERR:" + e.code }));
    if (data != null) r.write(data);
    r.end();
  });
}
const cfgPost = async (obj) => {
  const r = await req("POST", "/api/debug/config", obj);
  return r.code === 200;
};
const chat = async (sid, msg) => {
  const r = await req("POST", "/api/chat", { session_id: sid, message: msg });
  if (r.code !== 200) return { code: r.code, reply: r.body };
  const j = JSON.parse(r.body);
  return { code: 200, reply: j.reply || "" };
};
// 断言用：去掉全部空白，规避 mock stdout 尾随换行造成的边界失配
const flat = (s) => String(s || "").replace(/\s+/g, "");

// 三个虚拟服务商：id 必须与 activate.cjs 注册的提供方 id 一致；各自格式关键词与期望标记
const PROVIDERS = [
  {
    id: "e2e-mock-openai-native",
    label: "OpenAI  /v1/chat/completions",
    trigger: "E2E-NAT-OPENAI",
    expect: ["e2e-native-openai-ok"],
  },
  {
    id: "e2e-mock-claude",
    label: "Claude  /v1/messages",
    trigger: "E2E-CLAUDE-NAT",
    expect: ["e2e-claude-native-ok", "tool_result=true"],
  },
  {
    id: "e2e-mock-gemini",
    label: "Gemini  :generateContent",
    trigger: "E2E-GEMINI-NAT",
    expect: ["e2e-gemini-native-ok", "functionResponse=true"],
  },
];

(async () => {
  // 等实例就绪
  for (let i = 0; i < 20; i++) {
    try {
      await new Promise((res, rej) => {
        const q = http.get({ host: BASE, port: PORT, path: "/api/health", timeout: 2000 }, (r) => { r.resume(); res(); });
        q.on("error", rej);
        q.on("timeout", () => { q.destroy(); rej(new Error("t")); });
      });
      break;
    } catch { await new Promise((r) => setTimeout(r, 2000)); }
  }

  // 协议族基线：全程标准原生协议（各家 URL 由 provider 的 protocol 决定）
  await cfgPost({ compat_mode: false });

  for (const p of PROVIDERS) {
    const tag = `${p.id}`;
    // 切换当前扮演 AI 的上游 = 该虚拟服务商
    if (!(await cfgPost({ active_provider: p.id }))) {
      record(`${p.label} 切换服务商`, false, "active_provider 切换失败");
      continue;
    }

    // 功能 1：普通文本对话（三个格式各自的 E2E-PLAIN 支持）
    {
      const sid = `pv-${Date.now().toString(36)}-${p.id.replace(/[^a-z0-9]/gi, "")}-a`;
      const r = await chat(sid, "E2E-PLAIN 三服务商对话探测");
      record(`${tag} plain-chat`, r.code === 200 && flat(r.reply).includes("E2E-FINAL-PLAIN"), `reply=${String(r.reply).slice(0, 60)}`);
    }

    // 功能 2：原生工具调用一轮（shell echo → 结果回传 → AI 确认）——各格式原生通道
    {
      const sid = `pv-${Date.now().toString(36)}-${p.id.replace(/[^a-z0-9]/gi, "")}-b`;
      const r = await chat(sid, `${p.trigger} go`);
      const f = flat(r.reply);
      const okAll = p.expect.every((k) => f.includes(k));
      record(`${tag} native-tool-roundtrip`, r.code === 200 && okAll, `reply=${String(r.reply).slice(0, 110)}`);
    }
  }

  // 还原基线：active_provider 是持久配置，必须归位默认 OpenAI mock，避免污染其它套件
  await cfgPost({ compat_mode: false });
  await cfgPost({ active_provider: "e2e-mock-provider" });

  const pass = results.filter((x) => x.ok).length;
  console.log(`\n==== PROVIDER MATRIX ${pass}/${results.length} ====`);
  process.exit(pass === results.length ? 0 : 1);
})();
