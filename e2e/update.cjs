// 自动更新全流程 e2e（隔离数据目录 + BIT_FAKE_UPDATE_URL 注入 mock 更新源）：
// 1) 检测：latest.json → has_update / 无更新两个分支
// 2) 下载：/api/update/download 落盘 upgrade/、幂等缓存（重复调用不重复下载）
// 3) 状态：/api/update/status → downloaded
// 4) 启动自动下载：BIT 启动后台任务 6 秒后静默拉包（不发任何 API 请求也应完成）
// 用法：node e2e/update.cjs [BIT 二进制路径]
const { spawn, execSync } = require("child_process");
const http = require("http");
const fs = require("fs");
const os = require("os");
const net = require("net");
const path = require("path");

const BIN = process.argv[2] || path.join(__dirname, "../src-tauri/target/debug/bit");
const MOCK_PORT = 9903;
const API_PORT = 8766;
const KEY = "bit_e2e_update_test_key";
const ASSET_BYTES = 2 * 1024 * 1024;
const results = [];
const record = (name, ok, detail) => {
  results.push({ name, ok, detail });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? `  ${detail}` : ""}`);
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ── mock 更新源：latest.json（新版/旧版两个路径）+ 安装包 ──
let assetHits = 0;
const ASSET = Buffer.alloc(ASSET_BYTES, 7);
const assetsMap = {
  "windows-x64": `http://127.0.0.1:${MOCK_PORT}/asset.bin`,
  "windows-arm64": `http://127.0.0.1:${MOCK_PORT}/asset.bin`,
  "macos-arm64": `http://127.0.0.1:${MOCK_PORT}/asset.bin`,
  "macos-x64": `http://127.0.0.1:${MOCK_PORT}/asset.bin`,
  "linux-x64": `http://127.0.0.1:${MOCK_PORT}/asset.bin`,
  "linux-arm64": `http://127.0.0.1:${MOCK_PORT}/asset.bin`,
};
const mock = http
  .createServer((req, res) => {
    const latest = (ver) => {
      res.writeHead(200, { "Content-Type": "application/json" });
      res.end(JSON.stringify({ version: ver, notes: "e2e mock update", url: "https://osbt.space", assets: assetsMap }));
    };
    if (req.url === "/latest.json") return latest("999.0.0");
    if (req.url === "/old/latest.json") return latest("0.0.1");
    if (req.url === "/asset.bin") {
      assetHits++;
      res.writeHead(200, { "Content-Type": "application/octet-stream", "Content-Length": ASSET_BYTES });
      return res.end(ASSET);
    }
    res.writeHead(404);
    res.end();
  })
  .listen(MOCK_PORT, "127.0.0.1");

function portOpen(port) {
  return new Promise((resolve) => {
    const s = net.connect({ host: "127.0.0.1", port, timeout: 1500 });
    s.on("connect", () => { s.destroy(); resolve(true); });
    s.on("error", () => resolve(false));
    s.on("timeout", () => { s.destroy(); resolve(false); });
  });
}

function api(port, method, p) {
  return new Promise((resolve, reject) => {
    const req = http.request(
      { host: "127.0.0.1", port, path: p, method, headers: { Authorization: `Bearer ${KEY}`, "X-Access-Password": "12345678" }, timeout: 30000 },
      (res) => { let b = ""; res.on("data", (c) => (b += c)); res.on("end", () => resolve({ code: res.statusCode, body: b })); }
    );
    req.on("error", reject);
    req.end();
  });
}

function launchBit(dir, fakeUrl) {
  fs.mkdirSync(dir, { recursive: true });
  // 预写 config.json 开启远程访问（全新目录默认 remote_enabled=false，不会监听端口）
  fs.writeFileSync(
    path.join(dir, "config.json"),
    JSON.stringify({ remote_enabled: true, host: "127.0.0.1", port: API_PORT, client_key: KEY, password_enabled: false, revision: 1 })
  );
  const proc = spawn(BIN, [], {
    env: { ...process.env, BIT_DATA_DIR: dir, BIT_HEADLESS: "1", BIT_FAKE_UPDATE_URL: fakeUrl },
    stdio: ["ignore", "ignore", "pipe"],
    detached: false,
  });
  let errTail = "";
  proc.stderr.on("data", (d) => { errTail = (errTail + d.toString()).slice(-500); });
  proc.errTail = () => errTail;
  return proc;
}

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

(async () => {
  const conflicts = findConflicts();
  if (conflicts.length) {
    conflicts.forEach((p) => { try { process.kill(p, "SIGTERM"); } catch {} });
    for (let i = 0; i < 10; i++) { await sleep(300); if (!findConflicts().length) break; }
    findConflicts().forEach((p) => { try { process.kill(p, "SIGKILL"); } catch {} });
    await sleep(500);
  }

  // ── U1-U4：有更新的全流程 ──
  {
    const dir = path.join(os.tmpdir(), `bit-update-${Date.now()}`);
    const proc = launchBit(dir, `http://127.0.0.1:${MOCK_PORT}/latest.json`);
    try {
      let listening = false;
      for (let i = 0; i < 24 && !listening; i++) { await sleep(500); listening = await portOpen(API_PORT); }
      record("U0 instance-up", listening, proc.exitCode !== null ? `提前退出 ${proc.errTail()}` : `port=${API_PORT}`);

      const chk = await api(API_PORT, "GET", "/api/update/check");
      let chkOk = false;
      try {
        const j = JSON.parse(chk.body);
        chkOk = chk.code === 200 && j.has_update === true && j.latest === "999.0.0" && j.current === "0.4.9";
      } catch {}
      record("U1 check-has-update", chkOk, `code=${chk.code} body=${chk.body.slice(0, 120)}`);

      const dl = await api(API_PORT, "POST", "/api/update/download");
      let dlOk = false, fileName = "";
      try {
        const j = JSON.parse(dl.body);
        dlOk = dl.code === 200 && j.state === "downloaded";
        fileName = j.file || "";
      } catch {}
      const fileOk = fileName && fs.existsSync(fileName) && fs.statSync(fileName).size === ASSET_BYTES;
      record("U2 download-asset", dlOk && fileOk, `code=${dl.code} file=${fileName}`);

      // 幂等缓存：再次下载不得重新拉包
      const hitsBefore = assetHits;
      await api(API_PORT, "POST", "/api/update/download");
      record("U3 download-cached", assetHits === hitsBefore, `hits ${hitsBefore}→${assetHits}`);

      const st = await api(API_PORT, "GET", "/api/update/status");
      let stOk = false;
      try {
        const j = JSON.parse(st.body);
        stOk = st.code === 200 && j.downloaded === true && j.update.version === "999.0.0";
      } catch {}
      record("U4 status-downloaded", stOk, `code=${st.code} body=${st.body.slice(0, 120)}`);
    } catch (e) {
      record("U1-U4", false, e.message);
    } finally {
      proc.kill();
      await sleep(500);
      fs.rmSync(dir, { recursive: true, force: true });
    }
  }

  // ── U5：无更新分支（旧版本源）──
  {
    const dir = path.join(os.tmpdir(), `bit-update-none-${Date.now()}`);
    const proc = launchBit(dir, `http://127.0.0.1:${MOCK_PORT}/old/latest.json`);
    try {
      let listening = false;
      for (let i = 0; i < 24 && !listening; i++) { await sleep(500); listening = await portOpen(API_PORT); }
      if (!listening) { record("U5 no-update", false, proc.exitCode !== null ? proc.errTail() : "端口未监听"); }
      else {
        const chk = await api(API_PORT, "GET", "/api/update/check");
        const dl = await api(API_PORT, "POST", "/api/update/download");
        let noUp = false;
        try {
          const c = JSON.parse(chk.body);
          const d = JSON.parse(dl.body);
          noUp = c.has_update === false && dl.code === 200 && d.state === "none";
        } catch {}
        record("U5 no-update", noUp, `check=${chk.body.slice(0, 100)} dl=${dl.body.slice(0, 60)}`);
      }
    } finally {
      proc.kill();
      await sleep(500);
      fs.rmSync(dir, { recursive: true, force: true });
    }
  }

  // ── U6：启动自动下载（后台 6 秒任务，不发任何 API 请求）──
  {
    const dir = path.join(os.tmpdir(), `bit-update-auto-${Date.now()}`);
    const proc = launchBit(dir, `http://127.0.0.1:${MOCK_PORT}/latest.json`);
    try {
      let st = null;
      const stateFile = path.join(dir, "upgrade", "state.json");
      for (let i = 0; i < 30; i++) {
        await sleep(1000);
        if (proc.exitCode !== null) break;
        try { st = JSON.parse(fs.readFileSync(stateFile, "utf8")); if (st.state === "downloaded") break; } catch {}
      }
      const autoOk = st && st.state === "downloaded" && st.version === "999.0.0";
      record("U6 startup-auto-download", autoOk, autoOk ? "" : proc.exitCode !== null ? proc.errTail() : "state.json 未生成");
    } finally {
      proc.kill();
      await sleep(500);
      fs.rmSync(dir, { recursive: true, force: true });
    }
  }

  mock.close();
  const pass = results.filter((x) => x.ok).length;
  console.log(`\n==== ${pass}/${results.length} passed ====`);
  process.exit(pass === results.length ? 0 : 1);
})();
