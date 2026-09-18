// Generates icons/icon{16,48,128}.png with no dependencies (Node's zlib only).
// A rounded indigo square with a white "c" ring. Run: node tools/make-icons.js

const fs = require("fs");
const path = require("path");
const zlib = require("zlib");

const CRC_TABLE = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();

function crc32(buf) {
  let c = 0xffffffff;
  for (const b of buf) c = CRC_TABLE[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const td = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(td));
  return Buffer.concat([len, td, crc]);
}

function png(size, pixel) {
  const raw = Buffer.alloc(size * (size * 4 + 1));
  for (let y = 0; y < size; y++) {
    raw[y * (size * 4 + 1)] = 0; // filter: none
    for (let x = 0; x < size; x++) {
      const [r, g, b, a] = pixel(x + 0.5, y + 0.5, size);
      const o = y * (size * 4 + 1) + 1 + x * 4;
      raw[o] = r;
      raw[o + 1] = g;
      raw[o + 2] = b;
      raw[o + 3] = a;
    }
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // RGBA
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", zlib.deflateSync(raw)),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

function icon(x, y, s) {
  const r = s * 0.2; // corner radius
  const dx = Math.max(r - x, 0, x - (s - r));
  const dy = Math.max(r - y, 0, y - (s - r));
  if (dx * dx + dy * dy > r * r) return [0, 0, 0, 0];
  const cx = s / 2, cy = s / 2;
  const d = Math.hypot(x - cx, y - cy);
  const angle = Math.atan2(y - cy, x - cx);
  const inRing = d > s * 0.2 && d < s * 0.33;
  const inGap = Math.abs(angle) < 0.6; // opening on the right: a "c"
  if (inRing && !inGap) return [255, 255, 255, 255];
  return [79, 70, 229, 255];
}

const out = path.join(__dirname, "..", "icons");
fs.mkdirSync(out, { recursive: true });
for (const s of [16, 48, 128]) {
  fs.writeFileSync(path.join(out, `icon${s}.png`), png(s, icon));
}
console.log("wrote icons to", out);
