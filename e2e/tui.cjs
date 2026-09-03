// 简约 TUI E2E：`bit tui` 终端模式功能完整性 + 与桌面端同数据目录并行运行无冲突
// 覆盖：/help 对话 工具调用 会话管理 记忆 install-cli 桌面端/TUI 同跑
// 用法：mock-ai(9901) 就绪后：node e2e/tui.cjs [BIT 二进制路径]
const { spawn, execSync } = require("child_process");
const http = require("http");
const fs = require("fs");
const os = require("os");
const net = require("net");
const path = require("path");

const BIN = process.argv[2] || path.join(__dirname, "../src-tauri/target/release/bit");
const PORT = 8611; // 桌面端实例远程访问端口（避开默认 8600，防止撞上日常实例）
const results = [];
const record = (name, ok, detail) => {
  results.push({ name, ok, detail });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? `  ${detail}` : ""}`);
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const tail = (out) => out.slice(-400).replace(/\n+/g, " | ");

function portOpen(port) {
  return new Promise((resolve) => {
    const s = net.connect({ host: "127.0.0.1", port, timeout: 1500 });
    s.on("connect", () => { s.destroy(); resolve(true); });
    s.on("error", () => resolve(false));
    s.on("timeout", () => { s.destroy(); resolve(false); });
  });
}

function get(port, p) {
  return new Promise((resolve, reject) => {
    const req = http.request({ host: "127.0.0.1", port, path: p, method: "GET", timeout: 5000 },
      (res) => { let b = ""; res.on("data", (c) => (b += c)); res.on("end", () => resolve({ code: res.statusCode, body: b })); });
    req.on("error", reject);
    req.on("timeout", () => { req.destroy(); reject(new Error("timeout")); });
    req.end();
  });
}

// 运行中同路径 BIT 实例的 PID 列表（避免日常实例的单实例保护干扰测试）
function findConflicts() {
  let realBin = BIN;
  try { realBin = fs.realpathSync(BIN); } catch {}
  try {
    const out = execSync("ps -axo pid=,command=", { encoding: "utf8" });
    return out
      .split("\n")
      .map((l) => {
        const m = l.trim().match(/^(\d+)\s+(\S+)/);
        if (!m) return 0;
        let cmd = m[2];
        try { cmd = fs.realpathSync(cmd); } catch {}
        return cmd === realBin ? parseInt(m[1], 10) : 0;
      })
      .filter(Boolean);
  } catch {
    return [];
  }
}

// 启动 TUI：返回 { proc, out, send, close, waitExit }
// out 持续累积 stdout；send(line) 写入一行；waitExit 等待进程退出（默认超时强杀）
function launchTui(dir, extraEnv = {}) {
  const proc = spawn(BIN, ["tui"], {
    // NO_AT_BRIDGE：Linux 下跳过 AT-SPI 无障碍总线查找（CI 无 dbus 时该查找阻塞 ~25s，
    // 会吞掉首个测试标记导致 T17/T19a 假失败；对 macOS/Windows 无影响）
    env: { ...process.env, NO_AT_BRIDGE: "1", BIT_DATA_DIR: dir, ...extraEnv },
    stdio: ["pipe", "pipe", "pipe"],
  });
  let out = "";
  proc.stdout.on("data", (d) => (out += d.toString()));
  proc.stderr.on("data", (d) => (out += d.toString()));
  // 进程退出后再写 stdin 会触发 write EOF——吞掉避免测试脚本崩溃
  proc.stdin.on("error", () => {});
  proc.on("error", () => {});
  const send = (line) => proc.stdin.write(line + "\n");
  const close = () => proc.stdin.end();
  // 超时不抛异常：SIGKILL 后返回 -1，由断言展示已收集的输出便于诊断
  const waitExit = (timeout = 60000) =>
    new Promise((resolve) => {
      const t = setTimeout(() => {
        out += `\n[waitExit 超时 ${timeout}ms，SIGKILL]\n`;
        try { proc.kill("SIGKILL"); } catch {}
        resolve(-1);
      }, timeout);
      proc.on("exit", (code) => { clearTimeout(t); resolve(code); });
    });
  return { proc, get out() { return out; }, send, close, waitExit };
}

async function main() {
  // 停掉同路径旧实例（单实例保护会让新实例秒退）
  const pids = findConflicts();
  if (pids.length) {
    try { execSync(`kill ${pids.join(" ")}`); } catch {}
    await sleep(1000);
  }

  // 隔离数据目录 + 指向 mock-ai 的 AI 配置（TUI 与桌面端共用）
  const DIR = fs.mkdtempSync(path.join(os.tmpdir(), "bit-tui-e2e-"));
  fs.writeFileSync(
    path.join(DIR, "ai_config.json"),
    JSON.stringify({ providers: [{ id: "mock", name: "mock", protocol: "openai", base_url: "http://127.0.0.1:9901/v1", api_key: "e2e", model: "mock", active: true }] })
  );

  // ── T1 /help：命令清单完整 ──
  {
    const tui = launchTui(DIR);
    tui.send("/help");
    tui.send("/quit");
    const code = await tui.waitExit();
    const out = tui.out;
    const ok = code === 0 && ["/sessions", "/new", "/use", "/tools", "/mem", "/install-cli"].every((c) => out.includes(c));
    record("T1 /help 命令清单", ok, ok ? `exit=${code}` : tail(out));
  }

  // ── T2 对话 + T3 工具调用 + T4 会话 + T5 记忆（一个 REPL 会话内顺序执行） ──
  {
    const tui = launchTui(DIR);
    tui.send("TUI-MOCK-GREETING");
    tui.send("E2E-CMD-SHELL");
    tui.send("/new tui-test-session");
    tui.send("/sessions");
    tui.send("/mem TUI-MEM-ITEM-9527");
    tui.send("/mems");
    tui.send("/quit");
    const code = await tui.waitExit(120000);
    const out = tui.out;
    record("T2 对话(mock 默认回复)", code === 0 && out.includes("好的。"), `exit=${code}`);
    const toolOk = /\[tool\] shell \{"command":"echo e2e-shell-ok"\} → 成功/.test(out) && /E2E-FINAL-OK/.test(out);
    record("T3 工具调用全链路", toolOk, toolOk ? "" : out.slice(-400));
    const sessOk = out.includes("已创建会话") && /tui-test-session/.test(out);
    record("T4 会话新建/列表", sessOk, "");
    const memOk = out.includes("已沉淀记忆") && out.includes("TUI-MEM-ITEM-9527");
    record("T5 记忆沉淀/查看", memOk, "");
  }

  // ── T6 /install-cli：macOS/Linux 符号链接 / Windows bit.cmd 启动器 ──
  {
    const CLI_DIR = fs.mkdtempSync(path.join(os.tmpdir(), "bit-tui-cli-"));
    const tui = launchTui(DIR, { BIT_CLI_DIR: CLI_DIR });
    tui.send("/install-cli");
    tui.send("/quit");
    await tui.waitExit();
    let ok = false, detail = "";
    if (process.platform === "win32") {
      // Windows 写 bit.cmd 启动脚本（内含 exe 路径与 tui 参数）
      const launcher = path.join(CLI_DIR, "bit.cmd");
      try {
        const content = fs.readFileSync(launcher, "utf8");
        ok = /tui/.test(content) && content.toLowerCase().includes("bit");
        detail = content.trim().split(/\r?\n/).pop() || "";
      } catch (e) { detail = e.message; }
    } else {
      const link = path.join(CLI_DIR, "bit");
      try {
        ok = fs.realpathSync(link) === fs.realpathSync(BIN);
        detail = fs.readlinkSync(link);
      } catch (e) { detail = e.message; }
    }
    record("T6 /install-cli 安装", ok, detail);
  }

  // ── T8 非法命令与用法错误：各自得到提示，REPL 存活继续 ──
  {
    const tui = launchTui(DIR);
    tui.send("/foobar");
    tui.send("/use");
    tui.send("/use deadbeef-0000");
    tui.send("/mem");
    tui.send("/install-cli-extra"); // 带参数的未知命令
    tui.send("/QUIT");              // 大小写不敏感退出
    const code = await tui.waitExit();
    const out = tui.out;
    const ok =
      out.includes("未知命令 /foobar") &&
      out.includes("用法：/use") &&
      out.includes("会话不存在") &&
      out.includes("用法：/mem") &&
      out.includes("未知命令 /install-cli-extra") &&
      code === 0;
    record("T8 非法命令/用法错误", ok, `exit=${code}`);
  }

  // ── T9 空行与纯空白被忽略；T16 /q 变体退出 ──
  {
    const tui = launchTui(DIR);
    tui.send("");
    tui.send("   ");
    tui.send("\t");
    tui.send("/q");
    const code = await tui.waitExit();
    // 空白行不应产生"错误"输出
    const ok = code === 0 && !tui.out.split("BIT TUI")[1]?.includes("错误");
    record("T9 空白输入忽略 + /q 退出", ok, `exit=${code}`);
  }

  // ── T10 超长单行（200KB）+ T11 多字节/emoji 混合 ──
  {
    const tui = launchTui(DIR);
    tui.send("LONG-" + "A".repeat(200 * 1024) + "-END");
    tui.send("你好 🌍 こんにちは 🚀 Привет مرحба");
    tui.send("/quit");
    const code = await tui.waitExit(120000);
    // 回复正常 + 200KB 消息完整落库（TUI 不回显输入，从会话存储验证）
    let longOk = false;
    try {
      const sessions = JSON.parse(fs.readFileSync(path.join(DIR, "sessions.json"), "utf8"));
      const msgs = (sessions.sessions || sessions).flatMap((s) => s.messages || []);
      longOk = msgs.some((m) => typeof m.content === "string" && /^LONG-A+-END$/.test(m.content) && m.content.length === 5 + 200 * 1024 + 4);
    } catch {}
    const ok = code === 0 && tui.out.includes("好的。") && longOk;
    record("T10/11 超长行+多字节", ok, `exit=${code}`);
  }

  // ── T12 损坏的 ai_config.json：启动不崩，提示未配置，命令可用 ──
  {
    const BAD = fs.mkdtempSync(path.join(os.tmpdir(), "bit-tui-bad-"));
    fs.writeFileSync(path.join(BAD, "ai_config.json"), "{corrupted json!!!");
    const tui = launchTui(BAD);
    tui.send("/tools");
    tui.send("/quit");
    const code = await tui.waitExit();
    const out = tui.out;
    const ok = code === 0 && out.includes("AI 尚未配置") && out.includes("shell") && out.includes("Execute a shell command");
    record("T12 损坏 ai_config 容错 + 工具描述英文", ok, `exit=${code}`);
  }

  // ── T13 AI 不可达：报错有提示、不崩溃、REPL 继续可用 ──
  {
    const DEAD = fs.mkdtempSync(path.join(os.tmpdir(), "bit-tui-dead-"));
    fs.writeFileSync(
      path.join(DEAD, "ai_config.json"),
      JSON.stringify({ providers: [{ id: "dead", name: "dead", protocol: "openai", base_url: "http://127.0.0.1:9888/v1", api_key: "x", model: "x", active: true }] })
    );
    const tui = launchTui(DEAD);
    tui.send("TUI-DEAD-ENDPOINT");
    tui.send("/tools"); // 报错后 REPL 必须仍然可用
    tui.send("/quit");
    const code = await tui.waitExit(120000);
    const out = tui.out;
    const ok = code === 0 && out.includes("错误：") && out.includes("shell");
    record("T13 AI 不可达报错恢复", ok, `exit=${code}`);
  }

  // ── T14 EOF（无 /quit）干净退出；T15 命令风暴按序处理 ──
  {
    const tui = launchTui(DIR);
    for (let i = 0; i < 30; i++) tui.send(`/new storm-${i}`);
    tui.close(); // 不发 /quit，直接 EOF
    const code = await tui.waitExit(120000);
    let ok = code === 0;
    // 30 条 /new 全部生效（sessions.json 落盘）
    try {
      const sessions = JSON.parse(fs.readFileSync(path.join(DIR, "sessions.json"), "utf8"));
      const storms = (sessions.sessions || sessions).filter?.((s) => (s.title || "").startsWith("storm-")) || [];
      ok = ok && storms.length >= 30;
    } catch (e) { ok = false; }
    record("T14/15 EOF 退出 + 30 连发风暴", ok, `exit=${code}`);
  }

  // ── T17 模糊格式识别：变体 AI 响应格式动态识别（content 数组/legacy text/output_text/MAX_TOKENS/垃圾体）──
  {
    const tui = launchTui(DIR);
    // 三种变体格式：严格路径解析不到 → 模糊识别应成功取到正文
    for (const [marker, expect] of [
      ["E2E-FMT-ARRAY", "E2E-FMT-ARRAY-OK"],
      ["E2E-FMT-LEGACY", "E2E-FMT-LEGACY-OK"],
      ["E2E-FMT-OUTTEXT", "E2E-FMT-OUTTEXT-OK"],
    ]) {
      tui.send(marker);
      for (let i = 0; i < 60 && !tui.out.includes(expect); i++) await sleep(500);
      const ok = tui.out.includes(expect);
      record(`T17 变体格式 ${marker}`, ok, ok ? "" : tail(tui.out));
    }
    // finish_reason 大写变体 MAX_TOKENS：应被归一化识别并显式标注截断
    tui.send("E2E-FMT-MAXTOK");
    for (let i = 0; i < 60 && !tui.out.includes("E2E-FMT-MAXTOK-OK"); i++) await sleep(500);
    const maxOk = tui.out.includes("E2E-FMT-MAXTOK-OK") && tui.out.includes("截断");
    record("T17 MAX_TOKENS 截断归一化", maxOk, maxOk ? "" : tail(tui.out));
    // 完全不可解析的响应体：报错有提示、REPL 存活、可继续退出
    tui.send("E2E-FMT-GARBAGE");
    for (let i = 0; i < 60 && !tui.out.includes("错误："); i++) await sleep(500);
    tui.send("/tools");
    tui.send("/quit");
    const code = await tui.waitExit(60000);
    const garbOk = code === 0 && tui.out.includes("错误：") && tui.out.includes("Execute a shell command");
    record("T17 垃圾响应容错", garbOk, `exit=${code}`);
  }

  // ── T18 AI 行为模拟：真实模型的高频调用习惯 ──
  {
    const tui = launchTui(DIR);
    // 18a 参数缺失 → 错误反馈 → 自我纠正
    tui.send("E2E-AI-RETRY");
    for (let i = 0; i < 60 && !tui.out.includes("E2E-AI-RETRY-OK"); i++) await sleep(500);
    const retryOk = tui.out.includes("E2E-AI-RETRY-OK") && tui.out.includes("失败");
    record("T18a 参数缺失自纠", retryOk, retryOk ? "" : tail(tui.out));
    // 18b 幻觉工具 → 错误反馈 → 换真实工具
    tui.send("E2E-AI-NOTOOL");
    for (let i = 0; i < 60 && !tui.out.includes("E2E-AI-NOTOOL-OK"); i++) await sleep(500);
    const notoolOk = tui.out.includes("E2E-AI-NOTOOL-OK") && tui.out.includes("失败");
    record("T18b 幻觉工具自纠", notoolOk, notoolOk ? "" : tail(tui.out));
    // 18c 围栏 + 散文包裹的工具调用
    tui.send("E2E-AI-FENCED");
    for (let i = 0; i < 60 && !tui.out.includes("E2E-AI-FENCED-OK"); i++) await sleep(500);
    const fencedOk = tui.out.includes("E2E-AI-FENCED-OK");
    record("T18c 围栏调用识别", fencedOk, fencedOk ? "" : tail(tui.out));
    // 18d 一回合三个工具调用（并发上限 16 内全执行，结果按序回显）
    tui.send("E2E-AI-MULTI");
    for (let i = 0; i < 60 && !tui.out.includes("E2E-AI-MULTI-C"); i++) await sleep(500);
    const multiOk = ["E2E-AI-MULTI-A", "E2E-AI-MULTI-B", "E2E-AI-MULTI-C"].every((m) => tui.out.includes(m));
    record("T18d 一回合多调用", multiOk, multiOk ? "" : tail(tui.out));
    tui.send("/quit");
    await tui.waitExit(60000);
  }

  // ── T19 流式边界：多字节跨 chunk / 200 小块 / 流中输入模拟 / 立即 500 ──
  {
    const tui = launchTui(DIR);
    // 19a 多字节字符被 TCP 从中间切开：解码必须按完整行进行，emoji/中文不得损坏
    tui.send("E2E-STREAM-MULTIBYTE");
    for (let i = 0; i < 60 && !tui.out.includes("BIT-STREAM-OK"); i++) await sleep(500);
    await sleep(300);
    const mbOk = tui.out.includes("你好🌍BIT-STREAM-OK") && !tui.out.includes("�");
    record("T19a 多字节跨 chunk 流式", mbOk, mbOk ? "" : tail(tui.out));
    // 19b 200 个小 chunk 连发：内容必须完整有序
    tui.send("E2E-STREAM-MANY");
    for (let i = 0; i < 90 && !tui.out.includes("c199;"); i++) await sleep(500);
    const manyOk = Array.from({ length: 40 }, (_, k) => `c${k * 5};`).every((m) => tui.out.includes(m)) && tui.out.includes("c199;");
    record("T19b 200 chunk 完整性", manyOk, manyOk ? "" : tail(tui.out));
    // 19c 流中输入模拟：长流式输出进行中用户继续输入 → 两条消息都得到处理
    tui.send("E2E-CMD-LONG");
    await sleep(120); // 长流刚启动
    tui.send("E2E-CMD-SHELL");
    let guard = 0;
    for (; guard < 90 && !(tui.out.includes("L0049") && tui.out.includes("e2e-shell-ok")); guard++) await sleep(500);
    const bothOk = tui.out.includes("L0049") && tui.out.includes("e2e-shell-ok") && !tui.out.includes("�");
    record("T19c 流中输入模拟", bothOk, bothOk ? "" : tail(tui.out));
    // 19d 流式请求立即 500：报错反馈，REPL 存活
    tui.send("E2E-STREAM-ERR");
    for (let i = 0; i < 60 && !tui.out.includes("错误："); i++) await sleep(500);
    tui.send("/tools");
    tui.send("/quit");
    const code = await tui.waitExit(60000);
    const errOk = code === 0 && tui.out.includes("HTTP 500") && tui.out.includes("Execute a shell command");
    record("T19d 流式 500 容错", errOk, errOk ? "" : `exit=${code}`);
  }

  // ── T7 桌面端 + TUI 同数据目录并行：互不干扰 ──
  {
    // 桌面端配置：开启远程访问（8611），TUI 启动不应抢掉该端口也不应被单实例顶掉
    fs.writeFileSync(
      path.join(DIR, "config.json"),
      JSON.stringify({ remote_enabled: true, host: "127.0.0.1", port: PORT, client_key: "bit_e2e_tui_key", password_enabled: false, revision: 1 })
    );
    const desktop = spawn(BIN, [], {
      env: { ...process.env, BIT_DATA_DIR: DIR, BIT_HEADLESS: "1" },
      stdio: ["ignore", "ignore", "pipe"],
    });
    let deskErr = "";
    desktop.stderr.on("data", (d) => (deskErr += d.toString()));

    // 等桌面端 HTTP 服务就绪（xvfb/无 GPU 环境 webkit 首次启动可达数十秒）
    let up = false;
    for (let i = 0; i < 120; i++) {
      try { const r = await get(PORT, "/api/health"); if (r.code === 200) { up = true; break; } } catch {}
      await sleep(500);
    }
    record("T7a 桌面端启动并监听", up && !desktop.killed, up ? `port=${PORT}` : (deskErr.slice(-200) || "60s 内未监听"));

    if (up) {
      // TUI 与桌面端同时运行：对话仍可用（共用数据目录无冲突）
      const tui = launchTui(DIR);
      tui.send("TUI-PARALLEL-CHECK");
      // 轮询等待回复（最多 30s）
      for (let i = 0; i < 60 && !tui.out.includes("好的。"); i++) await sleep(500);
      tui.send("/quit");
      const code = await tui.waitExit(60000);
      const tuiOk = code === 0 && tui.out.includes("好的。");
      record("T7b 并行时 TUI 对话可用", tuiOk, `exit=${code}`);

      // TUI 退出后桌面端仍健在（TUI 没有抢实例/抢端口）
      let alive = false;
      try { const r = await get(PORT, "/api/health"); alive = r.code === 200; } catch {}
      record("T7c TUI 退出后桌面端健在", alive && !desktop.killed, "");
    }

    // 桌面端再起的反向验证：TUI 已退出，此时新桌面端能被单实例接管（健康检查可用）
    // （T7b/c 已覆盖核心冲突面，此处清理）
    try { desktop.kill(); } catch {}
    await sleep(500);
  }

  const pass = results.filter((r) => r.ok).length;
  console.log(`\n${pass}/${results.length} 通过`);
  process.exit(pass === results.length ? 0 : 1);
}

main().catch((e) => { console.error("E2E 异常:", e); process.exit(1); });
