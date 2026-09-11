// yxpil · BIT — UI 冒烟测试（发布流水线闸门，防黑屏包）
// 背景：v0.5.14 AiSettingsPage 漏声明 elevErr，渲染期 ReferenceError 使 React 整树卸载 →
// Windows 安装后黑屏且已流出。本脚本在 CI 构建后、上传前运行：
//   无头启动 BIT（窗口隐藏但 WebView 照常渲染）→ 前端挂载成功后调 ui_mounted 落审计
//   → 轮询 audit.json 出现 "ui.mounted" 即通过；缺席 = 渲染树崩溃 = 构建被拦截。
// 用法：node e2e/smoke-ui.cjs <BIT二进制路径> [超时秒数=90]
const { spawn, execSync } = require("child_process");
const fs = require("fs");
const os = require("os");
const path = require("path");

const BIN = process.argv[2];
const TIMEOUT_S = Number(process.argv[3]) || 90;
if (!BIN || !fs.existsSync(BIN)) {
  console.error(`smoke-ui: binary not found: ${BIN || "(missing argument)"}`);
  process.exit(2);
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// 同二进制残留实例（主进程 + --bit-guardian 守护进程）会让单实例保护令测试实例启动即退。
// CI 上无实例，此处为空操作；本地调试时清理残留测试进程（按 realpath 匹配，不碰其他安装版）。
let realBin = BIN;
try { realBin = fs.realpathSync(BIN); } catch {}
function sameBinaryPids() {
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
async function preflight() {
  const pre = sameBinaryPids();
  if (!pre.length) return;
  for (const p of pre) { try { process.kill(p, "SIGKILL"); } catch {} }
  await sleep(1500); // 等守护进程互拉尘埃落定
  for (const p of sameBinaryPids()) { try { process.kill(p, "SIGKILL"); } catch {} }
  console.log(`smoke-ui: cleaned ${pre.length} leftover same-binary instance(s)`);
}

(async () => {
  await preflight();

  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "bit-smoke-"));
  const proc = spawn(BIN, [], {
    env: {
      ...process.env,
      BIT_DATA_DIR: dir, // 隔离数据目录，不碰日常实例
      BIT_HEADLESS: "1", // 窗口隐藏不弹前台（WebView 仍正常渲染）
      BIT_NO_GUARDIAN: "1", // 测试实例不布防守护进程：结束后杀掉不会被重新拉起
    },
    stdio: ["ignore", "ignore", "pipe"],
    detached: false,
  });

  let errTail = "";
  proc.stderr.on("data", (d) => { errTail = (errTail + d.toString()).slice(-800); });
  let exited = null; // 提前退出（崩溃/启动失败）则立即判失败，不等超时
  proc.on("exit", (code) => { exited = code; });

  // audit.json 每次 record 都全量落盘，轮询文件即可，无需任何 IPC
  const mounted = () => {
    try {
      return fs.readFileSync(path.join(dir, "audit.json"), "utf8").includes("ui.mounted");
    } catch {
      return false;
    }
  };

  try {
    let ok = false;
    const deadline = Date.now() + TIMEOUT_S * 1000;
    while (Date.now() < deadline) {
      if (exited !== null) break;
      if (mounted()) { ok = true; break; }
      await sleep(500);
    }

    try { proc.kill("SIGKILL"); } catch {}
    await sleep(300);
    // 清理临时目录：慢 runner（如 arm）上 BIT 的 toolhomes venv 可能还有写入尾巴，
    // 立即 rm 会 ENOTEMPTY —— 重试几次；清理失败不应推翻已通过的冒烟结论
    for (let i = 0; i < 5; i++) {
      try { fs.rmSync(dir, { recursive: true, force: true, maxRetries: 3, retryDelay: 200 }); break; }
      catch { await sleep(500 * (i + 1)); }
    }

    if (ok) {
      console.log("SMOKE PASS: ui.mounted in audit log — frontend render tree mounted");
    } else if (exited !== null) {
      console.error(`SMOKE FAIL: BIT exited early (code=${exited}) before ui.mounted. stderr tail:\n${errTail}`);
      process.exitCode = 1;
    } else {
      console.error(`SMOKE FAIL: ui.mounted not found within ${TIMEOUT_S}s (frontend render crashed?). stderr tail:\n${errTail}`);
      process.exitCode = 1;
    }
  } catch (e) {
    try { proc.kill("SIGKILL"); } catch {}
    console.error("smoke-ui FAILED:", e.message);
    process.exitCode = 1;
  }
})();
