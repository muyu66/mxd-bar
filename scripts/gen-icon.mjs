// 生成应用图标源图(1024×1024 PNG):冒险岛橙圆角底 + 白色药水瓶,随后用 `npx tauri icon` 出全套尺寸
import { deflateSync } from 'node:zlib';
import { writeFileSync } from 'node:fs';

const W = 1024, H = 1024;
const px = Buffer.alloc(W * H * 4); // RGBA

const inRR = (x, y, x0, y0, x1, y1, r) => {
  if (x < x0 || x > x1 || y < y0 || y > y1) return false;
  const cx = Math.min(Math.max(x, x0 + r), x1 - r);
  const cy = Math.min(Math.max(y, y0 + r), y1 - r);
  const dx = x - cx, dy = y - cy;
  return dx * dx + dy * dy <= r * r;
};
const inEllipse = (x, y, cx, cy, rx, ry) => {
  const dx = (x - cx) / rx, dy = (y - cy) / ry;
  return dx * dx + dy * dy <= 1;
};
const lerp = (a, b, t) => a + (b - a) * t;
const set = (i, r, g, b, a = 255) => { px[i] = r; px[i + 1] = g; px[i + 2] = b; px[i + 3] = a; };

for (let y = 0; y < H; y++) {
  for (let x = 0; x < W; x++) {
    const i = (y * W + x) * 4;
    // 底:橙色圆角(上下渐变),圆角外透明
    const t = y / H;
    set(i, lerp(255, 245, t), lerp(184, 166, t), lerp(61, 35, t));
    if (!inRR(x, y, 0, 0, W - 1, H - 1, 200)) set(i, 0, 0, 0, 0);
    // 药水瓶:瓶身 + 瓶颈
    if (inRR(x, y, 352, 430, 672, 790, 96) || (x >= 462 && x <= 562 && y >= 310 && y <= 470)) {
      set(i, 255, 255, 255, 255);
    }
    // 瓶身高光
    if (inEllipse(x, y, 430, 560, 40, 90)) set(i, 245, 250, 255, 255);
    // 瓶塞(棕色)
    if (inRR(x, y, 442, 216, 582, 318, 34)) set(i, 185, 138, 86, 255);
  }
}

// ---------- PNG 编码 ----------
const crcTable = (() => {
  const t = new Int32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c;
  }
  return t;
})();
const crc32 = (buf) => {
  let c = 0xffffffff;
  for (const b of buf) c = crcTable[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
};
const chunk = (type, data) => {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, 'ascii'), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
};

const raw = Buffer.alloc(H * (1 + W * 4)); // 每行前置 filter 0
for (let y = 0; y < H; y++) {
  raw[y * (1 + W * 4)] = 0;
  px.copy(raw, y * (1 + W * 4) + 1, y * W * 4, (y + 1) * W * 4);
}
const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(W, 0);
ihdr.writeUInt32BE(H, 4);
ihdr[8] = 8; // bit depth
ihdr[9] = 6; // RGBA
const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk('IHDR', ihdr),
  chunk('IDAT', deflateSync(raw, { level: 9 })),
  chunk('IEND', Buffer.alloc(0)),
]);
writeFileSync(new URL('../src-tauri/app-icon.png', import.meta.url), png);
console.log('icon source written:', png.length, 'bytes');
