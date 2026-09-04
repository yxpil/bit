#!/usr/bin/env node
// bit-agent 启动器：定位 install.js 下载的应用二进制并启动
const { spawnSync } = require("child_process");
const fs = require("fs");
const os = require("os");
const path = require("path");

const HOME = path.join(os.homedir(), ".bit-agent");

function locate() {
  const manifestPath = path.join(HOME, "manifest.json");
  if (fs.existsSync(manifestPath)) {
    const m = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
    if (fs.existsSync(m.binary)) return m.binary;
  }
  // 兜底：常见位置
  const candidates = [
    path.join(HOME, "bin", process.platform === "win32" ? "BIT.exe" : "bit"),
    "/Applications/BIT.app/Contents/MacOS/bit",
  ];
  for (const c of candidates) if (fs.existsSync(c)) return c;
  return null;
}

function install() {
  console.log("[bit-agent] 未找到应用，开始下载...");
  const r = spawnSync(process.execPath, [path.join(__dirname, "..", "scripts", "install.js")], { stdio: "inherit" });
  if (r.status !== 0) {
    console.error("[bit-agent] 下载失败，请到 https://github.com/yxpil/OpenBit/releases 手动下载");
    process.exit(r.status || 1);
  }
  return locate();
}

const binary = locate() || install();
if (!binary) {
  console.error("[bit-agent] 无法定位 BIT 应用，请到 https://github.com/yxpil/OpenBit/releases 手动下载");
  process.exit(1);
}

const args = process.argv.slice(2);
if (process.platform === "darwin" && binary.includes("/Contents/MacOS/")) {
  // macOS：直接执行 app 内二进制
  const r = spawnSync(binary, args, { stdio: "inherit" });
  process.exit(r.status || 0);
}
const r = spawnSync(binary, args, { stdio: "inherit" });
process.exit(r.status || 0);
