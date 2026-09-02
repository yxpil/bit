#!/usr/bin/env bash
# 国产/新兴架构（riscv64 / loongarch64）构建脚本——在对应架构的 Debian 容器内运行。
# 宿主机（CI x64）已通过 QEMU binfmt 运行容器，前端 dist 已由宿主机构建好；
# 本脚本在容器内装工具链 → 原生编译 BIT → 手工打 deb 包 + 裸二进制 tar.gz。
# 用法: build.sh <deb-arch> <version>   例如: build.sh riscv64 0.4.5
set -euo pipefail
DEB_ARCH="$1"
VERSION="$2"

export DEBIAN_FRONTEND=noninteractive
apt-get update
# webkit2gtk/gtk 为 Tauri 必需；ayatana 为托盘图标（tray-icon feature）必需
apt-get install -y --no-install-recommends \
  curl ca-certificates build-essential pkg-config libssl-dev file \
  libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev

# rustup 官方分发 riscv64gc / loongarch64 的 rustup-init（tier2 目标）
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
source "$HOME/.cargo/env"
rustc --version

# 编译（前端资源经 tauri-build 的 generate_context! 在编译期嵌入二进制）
cargo build --release --manifest-path src-tauri/Cargo.toml
BIN=src-tauri/target/release/bit
ls -lh "$BIN"

OUT=packaging/exotic/out
rm -rf "$OUT"
mkdir -p "$OUT"

# ---- deb 包（Debian/Ubuntu/Loongnix/深 speed 等直接安装）----
PKG="$OUT/pkg"
mkdir -p "$PKG/DEBIAN" "$PKG/usr/bin" \
  "$PKG/usr/share/applications" \
  "$PKG/usr/share/icons/hicolor/256x256/apps"
install -m 755 "$BIN" "$PKG/usr/bin/bit"
install -m 644 src-tauri/icons/256x256.png "$PKG/usr/share/icons/hicolor/256x256/apps/bit.png"
cat > "$PKG/usr/share/applications/bit.desktop" <<'DESK'
[Desktop Entry]
Type=Application
Name=BIT
Comment=BIT AI 工具集
Exec=/usr/bin/bit
Icon=bit
Categories=Development;Utility;
DESK
# libssl 由 libwebkit2gtk 传递依赖（trixie 起包名 libssl3→libssl3t64，勿显式声明）
cat > "$PKG/DEBIAN/control" <<CTL
Package: bit
Version: $VERSION
Section: devel
Priority: optional
Architecture: $DEB_ARCH
Maintainer: yxpil <yxpil@users.noreply.github.com>
Depends: libwebkit2gtk-4.1-0, libgtk-3-0, libayatana-appindicator3-1, librsvg2-2
Description: BIT - AI 工具集（本地优先，多端适配）
CTL
dpkg-deb --build --root-owner-group "$PKG" "$OUT/bit_${VERSION}_${DEB_ARCH}.deb"

# ---- 裸二进制 tar.gz（Arch/其他发行版手动安装；自带 PKGBUILD 也可复用）----
tar -czf "$OUT/bit_${VERSION}_${DEB_ARCH}.tar.gz" -C src-tauri/target/release bit

ls -lh "$OUT"
