#!/usr/bin/env node
// postinstall：按平台/架构下载对应 BIT 产物到 ~/.bit-agent
const { execFileSync } = require("child_process");
const fs = require("fs");
const https = require("https");
const os = require("os");
const path = require("path");

const VERSION = require("../package.json").version;
const BASE = `https://github.com/yxpil/bit/releases/download/v${VERSION}`;
const HOME = path.join(os.homedir(), ".bit-agent");

function assetFor() {
  const p = process.platform, a = process.arch;
  if (p === "darwin" && a === "arm64") return { file: `BIT_${VERSION}_aarch64.dmg`, kind: "dmg" };
  if (p === "darwin" && a === "x64") return { file: `BIT_${VERSION}_x64.dmg`, kind: "dmg" };
  if (p === "linux" && a === "x64") return { file: `BIT_${VERSION}_amd64.AppImage`, kind: "appimage" };
  if (p === "linux" && a === "arm64") return { file: `BIT_${VERSION}_aarch64.AppImage`, kind: "appimage" };
  if (p === "linux" && a === "riscv64") return { file: `bit_${VERSION}_riscv64.tar.gz`, kind: "targz", bin: "bit" };
  if (p === "linux" && a === "loong64") return { file: `bit_${VERSION}_loongarch64.tar.gz`, kind: "targz", bin: "bit" };
  if (p === "win32" && a === "x64") return { file: `BIT_${VERSION}_x64-portable.zip`, kind: "zip", bin: "BIT.exe" };
  return null;
}

function download(url, dest) {
  return new Promise((resolve, reject) => {
    const get = (u, redirects) => {
      if (redirects > 5) return reject(new Error("too many redirects"));
      https.get(u, { headers: { "User-Agent": "bit-agent-npm" } }, (res) => {
        if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
          res.resume();
          return get(new URL(res.headers.location, u).href, redirects + 1);
        }
        if (res.statusCode !== 200) return reject(new Error(`HTTP ${res.statusCode}`));
        const f = fs.createWriteStream(dest);
        res.pipe(f);
        f.on("finish", () => f.close(resolve));
        f.on("error", reject);
      }).on("error", reject);
    };
    get(url, 0);
  });
}

async function main() {
  const asset = assetFor();
  if (!asset) {
    console.log(`[bit-agent] 平台 ${process.platform}/${process.arch} 暂无预编译包，请到 ${BASE.replace(`/v${VERSION}`, "/latest")} 下载`);
    return;
  }
  fs.mkdirSync(path.join(HOME, "bin"), { recursive: true });
  const marker = path.join(HOME, "manifest.json");
  if (fs.existsSync(marker)) {
    try {
      const m = JSON.parse(fs.readFileSync(marker, "utf8"));
      if (m.version === VERSION && fs.existsSync(m.binary)) {
        console.log(`[bit-agent] ${VERSION} 已安装，跳过下载`);
        return;
      }
    } catch {}
  }
  const tmp = path.join(HOME, asset.file);
  console.log(`[bit-agent] 下载 ${asset.file} ...`);
  await download(`${BASE}/${asset.file}`, tmp);

  let binary;
  if (asset.kind === "dmg") {
    const mnt = path.join(HOME, "mnt");
    fs.rmSync(mnt, { recursive: true, force: true });
    fs.mkdirSync(mnt, { recursive: true });
    execFileSync("hdiutil", ["attach", "-nobrowse", "-readonly", "-mountpoint", mnt, tmp]);
    try {
      fs.rmSync(path.join(HOME, "BIT.app"), { recursive: true, force: true });
      fs.cpSync(path.join(mnt, "BIT.app"), path.join(HOME, "BIT.app"), { recursive: true });
    } finally {
      try { execFileSync("hdiutil", ["detach", mnt, "-quiet"]); } catch {}
    }
    fs.rmSync(tmp, { force: true });
    binary = path.join(HOME, "BIT.app/Contents/MacOS/bit");
  } else if (asset.kind === "appimage") {
    const dest = path.join(HOME, "bin", "bit.AppImage");
    fs.renameSync(tmp, dest);
    fs.chmodSync(dest, 0o755);
    binary = dest;
  } else if (asset.kind === "targz") {
    execFileSync("tar", ["-xzf", tmp, "-C", path.join(HOME, "bin")]);
    fs.rmSync(tmp, { force: true });
    binary = path.join(HOME, "bin", asset.bin);
    fs.chmodSync(binary, 0o755);
  } else if (asset.kind === "zip") {
    // Windows 10+ 自带 bsdtar 可解 zip
    execFileSync("tar", ["-xf", tmp, "-C", path.join(HOME, "bin")]);
    fs.rmSync(tmp, { force: true });
    binary = path.join(HOME, "bin", asset.bin);
  }

  fs.writeFileSync(marker, JSON.stringify({ version: VERSION, binary, platform: process.platform, arch: process.arch }, null, 2));
  console.log(`[bit-agent] 安装完成：${binary}`);
  console.log("[bit-agent] 运行 `bit-agent` 启动 BIT");
}

main().catch((e) => {
  console.error("[bit-agent] 安装失败:", e.message);
  console.error("[bit-agent] 可到 https://github.com/yxpil/bit/releases 手动下载");
  process.exit(0); // 不阻塞 npm 安装
});
