// 生成 BIT 黑白小圆片图标（PNG 各尺寸 + ICO）
const fs = require("fs");
const path = require("path");
const zlib = require("zlib");

const dir = path.join(__dirname, "src-tauri", "icons");
fs.mkdirSync(dir, { recursive: true });

// 手写最小 PNG 编码器（RGBA → 无压缩 zlib stored 块）
function crc32(buf) {
  let c, crc = 0xffffffff;
  for (let n = 0; n < buf.length; n++) {
    c = (crc ^ buf[n]) & 0xff;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    crc = (crc >>> 8) ^ c;
  }
  return (crc ^ 0xffffffff) >>> 0;
}

function adler32(buf) {
  let a = 1, b = 0;
  for (let i = 0; i < buf.length; i++) {
    a = (a + buf[i]) % 65521;
    b = (b + a) % 65521;
  }
  return ((b << 16) | a) >>> 0;
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const td = Buffer.concat([Buffer.from(type), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(td));
  return Buffer.concat([len, td, crc]);
}

function makePng(size) {
  const rows = [];
  const c = size / 2; // 圆心
  const r = size * 0.42; // 圆半径
  const barW = size * 0.3, barH = size * 0.36; // 白色竖条（"I" 造型）
  for (let y = 0; y < size; y++) {
    const row = Buffer.alloc(1 + size * 4);
    row[0] = 0; // filter none
    for (let x = 0; x < size; x++) {
      const dx = x - c, dy = y - c;
      const inside = dx * dx + dy * dy <= r * r;
      const inBar =
        Math.abs(dx) <= barW / 2 && Math.abs(dy) <= barH / 2;
      const black = inside && !inBar;
      const alpha = inside ? 255 : 0;
      const o = 1 + x * 4;
      const v = black ? 15 : 255;
      row[o] = v; row[o + 1] = v; row[o + 2] = v; row[o + 3] = alpha;
    }
    rows.push(row);
  }
  const raw = Buffer.concat(rows);
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8; ihdr[9] = 6; // 8-bit RGBA
  const idat = zlib.deflateSync(raw, { level: 9 });
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", idat),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

for (const size of [32, 128, 256]) {
  fs.writeFileSync(path.join(dir, `${size}x${size}.png`), makePng(size));
}
fs.writeFileSync(path.join(dir, "128x128@2x.png"), makePng(256));
fs.writeFileSync(path.join(dir, "icon.png"), makePng(256));

// ICO：包含 32/128/256 PNG
const sizes = [32, 128, 256];
const pngs = sizes.map((s) => makePng(s));
const header = Buffer.alloc(6);
header.writeUInt16LE(0, 0);
header.writeUInt16LE(1, 2); // type: icon
header.writeUInt16LE(sizes.length, 4);
const entries = [];
let offset = 6 + sizes.length * 16;
const dataBufs = [];
sizes.forEach((s, i) => {
  const e = Buffer.alloc(16);
  e[0] = s >= 256 ? 0 : s;
  e[1] = s >= 256 ? 0 : s;
  e[2] = 0; e[3] = 0;
  e.writeUInt16LE(1, 4); // planes
  e.writeUInt16LE(32, 6); // bpp
  e.writeUInt32LE(pngs[i].length, 8);
  e.writeUInt32LE(offset, 12);
  offset += pngs[i].length;
  entries.push(e);
  dataBufs.push(pngs[i]);
});
fs.writeFileSync(path.join(dir, "icon.ico"), Buffer.concat([header, ...entries, ...dataBufs]));
console.log("icons generated at", dir);
