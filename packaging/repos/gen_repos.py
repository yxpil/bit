#!/usr/bin/env python3
"""从 .deb 生成自托管 APT 仓库（GitHub Pages）+ pacman 仓库。

用法: python3 gen_repos.py <deb目录> <输出目录>
deb 目录下应有 BIT_<version>_<arch>.deb（arch ∈ amd64/arm64/riscv64/loongarch64）
输出: <out>/apt  (dists + pool 结构)   <out>/pacman  (按架构的 [bit] 仓库)

apt 使用: deb [trusted=yes] https://yxpil.github.io/apt-repo stable main
pacman 使用: [bit] Server = https://yxpil.github.io/pacman-repo/$arch ; SigLevel = Never
"""
import gzip
import hashlib
import io
import json
import sys
import tarfile
import time
from pathlib import Path

VERSION = "0.4.5"
DEB_TO_DEB_ARCH = {"amd64": "amd64", "arm64": "arm64", "riscv64": "riscv64", "loongarch64": "loongarch64"}
DEB_TO_PAC_ARCH = {"amd64": "x86_64", "arm64": "aarch64", "riscv64": "riscv64", "loongarch64": "loongarch64"}
PKGDESC = "BIT - 本地优先的 AI Agent 工具集（MCP / 工具注册 / 技能）"
PKGURL = "https://github.com/yxpil/bit"


def read_ar(data: bytes) -> dict:
    """解析 ar 归档 → {成员名: 内容}"""
    assert data[:8] == b"!<arch>\n", "not an ar archive"
    out, pos = {}, 8
    while pos + 60 <= len(data):
        hdr = data[pos : pos + 60]
        name = hdr[0:16].decode().strip()
        size = int(hdr[48:58].decode().strip())
        out[name.rstrip("/")] = data[pos + 60 : pos + 60 + size]
        pos += 60 + size + (size % 2)
    return out


def open_tar(data: bytes):
    """按成员名探测压缩格式并解出 tar"""
    if data[:2] == b"x\x01" or data[:2] == b"\xfd7":  # xz magic \xfd377a585a00
        import lzma
        return tarfile.open(fileobj=io.BytesIO(lzma.decompress(data)))
    return tarfile.open(fileobj=io.BytesIO(gzip.decompress(data)))


def deb_control(path: Path) -> dict:
    """读取 deb 的 control 字段 → dict"""
    members = read_ar(path.read_bytes())
    for name in ("control.tar.xz", "control.tar.gz", "control.tar"):
        if name in members:
            tf = open_tar(members[name])
            names = tf.getnames()
            member = next(n for n in names if n.lstrip("./") == "control")
            fields = {}
            for line in tf.extractfile(member).read().decode().splitlines():
                if ": " in line:
                    k, v = line.split(": ", 1)
                    fields[k.strip()] = v.strip()
            return fields
    raise RuntimeError(f"control.tar not found in {path}")


def deb_data_files(path: Path):
    """列出 deb 的数据文件 → [(路径, 大小, md5, sha256, 数据字节或None)]"""
    members = read_ar(path.read_bytes())
    for name in ("data.tar.xz", "data.tar.gz", "data.tar"):
        if name in members:
            files = []
            tf = open_tar(members[name])
            for m in tf.getmembers():
                if not m.isfile():
                    continue
                data = tf.extractfile(m).read()
                files.append((m.name.lstrip("./"), len(data), hashlib.md5(data).hexdigest(), hashlib.sha256(data).hexdigest(), data))
            return files
    raise RuntimeError(f"data.tar not found in {path}")


def gen_apt(debs, out: Path):
    """APT 仓库: dists/stable/main/binary-<arch>/Packages.gz + pool"""
    comp, by_arch = "main", {}
    for deb, arch, fields in debs:
        by_arch.setdefault(arch, []).append((deb, fields))

    dists = out / "apt" / "dists" / "stable"
    for arch, items in by_arch.items():
        pkg_dir = dists / comp / f"binary-{arch}"
        pkg_dir.mkdir(parents=True, exist_ok=True)
        lines = []
        for deb, fields in items:
            fname = deb.name
            pool_file = f"pool/main/b/bit/{fname}"
            dest = out / "apt" / pool_file
            dest.parent.mkdir(parents=True, exist_ok=True)
            dest.write_bytes(deb.read_bytes())
            data = deb.read_bytes()
            size, md5, sha = len(data), hashlib.md5(data).hexdigest(), hashlib.sha256(data).hexdigest()
            lines.append(
                f"Package: {fields.get('Package', 'bit')}\n"
                f"Version: {fields.get('Version', VERSION)}\n"
                f"Architecture: {arch}\n"
                f"Maintainer: {fields.get('Maintainer', '')}\n"
                f"Installed-Size: {fields.get('Installed-Size', '')}\n"
                f"Depends: {fields.get('Depends', '')}\n"
                f"Section: {fields.get('Section', 'devel')}\n"
                f"Priority: optional\n"
                f"Filename: {pool_file}\n"
                f"Size: {size}\n"
                f"MD5sum: {md5}\n"
                f"SHA256: {sha}\n"
                f"Description: {fields.get('Description', PKGDESC)}\n"
            )
        text = "\n".join(lines).encode()
        (pkg_dir / "Packages").write_bytes(text)
        with gzip.open(pkg_dir / "Packages.gz", "wb") as f:
            f.write(text)

    # Release 文件（未签名；客户端用 [trusted=yes]）
    now = time.strftime("%a, %d %b %Y %H:%M:%S UTC", time.gmtime())
    entries = []
    for p in sorted(dists.rglob("*")):
        if p.is_file() and "binary-" in str(p):
            data = p.read_bytes()
            entries.append((p.relative_to(dists), len(data), hashlib.md5(data).hexdigest(), hashlib.sha256(data).hexdigest()))
    ck = "".join(
        f" {md5} {size:>8} {rel}\n {sha} {size:>8} {rel}\n"
        for rel, size, md5, sha in entries
    )
    (dists / "Release").write_text(
        f"Origin: bit\nLabel: bit\nSuite: stable\nCodename: stable\n"
        f"Architectures: {' '.join(by_arch.keys())}\nComponents: {comp}\nDate: {now}\n"
        f"MD5Sum:\n{ck}SHA256:\n{ck.rstrip()}\n"
    )


def gen_pacman(debs, out: Path):
    """pacman 仓库: 各架构 <pkg>.pkg.tar.gz + bit.db.tar.gz（由 deb 转换）"""
    for deb, arch, fields in debs:
        parch = DEB_TO_PAC_ARCH[arch]
        pdir = out / "pacman" / parch
        pdir.mkdir(parents=True, exist_ok=True)
        builddate = int(time.time())
        data_files = deb_data_files(deb)
        total_size = sum(s for _, s, *_ in data_files)

        # .PKGINFO
        depends = {"libwebkit2gtk-4.1-0": "webkit2gtk-4.1", "libgtk-3-0": "gtk3",
                   "libayatana-appindicator3-1": "libayatana-appindicator3", "librsvg2-2": "librsvg"}
        deps = [depends.get(d.strip().split()[0], d) for d in fields.get("Depends", "").split(",") if d.strip()]
        pkginfo = "\n".join(
            [f"pkgname = bit", f"pkgver = {VERSION}-1", f"pkgdesc = {PKGDESC}", f"url = {PKGURL}",
             f"builddate = {builddate}", f"packager = BIT Release <yxpil@users.noreply.github.com>",
             f"size = {total_size}", f"arch = {parch}", "license = Apache-2.0", "replaces = bit-git"]
            + [f"depend = {d}" for d in deps]
        ) + "\n"

        # .MTREE（pacman 4.2+ 校验所需）
        mtree = ["#mtree", "/set type=file uid=0 gid=0 mode=644"]
        for rel, size, md5, sha, _ in data_files:
            mode = "755" if rel.startswith("usr/bin/") else "644"
            mtree.append(f"/{rel} time={builddate}.0 size={size} md5digest={md5} sha256digest={sha} mode={mode}")
        mtree.append("")

        # 重打包: .PKGINFO + .MTREE + 数据文件 → <name>.pkg.tar.gz
        pkg_name = f"bit-{VERSION}-1-{parch}.pkg.tar.gz"
        with tarfile.open(pdir / pkg_name, "w:gz") as tf:
            def add(name, data):
                info = tarfile.TarInfo(name)
                info.size = len(data)
                info.mtime = builddate
                tf.addfile(info, io.BytesIO(data))
            add(".PKGINFO", pkginfo.encode())
            add(".MTREE", "\n".join(mtree).encode())
            for rel, size, md5, sha, blob in data_files:
                info = tarfile.TarInfo(rel)
                info.size = size
                info.mtime = builddate
                info.mode = 0o755 if rel.startswith("usr/bin/") else 0o644
                tf.addfile(info, io.BytesIO(blob))

        # 仓库数据库 desc 条目
        pkg_data = (pdir / pkg_name).read_bytes()
        desc = (
            f"%FILENAME%\n{pkg_name}\n\n%NAME%\nbit\n\n%BASE%\nbit\n\n%VERSION%\n{VERSION}-1\n\n"
            f"%DESC%\n{PKGDESC}\n\n%URL%\n{PKGURL}\n\n%ARCH%\n{parch}\n\n%BUILDDATE%\n{builddate}\n\n"
            f"%PACKAGER%\nBIT Release <yxpil@users.noreply.github.com>\n\n%SIZE%\n{len(pkg_data)}\n\n"
            f"%MD5SUM%\n{hashlib.md5(pkg_data).hexdigest()}\n\n%SHA256SUM%\n{hashlib.sha256(pkg_data).hexdigest()}\n\n"
            + "".join(f"%DEPENDS%\n{d}\n\n" for d in deps)
        )
        with tarfile.open(pdir / "bit.db.tar.gz", "w:gz") as tf:
            info = tarfile.TarInfo(f"bit-{VERSION}-1/desc")
            info.size = len(desc.encode())
            info.mtime = builddate
            tf.addfile(info, io.BytesIO(desc.encode()))
        print(f"pacman: {pkg_name} ({len(pkg_data)//1024} KB)")


def main():
    deb_dir, out = Path(sys.argv[1]), Path(sys.argv[2])
    debs = []
    for deb in sorted(deb_dir.glob("*.deb")):
        parts = deb.stem.rsplit("_", 1)
        arch = parts[1]
        if arch not in DEB_TO_DEB_ARCH:
            continue
        debs.append((deb, arch, deb_control(deb)))
        print(f"deb: {deb.name}  version={debs[-1][2].get('Version')}  arch={arch}")
    if not debs:
        sys.exit("no deb found")
    gen_apt(debs, out)
    gen_pacman(debs, out)
    print("done")


if __name__ == "__main__":
    main()
