#!/usr/bin/env python3
"""全渠道自动同步：Homebrew tap / Scoop bucket / APT / pacman / dnf 仓库。
用法: python3 sync_channels.py v0.5.9 [--token GHTOKEN] [--dry-run]
在 GitHub Actions 中由 sync-channels.yml 于 release 工作流完成后自动调用；
也可本地手动运行（--token 缺省时用 GITHUB_TOKEN / gh auth token）。
"""
import argparse
import base64
import gzip
import hashlib
import json
import os
import subprocess
import sys
import tempfile
import time
import urllib.request
from datetime import datetime, timezone
from pathlib import Path

TAP = "yxpil/homebrew-bit"
SCOOP = "yxpil/scoop-bit"
APT = "yxpil/apt-repo"
PACMAN = "yxpil/pacman-repo"
DNF = "yxpil/dnf-repo"
DL = "https://github.com/yxpil/bit/releases/download/{v}/{f}"
# brew/scoop 渠道必需的资产；apt 各架构缺失时跳过不阻塞
BREW_ASSETS = ["BIT_{v}_aarch64.dmg", "BIT_{v}_x64.dmg"]
SCOOP_ASSETS = ["BIT_{v}_x64-portable.zip"]
APT_ARCHES = ["amd64", "arm64", "loongarch64", "riscv64", "ppc64le"]
APT_NAMES = {"amd64": "BIT_{v}_amd64.deb", "arm64": "BIT_{v}_arm64.deb",
             "loongarch64": "bit_{v}_loongarch64.deb", "riscv64": "bit_{v}_riscv64.deb",
             "ppc64le": "bit_{v}_ppc64le.deb"}
RPM_NAMES = {"x86_64": "BIT-{v}-1.x86_64.rpm", "aarch64": "BIT-{v}-1.aarch64.rpm"}
# gen_repos.py（纯 Python 生成器，无系统依赖）位于 packaging/repos/
sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "repos"))
import gen_repos  # noqa: E402


def gh(method, url, token, payload=None, expect=(200, 201)):
    req = urllib.request.Request(f"https://api.github.com{url}", method=method)
    req.add_header("Authorization", f"Bearer {token}")
    req.add_header("Accept", "application/vnd.github+json")
    data = json.dumps(payload).encode() if payload is not None else None
    if data:
        req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req, data) as r:
            body = r.read()
            code = r.status
    except urllib.error.HTTPError as e:
        body, code = e.read(), e.code
    if code not in expect:
        raise RuntimeError(f"{method} {url} -> {code}: {body[:300].decode(errors='replace')}")
    return json.loads(body) if body else None


def download(url, dest: Path):
    req = urllib.request.Request(url)
    with urllib.request.urlopen(req) as r, open(dest, "wb") as f:
        while chunk := r.read(1 << 20):
            f.write(chunk)
    return dest.stat().st_size


def sha256_of(p: Path) -> str:
    h = hashlib.sha256()
    with open(p, "rb") as f:
        while chunk := f.read(1 << 20):
            h.update(chunk)
    return h.hexdigest()


def fetch_asset(version, name, tmp: Path) -> Path:
    url = DL.format(v=version, f=name)
    dest = tmp / name
    print(f"  ↓ {name}", flush=True)
    size = download(url, dest)
    print(f"    {size} bytes  sha256={sha256_of(dest)[:16]}…")
    return dest


def put_file(repo, path, text, token, msg, dry):
    """按 contents API 写文本文件（已存在则带 sha 更新）"""
    if dry:
        print(f"  [dry] {repo}/{path} ← {len(text)} bytes")
        return
    b64 = base64.b64encode(text.encode()).decode()
    payload = {"message": msg, "content": b64, "branch": "main"}
    try:
        payload["sha"] = gh("GET", f"/repos/{repo}/contents/{path}", token)["sha"]
    except RuntimeError:
        pass
    gh("PUT", f"/repos/{repo}/contents/{path}", token, payload)
    print(f"  ✓ {repo}/{path}")


# ── Homebrew ──────────────────────────────────────────────────────────────
CASK_TPL = '''cask "bit" do
  arch arm: "aarch64", intel: "x64"

  version "{version}"
  sha256 arm:   "{arm}",
         intel: "{intel}"

  url "https://github.com/yxpil/bit/releases/download/v#{{version}}/BIT_#{{version}}_#{{arch}}.dmg"
  name "BIT"
  desc "Local-first AI agent hub with MCP, tool registry and skills"
  homepage "https://github.com/yxpil/bit"

  livecheck do
    url :homepage
    strategy :github_latest
  end

  app "BIT.app"
end
'''


def sync_brew(version, token, tmp, dry):
    print(f"== brew {TAP} ==")
    v = version.lstrip("v")
    dmg = [fetch_asset(version, a.format(v=v), tmp) for a in BREW_ASSETS]
    arm = sha256_of(dmg[0])
    intel = sha256_of(dmg[1])
    put_file(TAP, "Casks/bit.rb", CASK_TPL.format(version=v, arm=arm, intel=intel),
             token, f"bit {v}", dry)


# ── Scoop ─────────────────────────────────────────────────────────────────
SCOOP_TPL = '''{{
    "version": "{version}",
    "description": "BIT - 本地优先的 AI Agent 工具集（MCP / 工具注册 / 技能）",
    "homepage": "https://github.com/yxpil/bit",
    "license": "Apache-2.0",
    "architecture": {{
        "64bit": {{
            "url": "https://github.com/yxpil/bit/releases/download/v{version}/BIT_{version}_x64-portable.zip",
            "hash": "{hash}"
        }}
    }},
    "bin": "BIT.exe",
    "shortcuts": [
        [
            "BIT.exe",
            "BIT"
        ]
    ],
    "checkver": "github",
    "autoupdate": {{
        "architecture": {{
            "64bit": {{
                "url": "https://github.com/yxpil/bit/releases/download/v$version/BIT_$version_x64-portable.zip"
            }}
        }}
    }}
}}
'''


def sync_scoop(version, token, tmp, dry):
    print(f"== scoop {SCOOP} ==")
    v = version.lstrip("v")
    z = fetch_asset(version, SCOOP_ASSETS[0].format(v=v), tmp)
    put_file(SCOOP, "bucket/bit.json", SCOOP_TPL.format(version=v, hash=sha256_of(z)),
             token, f"bit {v}", dry)


# ── APT ───────────────────────────────────────────────────────────────────
def apt_stanza(deb: Path) -> dict:
    """dpkg-deb 提取 control 字段，补上仓库路径与校验和"""
    fields = subprocess.run(
        ["dpkg-deb", "-f", str(deb),
         "Package", "Version", "Architecture", "Maintainer", "Installed-Size",
         "Depends", "Priority", "Description"],
        capture_output=True, text=True, check=True).stdout
    s = {}
    key = None
    for line in fields.splitlines():
        if line.startswith((" ", "\t")) and key:  # Description 续行
            s[key] += "\n" + line
        elif ":" in line:
            key, _, val = line.partition(":")
            s[key.strip()] = val.strip()
    name = deb.name
    s["Filename"] = f"pool/main/b/bit/{name}"
    s["Size"] = str(deb.stat().st_size)
    h = hashlib.md5(deb.read_bytes()).hexdigest()
    s["MD5sum"] = h
    s["SHA256"] = sha256_of(deb)
    return s


def render_packages(stanzas):
    out = []
    for s in stanzas:
        for k, v in s.items():
            out.append(f"{k}: {v}")
        out.append("")
    return "\n".join(out)


def sync_apt(version, token, tmp, dry):
    print(f"== apt {APT} ==")
    v = version.lstrip("v")
    stanzas_by_arch = {}
    debs = []
    for arch in APT_ARCHES:
        name = APT_NAMES[arch].format(v=v)
        try:
            d = fetch_asset(version, name, tmp)
        except Exception as e:
            print(f"  ! 跳过 {arch}: {e}")
            continue
        debs.append((arch, d))
        stanzas_by_arch[arch] = apt_stanza(d)
    if not debs:
        raise RuntimeError("apt: 没有任何架构的 deb 可用")
    if dry:
        print(f"  [dry] 上传 {len(debs)} 个 deb 到 pool + 重新生成 dists 元数据")
        return
    # 上传 deb（二进制走 contents API）
    for arch, d in debs:
        b64 = base64.b64encode(d.read_bytes()).decode()
        payload = {"message": f"bit {v} {arch}", "content": b64, "branch": "main"}
        try:
            payload["sha"] = gh("GET", f"/repos/{APT}/contents/pool/main/b/bit/{d.name}", token)["sha"]
        except RuntimeError:
            pass
        gh("PUT", f"/repos/{APT}/contents/pool/main/b/bit/{d.name}", token, payload)
        print(f"  ✓ pool/main/b/bit/{d.name}")
    # 重新生成 Packages / Packages.gz
    checksums = {"MD5Sum": [], "SHA256": []}
    for arch in stanzas_by_arch:
        text = render_packages([stanzas_by_arch[arch]])
        gz = gzip.compress(text.encode())
        for rel, content in [(f"dists/stable/main/binary-{arch}/Packages", text.encode()),
                             (f"dists/stable/main/binary-{arch}/Packages.gz", gz)]:
            b64 = base64.b64encode(content).decode()
            payload = {"message": f"apt metadata {v} {arch}", "content": b64, "branch": "main"}
            try:
                payload["sha"] = gh("GET", f"/repos/{APT}/contents/{rel}", token)["sha"]
            except RuntimeError:
                pass
            gh("PUT", f"/repos/{APT}/contents/{rel}", token, payload)
            print(f"  ✓ {rel}")
            checksums["MD5Sum"].append((rel, hashlib.md5(content).hexdigest(), str(len(content))))
            checksums["SHA256"].append((rel, hashlib.sha256(content).hexdigest(), str(len(content))))
    # Release 文件：沿用现有头部字段，刷新 Date 与校验和
    rel_path = "dists/stable/Release"
    old = gh("GET", f"/repos/{APT}/contents/{rel_path}", token)
    header = {}
    for line in base64.b64decode(old["content"]).decode().splitlines():
        if not line:
            break
        if line.startswith((" ", "\t")):
            continue
        k, _, val = line.partition(":")
        if k in ("Date", "MD5Sum", "SHA1", "SHA256", "SHA512"):
            continue
        header[k] = val.strip()
    lines = [f"{k}: {v2}" for k, v2 in header.items()]
    lines.append(f"Date: {datetime.now(timezone.utc).strftime('%a, %d %b %Y %H:%M:%S UTC')}")
    lines.append("")
    for sec in ("MD5Sum", "SHA256"):
        lines.append(sec + ":")
        for rel, digest, size in checksums[sec]:
            lines.append(f" {digest} {size:>8} {rel}")
        lines.append("")
    put_file(APT, rel_path, "\n".join(lines), token, f"apt Release {v}", dry)


# ── pacman / dnf 仓库（复用 gen_repos 生成器） ────────────────────────────
def blob_sha(data: bytes) -> str:
    return hashlib.sha1(b"blob %d\x00" % len(data) + data).hexdigest()


def put_binary(repo, path, data: bytes, token, msg, dry, retries=4):
    """contents API 写二进制文件：blob-sha 未变则跳过，401 抖动退避重试"""
    if dry:
        print(f"  [dry] {repo}/{path} ← {len(data)} bytes")
        return
    payload = {"message": msg, "content": base64.b64encode(data).decode(), "branch": "main"}
    try:
        payload["sha"] = gh("GET", f"/repos/{repo}/contents/{path}", token)["sha"]
        if payload["sha"] == blob_sha(data):
            print(f"  · {repo}/{path} 未变跳过")
            return
    except RuntimeError:
        pass
    for attempt in range(retries):
        try:
            gh("PUT", f"/repos/{repo}/contents/{path}", token, payload)
            print(f"  ✓ {repo}/{path} ({len(data)}B)")
            return
        except RuntimeError as e:
            if "401" in str(e) and attempt < retries - 1:
                time.sleep(5 * (attempt + 1))  # 认证抖动，退避重试
                continue
            raise
    raise RuntimeError(f"{repo}/{path}: 重试耗尽")


def sync_pacman(version, token, tmp, dry):
    print(f"== pacman {PACMAN} ==")
    v = version.lstrip("v")
    debs = []
    for arch in APT_ARCHES:
        try:
            d = fetch_asset(version, APT_NAMES[arch].format(v=v), tmp)
        except Exception as e:
            print(f"  ! 跳过 {arch}: {e}")
            continue
        debs.append((d, arch, gen_repos.deb_control(d)))
    if not debs:
        raise RuntimeError("pacman: 没有任何 deb 可用")
    out = tmp / "pacman-out"
    gen_repos.gen_pacman(debs, out)
    if dry:
        print("  [dry] 上传各架构 pkg.tar.gz + bit.db.tar.gz")
        return
    for f in sorted(out.rglob("*")):
        if f.is_file():
            # gen_pacman 在 out 下多写了一层 pacman/，仓库结构需要去掉这层
            put_binary(PACMAN, f.relative_to(out / "pacman").as_posix(), f.read_bytes(),
                       token, f"pacman {v}", dry)


def sync_dnf(version, token, tmp, dry):
    print(f"== dnf {DNF} ==")
    v = version.lstrip("v")
    rpms = []
    for arch, name_tpl in RPM_NAMES.items():
        try:
            r = fetch_asset(version, name_tpl.format(v=v), tmp)
        except Exception as e:
            print(f"  ! 跳过 {arch}: {e}")
            continue
        rpms.append((r, arch, {}))
    if not rpms:
        raise RuntimeError("dnf: 没有任何 rpm 可用")
    out = tmp / "dnf-out"
    gen_repos.gen_dnf(rpms, out)
    if dry:
        print("  [dry] 上传 packages/*.rpm + repodata/*")
        return
    for f in sorted(out.rglob("*")):
        if f.is_file():
            put_binary(DNF, f.relative_to(out).as_posix(), f.read_bytes(),
                       token, f"dnf {v}", dry)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("version", help="如 v0.5.9")
    ap.add_argument("--token", default=os.environ.get("GITHUB_TOKEN") or
                    subprocess.run(["gh", "auth", "token"], capture_output=True, text=True).stdout.strip())
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument("--only", help="只跑指定渠道（brew/scoop/apt/pacman/dnf），逗号分隔")
    a = ap.parse_args()
    if not a.token:
        sys.exit("需要 --token 或 GITHUB_TOKEN / gh auth token")
    # 校验 release 存在且非预发布
    rel = gh("GET", f"/repos/yxpil/bit/releases/tags/{a.version}", a.token)
    if rel.get("prerelease"):
        sys.exit(f"{a.version} 是预发布版本，跳过渠道同步")
    tmp = Path(tempfile.mkdtemp(prefix="bit-channels-"))
    results = {}
    for name, fn in [("brew", sync_brew), ("scoop", sync_scoop), ("apt", sync_apt),
                     ("pacman", sync_pacman), ("dnf", sync_dnf)]:
        if a.only and name not in [x.strip() for x in a.only.split(",")]:
            continue
        try:
            fn(a.version, a.token, tmp, a.dry_run)
            results[name] = "ok"
        except Exception as e:
            print(f"!! {name} 失败: {e}", file=sys.stderr)
            results[name] = f"FAIL: {e}"
    print("\n结果:", json.dumps(results, ensure_ascii=False))
    if any(v.startswith("FAIL") for v in results.values()):
        sys.exit(1)


if __name__ == "__main__":
    main()
