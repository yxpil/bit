// AI Actor E2E：本脚本扮演「AI 模型」（OpenAI 兼容上游），驱动 BIT 的真实工具执行链路。
// 与 mock-ai 的区别：mock 是被动应答，actor 会对 BIT 回传的工具结果做【断言】——
// 即「模型视角」验证：调用被执行了吗？结果对吗？tool_call_id 回传对吗？错误反馈可纠正吗？
//
// 覆盖：
//   C1 标准 function calling 闭环（tool_calls → 执行 → tool 消息回传，id 配对）
//   C2 一轮并行多调用（两条 tool 消息都回来）
//   C3 文件写入 + shell 读回（跨工具内容一致性）
//   C4 幻觉工具 → 错误反馈含可用列表 → 自我纠正换真实工具
//   C5 参数缺失 → 错误反馈 → 修参重试成功
//   C6 工具清单契约：全部 type=function、schema 齐全；sub_agent 不得出现在 tools 与系统提示词
//   C7 多轮连续调用（≥3 轮不丢轮次）
//
// 用法：node e2e/ai-actor.cjs [bin路径]   （自起隔离实例，端口 8607 / 上游 9905）
const http = require("http");
const fs = require("fs");
const os = require("os");
const path = require("path");
const { spawn } = require("child_process");

const BIN = process.argv[2] || "src-tauri/target/debug/bit";
const UP_PORT = 9905; // 扮演 AI 的上游端口
const APP_PORT = 8607; // 被测 BIT 实例端口
const KEY = "bit_actor_client_key_000000000000";
const PASSWORD = "actor-pass-0001";

const results = [];
const record = (name, ok, detail = "") => {
  results.push({ name, ok });
  console.log(`${ok ? "✓" : "✗"} ${name}${detail ? "  " + detail : ""}`);
};

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ── 断言辅助：actor 收到的请求 ──
const contentText = (m) => (typeof m.content === "string" ? m.content : Array.isArray(m.content) ? m.content.map((p) => p.text || "").join("\n") : "");
const toolMsgs = (msgs) => msgs.filter((m) => m.role === "tool");
const findMarker = (msgs, marker) => msgs.some((m) => contentText(m).includes(marker));
const assistantToolCalls = (msgs) =>
  msgs.filter((m) => m.role === "assistant" && Array.isArray(m.tool_calls)).flatMap((m) => m.tool_calls);
const stdoutOf = (t) => (String(t.content || "").match(/"stdout"\s*:\s*"([^"]*)"/) || [])[1] || "";

// ── 上游响应（OpenAI 两种形态都支持，与 mock-ai 同构）──
function respondJSON(res, body) {
  res.writeHead(200, { "Content-Type": "application/json" });
  res.end(JSON.stringify(body));
}
function respondText(res, sse, messages, content) {
  const usage = { prompt_tokens: 120, completion_tokens: 42, prompt_tokens_details: { cached_tokens: 0 } };
  if (sse) {
    res.writeHead(200, { "Content-Type": "text/event-stream" });
    res.write(`data: ${JSON.stringify({ id: "actor", object: "chat.completion.chunk", choices: [{ index: 0, delta: { content } }], usage })}\n\n`);
    res.write("data: [DONE]\n\n");
    return res.end();
  }
  respondJSON(res, { id: "actor", object: "chat.completion", choices: [{ index: 0, message: { role: "assistant", content }, finish_reason: "stop" }], usage });
}
// 原生 tool_calls：一次 turn 可以带多个调用（并行）
function respondTools(res, sse, calls) {
  if (sse) {
    res.writeHead(200, { "Content-Type": "text/event-stream" });
    const mk = (d) => JSON.stringify({ id: "actor", object: "chat.completion.chunk", choices: [{ index: 0, delta: d }] });
    calls.forEach((c, i) => {
      res.write(`data: ${mk({ role: "assistant", tool_calls: [{ index: i, id: c.id, type: "function", function: { name: c.name, arguments: "" } }] })}\n\n`);
      res.write(`data: ${mk({ tool_calls: [{ index: i, function: { arguments: JSON.stringify(c.args) } }] })}\n\n`);
    });
    res.write(`data: ${JSON.stringify({ id: "actor", object: "chat.completion.chunk", choices: [{ index: 0, delta: {}, finish_reason: "tool_calls" }] })}\n\n`);
    res.write("data: [DONE]\n\n");
    return res.end();
  }
  respondJSON(res, {
    id: "actor",
    object: "chat.completion",
    choices: [{
      index: 0,
      message: { role: "assistant", content: null, tool_calls: calls.map((c) => ({ id: c.id, type: "function", function: { name: c.name, arguments: JSON.stringify(c.args) } })) },
      finish_reason: "tool_calls",
    }],
  });
}

const mkCall = (id, name, args) => ({ id, name, args });

// ── 各场景：函数收 (req, res, parsed)，内部断言并决定本轮回应 ──
const TMP_DIR = fs.mkdtempSync(path.join(os.tmpdir(), "bit-actor-work-"));
const C3_FILE = path.join(TMP_DIR, "actor-c3.txt");

const scenarios = {
  // C1 标准 function calling 闭环
  C1(req, res, parsed) {
    const { sse, msgs } = req;
    const rounds = toolMsgs(msgs).length;
    if (rounds === 0) return respondTools(res, sse, [mkCall("call-actor-1", "shell", { command: "echo ACTOR-C1-OK" })]);
    // 断言1：工具真的执行了，stdout 回传
    const t = toolMsgs(msgs)[0];
    const out = stdoutOf(t);
    record("C1 工具执行结果回传 stdout", out.includes("ACTOR-C1-OK"), `stdout=「${out}」`);
    // 断言2：tool_call_id 与调用时配对
    record("C1 tool_call_id 配对", t.tool_call_id === "call-actor-1", `got=${t.tool_call_id}`);
    // 断言3：assistant 的 tool_calls 消息被回放进历史（协议闭环）
    const echoed = assistantToolCalls(msgs).find((c) => c.id === "call-actor-1");
    record("C1 assistant tool_calls 回放历史", !!echoed && echoed.function?.name === "shell", echoed ? `name=${echoed.function?.name}` : "缺失");
    return respondText(res, sse, msgs, "ACTOR-C1-DONE 闭环完成");
  },

  // C2 一轮并行多调用
  C2(req, res, parsed) {
    const { sse, msgs } = req;
    const rounds = toolMsgs(msgs).length;
    if (rounds === 0)
      return respondTools(res, sse, [
        mkCall("call-par-a", "shell", { command: "echo ACTOR-C2-ALPHA" }),
        mkCall("call-par-b", "shell", { command: "echo ACTOR-C2-BETA" }),
      ]);
    const outs = toolMsgs(msgs).map(stdoutOf).join("|");
    record("C2 并行两调用都执行", outs.includes("ACTOR-C2-ALPHA") && outs.includes("ACTOR-C2-BETA"), `outs=${outs}`);
    record("C2 两条独立 tool 消息", toolMsgs(msgs).length === 2, `n=${toolMsgs(msgs).length}`);
    const ids = toolMsgs(msgs).map((t) => t.tool_call_id).sort().join(",");
    record("C2 并行 id 各自配对", ids === "call-par-a,call-par-b", `ids=${ids}`);
    return respondText(res, sse, msgs, "ACTOR-C2-DONE 并行完成");
  },

  // C3 文件写入 + shell 读回
  C3(req, res, parsed) {
    const { sse, msgs } = req;
    const rounds = toolMsgs(msgs).length;
    if (rounds === 0) return respondTools(res, sse, [mkCall("call-w", "write_file", { path: C3_FILE, content: "actor-file-content-42" })]);
    if (rounds === 1) {
      const t = toolMsgs(msgs)[0];
      record("C3 write_file 执行成功", /"bytes"\s*:\s*\d+/.test(t.content) && !/error|Failed|失败/i.test(t.content), `fb=${String(t.content).slice(0, 80)}`);
      return respondTools(res, sse, [mkCall("call-r", "shell", { command: `cat '${C3_FILE}'` })]);
    }
    const out = stdoutOf(toolMsgs(msgs)[1]);
    record("C3 shell 读回内容一致", out.includes("actor-file-content-42"), `stdout=「${out}」`);
    return respondText(res, sse, msgs, "ACTOR-C3-DONE 文件闭环完成");
  },

  // C4 幻觉工具 → 错误反馈（含可用列表）→ 纠正
  C4(req, res, parsed) {
    const { sse, msgs } = req;
    const rounds = toolMsgs(msgs).length;
    if (rounds === 0) return respondTools(res, sse, [mkCall("call-h", "no_such_tool_xyz", {})]);
    if (rounds === 1) {
      const t = toolMsgs(msgs)[0];
      const fb = String(t.content || "");
      record("C4 幻觉工具报错反馈", /unknown tool|不存在|no_such_tool_xyz/i.test(fb), `fb=${fb.slice(0, 80)}`);
      record("C4 错误反馈含可用工具列表", /Available|可用/i.test(fb) && /shell/.test(fb), "hint 带列表");
      return respondTools(res, sse, [mkCall("call-fix", "shell", { command: "echo ACTOR-C4-OK" })]);
    }
    record("C4 纠正后执行成功", stdoutOf(toolMsgs(msgs)[1]).includes("ACTOR-C4-OK"));
    return respondText(res, sse, msgs, "ACTOR-C4-DONE 自我纠正完成");
  },

  // C5 参数缺失 → 错误反馈 → 修参重试
  C5(req, res, parsed) {
    const { sse, msgs } = req;
    const rounds = toolMsgs(msgs).length;
    if (rounds === 0) return respondTools(res, sse, [mkCall("call-bad", "shell", { cmd: "wrong-param-name" })]);
    if (rounds === 1) {
      const fb = String(toolMsgs(msgs)[0].content || "");
      record("C5 缺参错误反馈", /missing parameter|command/i.test(fb), `fb=${fb.slice(0, 80)}`);
      return respondTools(res, sse, [mkCall("call-good", "shell", { command: "echo ACTOR-C5-OK" })]);
    }
    record("C5 修参重试成功", stdoutOf(toolMsgs(msgs)[1]).includes("ACTOR-C5-OK"));
    return respondText(res, sse, msgs, "ACTOR-C5-DONE 修参重试完成");
  },

  // C6 工具清单契约（不调用工具，纯断言清单 + 系统提示词）
  C6(req, res, parsed) {
    const { sse, msgs } = req;
    const tools = parsed.tools || [];
    const names = tools.map((t) => t?.function?.name);
    record("C6 携带原生 tools 清单", tools.length > 0, `n=${tools.length}`);
    record(
      "C6 全部 type=function 且 schema 齐全",
      tools.every((t) => t?.type === "function" && t?.function?.name && t?.function?.description && t?.function?.parameters?.type === "object"),
      "name/description/parameters 缺一不可"
    );
    record("C6 sub_agent 不在工具清单", !names.includes("sub_agent"), names.includes("sub_agent") ? "泄露！" : "已剔除");
    const sys = msgs.find((m) => m.role === "system");
    record("C6 系统提示词不含 sub_agent", !!sys && !String(sys.content || "").includes("sub_agent"), sys ? "" : "无 system 消息");
    record("C6 系统提示词含宿主调度表述", !!sys && /host-scheduled|宿主/i.test(String(sys.content || "")), "");
    record("C6 常用工具在清单中", ["shell", "write_file", "plan"].every((n) => names.includes(n)), `shell/write_file/plan`);
    return respondText(res, sse, msgs, "ACTOR-C6-DONE 清单契约通过");
  },

  // C7 多轮连续调用（3 轮工具 + 终答）
  C7(req, res, parsed) {
    const { sse, msgs } = req;
    const rounds = toolMsgs(msgs).length;
    if (rounds < 3) return respondTools(res, sse, [mkCall(`call-m${rounds}`, "shell", { command: `echo ACTOR-C7-STEP${rounds + 1}` })]);
    const outs = toolMsgs(msgs).map(stdoutOf).join("|");
    record("C7 三轮全部执行", ["STEP1", "STEP2", "STEP3"].every((s) => outs.includes(`ACTOR-C7-${s}`)), `outs=${outs}`);
    return respondText(res, sse, msgs, "ACTOR-C7-DONE 多轮完成");
  },
};

// ── 上游服务器 ──
let lastReqDump = "";
const upstream = http.createServer((req, res) => {
  if (req.method !== "POST") {
    if (req.method === "GET" && req.url.includes("/models")) {
      res.writeHead(200, { "Content-Type": "application/json" });
      return res.end(JSON.stringify({ object: "list", data: [{ id: "actor-1", object: "model", owned_by: "actor", context_length: 32768 }] }));
    }
    res.writeHead(404);
    return res.end();
  }
  const chunks = [];
  req.on("data", (c) => chunks.push(c));
  req.on("end", () => {
    let parsed;
    try {
      parsed = JSON.parse(Buffer.concat(chunks).toString("utf8"));
    } catch {
      res.writeHead(400);
      return res.end("{}");
    }
    const msgs = parsed.messages || [];
    const sse = !!parsed.stream;
    const marker = (["ACTOR-C1", "ACTOR-C2", "ACTOR-C3", "ACTOR-C4", "ACTOR-C5", "ACTOR-C6", "ACTOR-C7"].find((k) =>
      msgs.some((m) => contentText(m).includes(k))
    ) || "").slice("ACTOR-".length);
    lastReqDump = JSON.stringify({ tools: !!(parsed.tools || []).length, last: contentText(msgs[msgs.length - 1] || {}).slice(0, 60) });
    console.log(`[actor-upstream] ${sse ? "sse" : "json"} tools=${!!parsed.tools} case=${marker || "-"} toolmsgs=${toolMsgs(msgs).length}`);
    if (marker && scenarios[marker]) return scenarios[marker]({ sse, msgs }, res, parsed);
    // 未识别（健康探测等）：普通文本，不触发工具
    return respondText(res, sse, msgs, "好的。");
  });
});

// ── 驱动端：以用户身份打 BIT ──
function call(path, body, timeout = 120000) {
  return new Promise((resolve, reject) => {
    const data = JSON.stringify(body);
    const rq = http.request(
      {
        host: "127.0.0.1",
        port: APP_PORT,
        path,
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          Authorization: `Bearer ${KEY}`,
          "X-Access-Password": PASSWORD,
          "Content-Length": Buffer.byteLength(data),
        },
        timeout,
      },
      (rs) => {
        let b = "";
        rs.on("data", (c) => (b += c));
        rs.on("end", () => resolve({ code: rs.statusCode, body: b }));
      }
    );
    rq.on("timeout", () => {
      rq.destroy();
      reject(new Error("timeout"));
    });
    rq.on("error", reject);
    rq.end(data);
  });
}

async function chat(session, message) {
  const r = await call("/api/chat", { session_id: session, message });
  let json = {};
  try {
    json = JSON.parse(r.body);
  } catch {}
  return { code: r.code, reply: json.reply || "", msgs: json.messages || [] };
}

async function waitHealth() {
  for (let i = 0; i < 60; i++) {
    const ok = await new Promise((resolve) => {
      const rq = http.request(
        { host: "127.0.0.1", port: APP_PORT, path: "/api/health", method: "GET", timeout: 2000 },
        (rs) => {
          rs.resume();
          resolve(rs.statusCode === 200);
        }
      );
      rq.on("timeout", () => {
        rq.destroy();
        resolve(false);
      });
      rq.on("error", () => resolve(false));
      rq.end();
    });
    if (ok) return true;
    await sleep(500);
  }
  return false;
}

function sameBinaryPids(bin) {
  try {
    const out = require("child_process").execSync("ps -axo pid=,command=", { encoding: "utf8" });
    let real = bin;
    try {
      real = fs.realpathSync(bin);
    } catch {}
    return out
      .split("\n")
      .map((l) => {
        const m = l.trim().match(/^(\d+)\s+(\S+)/);
        if (!m) return 0;
        let cmd = m[2];
        try {
          cmd = fs.realpathSync(cmd);
        } catch {}
        return cmd === real ? parseInt(m[1], 10) : 0;
      })
      .filter(Boolean);
  } catch {
    return [];
  }
}

(async () => {
  // 清理同二进制残留实例（主进程 + guardian 会令测试实例启动即退）
  const pre = sameBinaryPids(BIN);
  for (const p of pre) {
    try {
      process.kill(p, "SIGKILL");
    } catch {}
  }
  if (pre.length) {
    await sleep(1500);
    for (const p of sameBinaryPids(BIN)) {
      try {
        process.kill(p, "SIGKILL");
      } catch {}
    }
  }

  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "bit-actor-"));
  fs.writeFileSync(
    path.join(dir, "config.json"),
    JSON.stringify(
      {
        remote_enabled: true,
        host: "127.0.0.1",
        port: APP_PORT,
        client_key: KEY,
        access_password: PASSWORD,
        password_enabled: true,
        revision: 1,
        tool_approval: "allow_all", // 工具直执行，聚焦链路本身
      },
      null,
      2
    )
  );
  fs.writeFileSync(
    path.join(dir, "ai_config.json"),
    JSON.stringify(
      {
        providers: [
          {
            id: "actor-provider",
            name: "AI-Actor",
            protocol: "openai",
            base_url: `http://127.0.0.1:${UP_PORT}/v1`,
            api_key: "sk-actor",
            model: "actor-1",
            active: true,
          },
        ],
      },
      null,
      2
    )
  );

  await new Promise((r) => upstream.listen(UP_PORT, "127.0.0.1", r));
  const proc = spawn(BIN, [], {
    env: { ...process.env, BIT_DATA_DIR: dir, BIT_HEADLESS: "1", BIT_NO_GUARDIAN: "1" },
    stdio: ["ignore", "ignore", "pipe"],
  });
  let errTail = "";
  proc.stderr.on("data", (d) => (errTail = (errTail + d.toString()).slice(-800)));

  const cleanup = async () => {
    try {
      proc.kill("SIGKILL");
    } catch {}
    await sleep(200);
    upstream.close();
    try {
      fs.rmSync(dir, { recursive: true, force: true });
      fs.rmSync(TMP_DIR, { recursive: true, force: true });
    } catch {}
  };

  if (!(await waitHealth())) {
    console.error(`AI-ACTOR FAIL: BIT 未就绪。stderr:\n${errTail}`);
    await cleanup();
    process.exit(1);
  }

  // 逐场景驱动（每个场景独立会话）
  const cases = [
    ["C1", "ACTOR-C1 请执行一次工具调用并汇报结果"],
    ["C2", "ACTOR-C2 请在同一轮并行发起两个独立调用"],
    ["C3", "ACTOR-C3 请写入文件再用 shell 读回验证"],
    ["C4", "ACTOR-C4 请尝试调用一个工具（允许先失败再纠正）"],
    ["C5", "ACTOR-C5 请执行命令（注意参数要符合 schema）"],
    ["C6", "ACTOR-C6 请汇报你可用的工具能力"],
    ["C7", "ACTOR-C7 请连续执行三轮工具调用后总结"],
  ];
  for (const [id, msg] of cases) {
    try {
      const r = await chat(`actor-${id}`, msg);
      const ok = r.code === 200 && r.reply.includes(`ACTOR-${id}-DONE`);
      record(`${id} 驱动闭环（最终回复）`, ok, `code=${r.code} reply=「${r.reply.slice(0, 50)}」`);
    } catch (e) {
      record(`${id} 驱动闭环（最终回复）`, false, String(e));
    }
  }

  const pass = results.filter((r) => r.ok).length;
  console.log(`\n${pass}/${results.length} 通过`);
  await cleanup();
  process.exit(pass === results.length ? 0 : 1);
})().catch(async (e) => {
  console.error("AI-ACTOR FAILED:", e);
  process.exit(1);
});
