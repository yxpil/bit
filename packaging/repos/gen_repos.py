#!/usr/bin/env python3
"""从 .deb 生成自托管 APT 仓库（GitHub Pages）+ pacman 仓库。

用法: python3 gen_repos.py <deb目录> <输出目录>
deb 目录下应有 BIT_<version>_<arch>.deb（arch ∈ amd64/arm64/riscv64/loongarch64/ppc64le）
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

VERSION = "0.5.2"  # 兜底；实际版本从 deb control / rpm 头部动态读取
DEB_TO_DEB_ARCH = {"amd64": "amd64", "arm64": "arm64", "riscv64": "riscv64",
                   "loongarch64": "loongarch64", "ppc64le": "ppc64le"}
DEB_TO_PAC_ARCH = {"amd64": "x86_64", "arm64": "aarch64", "riscv64": "riscv64",
                   "loongarch64": "loongarch64", "ppc64le": "powerpc64le"}
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
        ver = fields.get("Version", VERSION)
        pkginfo = "\n".join(
            [f"pkgname = bit", f"pkgver = {ver}-1", f"pkgdesc = {PKGDESC}", f"url = {PKGURL}",
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
        pkg_name = f"bit-{ver}-1-{parch}.pkg.tar.gz"
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
            f"%FILENAME%\n{pkg_name}\n\n%NAME%\nbit\n\n%BASE%\nbit\n\n%VERSION%\n{ver}-1\n\n"
            f"%DESC%\n{PKGDESC}\n\n%URL%\n{PKGURL}\n\n%ARCH%\n{parch}\n\n%BUILDDATE%\n{builddate}\n\n"
            f"%PACKAGER%\nBIT Release <yxpil@users.noreply.github.com>\n\n%SIZE%\n{len(pkg_data)}\n\n"
            f"%MD5SUM%\n{hashlib.md5(pkg_data).hexdigest()}\n\n%SHA256SUM%\n{hashlib.sha256(pkg_data).hexdigest()}\n\n"
            + "".join(f"%DEPENDS%\n{d}\n\n" for d in deps)
        )
        with tarfile.open(pdir / "bit.db.tar.gz", "w:gz") as tf:
            info = tarfile.TarInfo(f"bit-{ver}-1/desc")
            info.size = len(desc.encode())
            info.mtime = builddate
            tf.addfile(info, io.BytesIO(desc.encode()))
        print(f"pacman: {pkg_name} ({len(pkg_data)//1024} KB)")


# ---------------- dnf / yum（RPM 仓库） ----------------

RPM_STR, RPM_I18N, RPM_BIN = 6, 8, 7
RPM_INT16, RPM_INT32, RPM_INT64 = 3, 4, 5
TAG_NAME, TAG_VERSION, TAG_RELEASE = 1000, 1001, 1002
TAG_SUMMARY, TAG_DESCRIPTION, TAG_BUILDTIME, TAG_SIZE = 1004, 1005, 1006, 1009
TAG_VENDOR, TAG_LICENSE, TAG_URL, TAG_ARCH = 1011, 1014, 1020, 1022
TAG_PROVIDENAME, TAG_REQUIRENAME = 1047, 1049


def _read_header(data: bytes, pos: int):
    nindex = int.from_bytes(data[pos + 8 : pos + 12], "big")
    hsize = int.from_bytes(data[pos + 12 : pos + 16], "big")
    idx = pos + 16
    entries = []
    for i in range(nindex):
        off = idx + i * 16
        entries.append(tuple(int.from_bytes(data[off + j * 4 : off + j * 4 + 4], "big") for j in range(4)))
    return entries, data[idx + nindex * 16 : idx + nindex * 16 + hsize], idx + nindex * 16 + hsize


def _header_entries(data: bytes, pos: int):
    """解析 main header → {tag: 值}"""
    region_entries, ddata, end = _read_header(data, pos)
    values = {}

    def val(tag):
        for t, typ, off, cnt in region_entries:
            if t == tag:
                if typ in (RPM_STR, RPM_I18N):
                    return ddata[off : ddata.index(b"\x00", off)].decode("utf-8", "replace")
                if typ == RPM_INT32:
                    return [int.from_bytes(ddata[off + i * 4 : off + i * 4 + 4], "big") for i in range(cnt)][0 if cnt == 1 else slice(None)]
                if typ == RPM_INT16:
                    return [int.from_bytes(ddata[off + i * 2 : off + i * 2 + 2], "big") for i in range(cnt)][0 if cnt == 1 else slice(None)]
                if typ == RPM_BIN:
                    return ddata[off : off + cnt]
        return None

    for t in (TAG_NAME, TAG_VERSION, TAG_RELEASE, TAG_SUMMARY, TAG_DESCRIPTION,
              TAG_BUILDTIME, TAG_SIZE, TAG_VENDOR, TAG_LICENSE, TAG_URL, TAG_ARCH,
              TAG_PROVIDENAME, TAG_REQUIRENAME):
        v = val(t)
        if v is not None:
            values[t] = v
    return values, end


def rpm_fields(path: Path) -> dict:
    """解析 rpm 头部关键字段"""
    data = path.read_bytes()
    pos = 96  # 跳过 lead
    _, _, end = _read_header(data, pos)          # signature header
    pad = (8 - ((end - 96) % 8)) % 8
    pos = end + pad
    # 对齐容错：若 magic 不符则逐 4 字节向后找
    while data[pos : pos + 3] != b"\x8e\xad\xe8":
        pos += 4
    fields, _ = _header_entries(data, pos)
    return fields


def gen_dnf(rpms, out: Path):
    """dnf 仓库: packages/*.rpm + repodata/repomd.xml + primary.xml.gz"""
    rdir = out / "dnf"
    (rdir / "packages").mkdir(parents=True, exist_ok=True)
    now = int(time.time())
    pkgs_xml = []

    for rpm, arch, _ in rpms:
        f = rpm_fields(rpm)
        name = f.get(TAG_NAME, "bit")
        ver, rel = f.get(TAG_VERSION, VERSION), f.get(TAG_RELEASE, "1")
        size = rpm.stat().st_size
        sha = hashlib.sha256(rpm.read_bytes()).hexdigest()
        dest = rdir / "packages" / rpm.name
        dest.write_bytes(rpm.read_bytes())
        requires = f.get(TAG_REQUIRENAME) or []
        if isinstance(requires, str):
            requires = [requires]
        provides = f.get(TAG_PROVIDENAME) or []
        if isinstance(provides, str):
            provides = [provides]
        req_xml = "".join(
            f'<rpm:entry name="{r}"/>' for r in requires
            if not r.startswith("rpmlib(") and not r.startswith("/")
        )
        prov_xml = "".join(f'<rpm:entry name="{p}"/>' for p in provides)
        pkgs_xml.append(
            f'<package type="rpm"><name>{name}</name><arch>{f.get(TAG_ARCH, arch)}</arch>'
            f'<version epoch="0" ver="{ver}" rel="{rel}"/>'
            f'<checksum type="sha256" pkgid="YES">{sha}</checksum>'
            f"<summary>{f.get(TAG_SUMMARY, PKGDESC)}</summary>"
            f"<description>{f.get(TAG_DESCRIPTION, PKGDESC)}</description>"
            f"<packager>BIT Release</packager><url>{f.get(TAG_URL, PKGURL)}</url>"
            f'<time file="{now}" build="{f.get(TAG_BUILDTIME, now)}"/>'
            f'<size package="{size}" installed="{f.get(TAG_SIZE, size)}" archive="{size}"/>'
            f'<location href="packages/{rpm.name}"/>'
            f"<format>"
            f"<rpm:license>{f.get(TAG_LICENSE, 'Apache-2.0')}</rpm:license>"
            f"<rpm:vendor>{f.get(TAG_VENDOR, 'BIT')}</rpm:vendor>"
            f"<rpm:group>Applications/Development</rpm:group>"
            f"<rpm:buildhost>bit-release</rpm:buildhost>"
            f"<rpm:sourcerpm>{name}-{ver}-{rel}.src.rpm</rpm:sourcerpm>"
            f'<rpm:header-range start="372" end="{size - 1}"/>'
            f"<rpm:provides>{prov_xml}</rpm:provides>"
            f"<rpm:requires>{req_xml}</rpm:requires>"
            f"</format></package>"
        )
        print(f"dnf: {rpm.name}  ({name}-{ver}-{rel} {f.get(TAG_ARCH, arch)})")

    body = "\n".join(pkgs_xml).encode()
    import gzip as _gz
    primary_gz = _gz.compress(body)
    psha = hashlib.sha256(primary_gz).hexdigest()
    ptime, psize = now, len(primary_gz)
    repomd = (
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<repomd xmlns="http://linux.duke.edu/metadata/repo" xmlns:rpm="http://linux.duke.edu/metadata/rpm">\n'
        f"<revision>{now}</revision>\n"
        f'<data type="primary">\n<checksum type="sha256">{psha}</checksum>\n'
        f"<open-checksum type=\"sha256\">{hashlib.sha256(body).hexdigest()}</open-checksum>\n"
        f"<location href=\"repodata/{psha}-primary.xml.gz\"/>\n<timestamp>{ptime}</timestamp>\n"
        f"<size>{psize}</size>\n<open-size>{len(body)}</open-size>\n</data>\n</repomd>\n"
    )
    (rdir / "repodata").mkdir(exist_ok=True)
    (rdir / "repodata" / "repomd.xml").write_text(repomd)
    (rdir / "repodata" / f"{psha}-primary.xml.gz").write_bytes(primary_gz)


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
    rpms = []
    for rpm in sorted(deb_dir.glob("*.rpm")):
        arch = "x86_64" if "x86_64" in rpm.name else ("aarch64" if "aarch64" in rpm.name else None)
        if arch:
            rpms.append((rpm, arch, {}))
    if rpms:
        gen_dnf(rpms, out)
    print("done")


if __name__ == "__main__":
    main()
