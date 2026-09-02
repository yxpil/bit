// 远程访问默认状态专项测试（隔离数据目录，不影响日常使用的实例）：
// 1) 全新数据目录（无 config.json）启动 → 默认 remote_enabled=false（远程访问关闭，不监听端口）
// 2) 用户开启后（配置 remote_enabled=true）→ 端口监听、正确凭据放行、错误 Key 拒绝
// 用法：node e2e/remote-default.cjs [BIT 二进制路径]
const { spawn, execSync } = require("child_process");
const http = require("http");
const fs = require("fs");
const os = require("os");
const net = require("net");
const path = require("path");

const BIN = process.argv[2] || path.join(__dirname, "../src-tauri/target/debug/bit");
const results = [];
const record = (name, ok, detail) => {
  results.push({ name, ok, detail });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? `  ${detail}` : ""}`);
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// 端口是否可建立 TCP 连接（true = 有服务监听）
function portOpen(port) {
  return new Promise((resolve) => {
    const s = net.connect({ host: "127.0.0.1", port, timeout: 1500 });
    s.on("connect", () => { s.destroy(); resolve(true); });
    s.on("error", () => resolve(false));
    s.on("timeout", () => { s.destroy(); resolve(false); });
  });
}

function getStatus(port, p, headers) {
  return new Promise((resolve, reject) => {
    const req = http.request({ host: "127.0.0.1", port, path: p, method: "GET", headers, timeout: 5000 },
      (res) => { let b = ""; res.on("data", (c) => (b += c)); res.on("end", () => resolve({ code: res.statusCode, body: b })); });
    req.on("error", reject);
    req.end();
  });
}

// 用隔离数据目录启动一个 BIT 实例（无头模式，不弹窗），返回 { proc, dir }
// stderr 保留尾部用于失败诊断（如单实例冲突、启动异常）
function launchBit(dir) {
  fs.mkdirSync(dir, { recursive: true });
  const proc = spawn(BIN, [], {
    env: { ...process.env, BIT_DATA_DIR: dir, BIT_HEADLESS: "1" },
    stdio: ["ignore", "ignore", "pipe"],
    detached: false,
  });
  let errTail = "";
  proc.stderr.on("data", (d) => { errTail = (errTail + d.toString()).slice(-500); });
  proc.errTail = () => errTail;
  return { proc, dir };
}

// 运行中同路径 BIT 实例的 PID 列表。
// 单实例保护（tauri_plugin_single_instance）下，已有实例会让测试实例启动即退出，
// 导致 config.json 永不生成 —— 测试前必须先停掉已有实例。
// 匹配时对 ps 首列命令路径做 realpath 归一化，兼容相对路径启动的实例。
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

// 进程意外退出（单实例冲突 / 启动失败）的统一报错文案
const diedMsg = (proc, tag) =>
  `${tag} 进程提前退出 code=${proc.exitCode}${proc.errTail() ? ` stderr: ${proc.errTail()}` : ""}`;

(async () => {
  // ── 前置：停掉运行中的 BIT 实例，避免单实例保护顶掉测试实例 ──
  const conflicts = findConflicts();
  if (conflicts.length) {
    console.log(`[setup] 停止 ${conflicts.length} 个运行中的 BIT 实例（避免单实例冲突）: ${conflicts.join(", ")}`);
    conflicts.forEach((p) => { try { process.kill(p, "SIGTERM"); } catch {} });
    for (let i = 0; i < 10; i++) {
      await sleep(300);
      if (!findConflicts().length) break;
    }
    findConflicts().forEach((p) => { try { process.kill(p, "SIGKILL"); } catch {} });
    await sleep(500);
  }

  // ── 场景 1：全新环境默认关闭 ──
  // 启动后读取落盘的 config.json：默认 remote_enabled 必须为 false
  {
    const { proc, dir } = launchBit(path.join(os.tmpdir(), `bit-remote-off-${Date.now()}`));
    try {
      let cfgFile = {};
      for (let i = 0; i < 30; i++) {
        await sleep(500);
        if (proc.exitCode !== null) break; // 启动即挂（如单实例冲突），无需再等
        try {
          cfgFile = JSON.parse(fs.readFileSync(path.join(dir, "config.json"), "utf8"));
          if ("remote_enabled" in cfgFile) break;
        } catch {}
      }
      const off = cfgFile.remote_enabled === false && proc.exitCode === null;
      record("R1 fresh-default-off", off,
        proc.exitCode !== null ? diedMsg(proc, "R1") : `remote_enabled=${cfgFile.remote_enabled}`);
    } catch (e) {
      record("R1 fresh-default-off", false, e.message);
    } finally {
      proc.kill();
      await sleep(500);
      fs.rmSync(dir, { recursive: true, force: true });
    }
  }

  // ── 场景 2：用户开启后可访问，鉴权生效 ──
  // 用独立端口 8765，避免与正在运行的日常实例（8600）冲突
  {
    const KEY = "bit_e2e_remote_default_test_key";
    const port = 8765;
    const dir = path.join(os.tmpdir(), `bit-remote-on-${Date.now()}`);
    const { proc } = launchBit(dir);
    try {
      await sleep(1000); // 等 BIT 先生成完整默认配置
      fs.writeFileSync(
        path.join(dir, "config.json"),
        JSON.stringify({
          remote_enabled: true,
          host: "127.0.0.1",
          port,
          client_key: KEY,
          access_password: "12345678",
          password_enabled: true,
          revision: 1,
          tool_approval: "allow_all",
        })
      );
      // BIT 启动时只读一次配置，改配置需重启才生效
      proc.kill();
      await sleep(1000);
      const { proc: again } = launchBit(dir);
      try {
        let listening = false;
        for (let i = 0; i < 20; i++) {
          await sleep(500);
          if (again.exitCode !== null) break; // 启动即挂，无需再等
          if (await portOpen(port)) { listening = true; break; }
        }
        record("R2 enabled-listening", listening,
          again.exitCode !== null ? diedMsg(again, "R2") : `port=${port}`);
        if (listening) {
          // /api/health 免鉴权（探活端点），用 /api/tools 验证双重认证
          const ok = await getStatus(port, "/api/tools", { Authorization: `Bearer ${KEY}`, "X-Access-Password": "12345678" });
          const bad = await getStatus(port, "/api/tools", { Authorization: "Bearer bit_wrong", "X-Access-Password": "12345678" });
          record("R2 auth-guard", ok.code === 200 && bad.code === 401, `ok=${ok.code} badKey=${bad.code}`);
        }
      } finally {
        again.kill();
      }
    } catch (e) {
      record("R2 enabled-listening", false, e.message);
    } finally {
      proc.kill();
      await sleep(500);
      fs.rmSync(dir, { recursive: true, force: true });
    }
  }

  const pass = results.filter((x) => x.ok).length;
  console.log(`\n==== ${pass}/${results.length} passed ====`);
  process.exit(pass === results.length ? 0 : 1);
})();
