#!/usr/bin/env bash
# Alpine (musl) 构建脚本——在 alpine:edge 容器内运行。
# 产物：裸二进制 tar.gz。webkit2gtk 等运行时依赖由用户侧 apk 提供：
#   apk add webkit2gtk-4.1 gtk+3.0 libayatana-appindicator librsvg
# musl 生态对 Tauri 无官方支持，构建失败属预期内（CI 允许失败）。
# 用法: build.sh <apk-arch> <version>   例如: build.sh x86_64 0.5.10
set -euo pipefail
ARCH="$1"
VERSION="$2"

apk add --no-cache \
  build-base pkgconf openssl-dev file \
  webkit2gtk-4.1-dev gtk+3.0-dev libayatana-appindicator-dev librsvg-dev \
  rust cargo

rustc --version
# 编译（前端资源经 tauri-build 的 generate_context! 在编译期嵌入二进制）
# 注意：不能用 cargo build 原样启用 Cargo.toml 的全部 features，
#       因为 tauri v2 build.rs 会校验 features 与 tauri.conf.json allowlist 匹配。
#       Cargo.toml 包含 macos-private-api（macOS 专用），Linux 平台不识别会直接 exit 1。
#       只启用 Linux 必需的 tray-icon，跳过 macos 专属 features。
cargo build --release --manifest-path src-tauri/Cargo.toml \
  --no-default-features --features tray-icon
BIN=src-tauri/target/release/bit
ls -lh "$BIN"

OUT=packaging/musl/out
rm -rf "$OUT"
mkdir -p "$OUT"
tar -czf "$OUT/bit_${VERSION}_${ARCH}-musl.tar.gz" -C src-tauri/target/release bit

ls -lh "$OUT"
