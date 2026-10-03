#!/usr/bin/env node
/**
 * 从一张像素画 PNG 生成「枫之助」全套图标：
 *   - src-tauri/icons/ 下的 32/128/256/icon.png、icon.ico（含多尺寸 PNG 条目）、icon.icns、
 *     Square*Logo.png、StoreLogo.png（exe / 窗口 / 托盘 / 打包用）
 *   - src/assets/mushroom.png（界面左上角 Logo）
 *   - public/favicon.svg（内嵌 base64 PNG）
 *
 * 源图是一张不带透明通道的像素画（棋盘格“假背景”直接画在像素里），
 * 所以第一步是把棋盘格背景抠成透明：从四边 flood fill 只清除两种格子色，
 * 再做一两轮紧贴透明区的“描边清理”，把抗锯齿残留的浅色毛边也去掉。
 *
 * 缩放一律用最近邻（像素画专用，不能用双线性，否则会糊）。
 *
 * 用法：node scripts/gen-icons.mjs <source.png>
 */
import fs from "node:fs";
import path from "node:path";
import zlib from "node:zlib";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

// ---------------------------------------------------------------------------
// PNG 解码（只支持 8-bit RGB / RGBA、非隔行 —— 够用了）
// ---------------------------------------------------------------------------

function decodePng(buf) {
  if (buf.slice(1, 4).toString("latin1") !== "PNG") throw new Error("不是 PNG 文件");
  const chunks = [];
  let pos = 8;
  while (pos < buf.length) {
    const len = buf.readUInt32BE(pos);
    const type = buf.slice(pos + 4, pos + 8).toString("latin1");
    chunks.push({ type, data: buf.slice(pos + 8, pos + 8 + len) });
    pos += 12 + len;
  }
  const ihdr = chunks.find((c) => c.type === "IHDR").data;
  const w = ihdr.readUInt32BE(0);
  const h = ihdr.readUInt32BE(4);
  const bitDepth = ihdr[8];
  const colorType = ihdr[9];
  const interlace = ihdr[12];
  if (bitDepth !== 8 || interlace !== 0 || (colorType !== 2 && colorType !== 6)) {
    throw new Error(`不支持的 PNG（bitDepth=${bitDepth} colorType=${colorType} interlace=${interlace}）`);
  }
  const bpp = colorType === 2 ? 3 : 4;
  const raw = zlib.inflateSync(Buffer.concat(chunks.filter((c) => c.type === "IDAT").map((c) => c.data)));
  const stride = w * bpp;
  const out = Buffer.alloc(h * stride);
  let rp = 0;
  const paeth = (a, b, c) => {
    const p = a + b - c;
    const pa = Math.abs(p - a);
    const pb = Math.abs(p - b);
    const pc = Math.abs(p - c);
    return pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
  };
  for (let y = 0; y < h; y++) {
    const filter = raw[rp++];
    const row = out.subarray(y * stride, (y + 1) * stride);
    const prev = y > 0 ? out.subarray((y - 1) * stride, y * stride) : null;
    for (let x = 0; x < stride; x++) {
      const rv = raw[rp + x];
      const a = x >= bpp ? row[x - bpp] : 0;
      const b = prev ? prev[x] : 0;
      const c = prev && x >= bpp ? prev[x - bpp] : 0;
      let v;
      switch (filter) {
        case 0: v = rv; break;
        case 1: v = rv + a; break;
        case 2: v = rv + b; break;
        case 3: v = rv + ((a + b) >> 1); break;
        case 4: v = rv + paeth(a, b, c); break;
        default: throw new Error(`未知的 PNG 行滤波 ${filter}`);
      }
      row[x] = v & 0xff;
    }
    rp += stride;
  }
  const data = Buffer.alloc(w * h * 4);
  for (let i = 0; i < w * h; i++) {
    data[i * 4] = out[i * bpp];
    data[i * 4 + 1] = out[i * bpp + 1];
    data[i * 4 + 2] = out[i * bpp + 2];
    data[i * 4 + 3] = bpp === 4 ? out[i * bpp + 3] : 255;
  }
  return { w, h, data };
}

// ---------------------------------------------------------------------------
// PNG 编码（RGBA、filter 0）
// ---------------------------------------------------------------------------

const CRC_TABLE = (() => {
  const t = new Int32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c;
  }
  return t;
})();

function crc32(buf) {
  let c = 0xffffffff;
  for (let i = 0; i < buf.length; i++) c = CRC_TABLE[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function pngChunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "latin1"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
}

function encodePng(img) {
  const { w, h, data } = img;
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0);
  ihdr.writeUInt32BE(h, 4);
  ihdr[8] = 8;  // bit depth
  ihdr[9] = 6;  // RGBA
  const stride = w * 4;
  const raw = Buffer.alloc(h * (stride + 1));
  for (let y = 0; y < h; y++) {
    raw[y * (stride + 1)] = 0; // filter: none
    data.copy(raw, y * (stride + 1) + 1, y * stride, (y + 1) * stride);
  }
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    pngChunk("IHDR", ihdr),
    pngChunk("IDAT", zlib.deflateSync(raw, { level: 9 })),
    pngChunk("IEND", Buffer.alloc(0)),
  ]);
}

// ---------------------------------------------------------------------------
// 背景清除：棋盘格假背景 → 透明
// ---------------------------------------------------------------------------

function removeCheckerBackground(img) {
  const { w, h, data } = img;
  const idx = (x, y) => (y * w + x) * 4;

  // 1. 统计全图颜色，出现最多的两种就是棋盘格的两种格子色
  //    （背景占了画面大半，蘑菇的任何颜色都盖不过它）。
  const hist = new Map();
  for (let i = 0; i < w * h; i++) {
    const key = (data[i * 4] << 16) | (data[i * 4 + 1] << 8) | data[i * 4 + 2];
    hist.set(key, (hist.get(key) ?? 0) + 1);
  }
  const top = [...hist.entries()].sort((a, b) => b[1] - a[1]).slice(0, 2);
  const checker = top.map(([key]) => [(key >> 16) & 0xff, (key >> 8) & 0xff, key & 0xff]);
  console.log(`棋盘格颜色：${checker.map((c) => `rgb(${c.join(",")})`).join(" / ")}`);

  const isChecker = (x, y, tol) => {
    const i = idx(x, y);
    const [r, g, b] = [data[i], data[i + 1], data[i + 2]];
    return checker.some(
      ([cr, cg, cb]) => Math.abs(r - cr) + Math.abs(g - cg) + Math.abs(b - cb) <= tol,
    );
  };
  const clear = (x, y) => { data[idx(x, y) + 3] = 0; };
  const isClear = (x, y) => data[idx(x, y) + 3] === 0;

  // 2. 最外 3px 是画布自带的细边框，无条件清掉
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      if (x < 3 || y < 3 || x >= w - 3 || y >= h - 3) clear(x, y);
    }
  }

  // 3. 从内圈边缘出发 flood fill，把连通的格子色背景清成透明。
  //    只走格子色 —— 蘑菇身上的浅米色是“内部”，不会被误伤。
  const stack = [];
  for (let x = 3; x < w - 3; x++) { stack.push([x, 3], [x, h - 4]); }
  for (let y = 3; y < h - 3; y++) { stack.push([3, y], [w - 4, y]); }
  while (stack.length) {
    const [x, y] = stack.pop();
    if (x < 3 || y < 3 || x >= w - 3 || y >= h - 3) continue;
    if (isClear(x, y) || !isChecker(x, y, 24)) continue;
    clear(x, y);
    stack.push([x + 1, y], [x - 1, y], [x, y + 1], [x, y - 1]);
  }

  // 4. 描边清理：紧贴透明区、又确实像格子色的残留毛边（缩放/抗锯齿产物）也清掉。
  //    额外要求“低饱和度”，保护蘑菇身体的米白色。
  for (let pass = 0; pass < 2; pass++) {
    const doomed = [];
    for (let y = 3; y < h - 3; y++) {
      for (let x = 3; x < w - 3; x++) {
        if (isClear(x, y)) continue;
        const touchesClear =
          isClear(x + 1, y) || isClear(x - 1, y) || isClear(x, y + 1) || isClear(x, y - 1);
        if (!touchesClear || !isChecker(x, y, 30)) continue;
        const i = idx(x, y);
        const spread = Math.max(data[i], data[i + 1], data[i + 2]) - Math.min(data[i], data[i + 1], data[i + 2]);
        if (spread <= 10) doomed.push([x, y]);
      }
    }
    for (const [x, y] of doomed) clear(x, y);
    console.log(`描边清理第 ${pass + 1} 轮：清除 ${doomed.length} 个毛边像素`);
  }

  // 5. 统计清除量 + 有效内容包围盒
  let cleared = 0;
  let minX = w, minY = h, maxX = -1, maxY = -1;
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      if (isClear(x, y)) cleared++;
      else {
        if (x < minX) minX = x;
        if (x > maxX) maxX = x;
        if (y < minY) minY = y;
        if (y > maxY) maxY = y;
      }
    }
  }
  console.log(`背景清除 ${cleared} / ${w * h} 像素；内容包围盒 ${maxX - minX + 1}x${maxY - minY + 1} @ (${minX},${minY})`);
  return { minX, minY, maxX, maxY };
}

// ---------------------------------------------------------------------------
// 方形图标渲染（最近邻，内容居中）
// ---------------------------------------------------------------------------

function crop(img, box) {
  const { minX, minY, maxX, maxY } = box;
  const cw = maxX - minX + 1;
  const ch = maxY - minY + 1;
  const data = Buffer.alloc(cw * ch * 4);
  for (let y = 0; y < ch; y++) {
    img.data.copy(data, y * cw * 4, ((minY + y) * img.w + minX) * 4, ((minY + y) * img.w + minX + cw) * 4);
  }
  return { w: cw, h: ch, data };
}

function renderSquare(src, size) {
  // 大尺寸用整数倍缩放（像素块均匀），小尺寸放开用分数倍（否则缩不出细节）
  let scale;
  const intScale = Math.floor((size * 0.92) / Math.max(src.w, src.h));
  if (size >= 48 && intScale >= 1) {
    scale = intScale;
  } else {
    scale = (size * 0.92) / Math.max(src.w, src.h);
  }
  const drawW = Math.max(1, Math.round(src.w * scale));
  const drawH = Math.max(1, Math.round(src.h * scale));
  const offX = Math.floor((size - drawW) / 2);
  const offY = Math.floor((size - drawH) / 2);
  const data = Buffer.alloc(size * size * 4); // 全透明
  for (let dy = 0; dy < drawH; dy++) {
    for (let dx = 0; dx < drawW; dx++) {
      const sx = Math.min(src.w - 1, Math.floor(dx / scale));
      const sy = Math.min(src.h - 1, Math.floor(dy / scale));
      const px = dx + offX;
      const py = dy + offY;
      if (px < 0 || py < 0 || px >= size || py >= size) continue;
      const si = (sy * src.w + sx) * 4;
      const di = (py * size + px) * 4;
      data[di] = src.data[si];
      data[di + 1] = src.data[si + 1];
      data[di + 2] = src.data[si + 2];
      data[di + 3] = src.data[si + 3];
    }
  }
  return { w: size, h: size, data };
}

// ---------------------------------------------------------------------------
// .ico / .icns 封装
// ---------------------------------------------------------------------------

function buildIco(entries /* [{size, png}] */) {
  const header = Buffer.alloc(6);
  header.writeUInt16LE(0, 0); // reserved
  header.writeUInt16LE(1, 2); // type: icon
  header.writeUInt16LE(entries.length, 4);
  const dirLen = 16 * entries.length;
  let offset = 6 + dirLen;
  const dir = Buffer.alloc(dirLen);
  const bufs = [];
  entries.forEach((e, i) => {
    const d = dir.subarray(i * 16, (i + 1) * 16);
    d[0] = e.size >= 256 ? 0 : e.size; // 256 记 0
    d[1] = e.size >= 256 ? 0 : e.size;
    d[2] = 0; // 调色板颜色数
    d[3] = 0; // reserved
    d.writeUInt16LE(1, 4);  // planes
    d.writeUInt16LE(32, 6); // bpp
    d.writeUInt32LE(e.png.length, 8);
    d.writeUInt32LE(offset, 12);
    offset += e.png.length;
    bufs.push(e.png);
  });
  return Buffer.concat([header, dir, ...bufs]);
}

function buildIcns(entries /* [{type, png}] */) {
  const body = Buffer.concat(
    entries.map((e) => {
      const head = Buffer.alloc(8);
      head.write(e.type, 0, "latin1");
      head.writeUInt32BE(8 + e.png.length, 4);
      return Buffer.concat([head, e.png]);
    }),
  );
  const head = Buffer.alloc(8);
  head.write("icns", 0, "latin1");
  head.writeUInt32BE(8 + body.length, 4);
  return Buffer.concat([head, body]);
}

// ---------------------------------------------------------------------------
// 主流程
// ---------------------------------------------------------------------------

const srcPath = process.argv[2];
if (!srcPath) {
  console.error("用法：node scripts/gen-icons.mjs <source.png>");
  process.exit(1);
}

const decoded = decodePng(fs.readFileSync(srcPath));
console.log(`源图：${decoded.w}x${decoded.h}`);
const box = removeCheckerBackground(decoded);
const art = crop(decoded, box);

const write = (rel, buf) => {
  const p = path.join(ROOT, rel);
  fs.mkdirSync(path.dirname(p), { recursive: true });
  fs.writeFileSync(p, buf);
  console.log(`已写入 ${rel}（${(buf.length / 1024).toFixed(1)} KB）`);
};

// —— 按需渲染任意方形尺寸 ——
const rendered = new Map();
const pngOf = (size) => {
  if (!rendered.has(size)) rendered.set(size, renderSquare(art, size));
  return encodePng(rendered.get(size));
};

// —— src-tauri/icons（exe / 窗口 / 托盘 / 打包）——
write("src-tauri/icons/32x32.png", pngOf(32));
write("src-tauri/icons/128x128.png", pngOf(128));
write("src-tauri/icons/128x128@2x.png", pngOf(256));
write("src-tauri/icons/icon.png", pngOf(512));

// icon.ico：Windows 用（exe 图标、标题栏、任务栏、托盘）
write(
  "src-tauri/icons/icon.ico",
  buildIco([16, 24, 32, 48, 64, 128, 256].map((size) => ({ size, png: pngOf(size) }))),
);

// icon.icns：macOS 打包用（PNG 数据块）
write(
  "src-tauri/icons/icon.icns",
  buildIcns([
    { type: "ic07", png: pngOf(128) },                 // 128x128
    { type: "ic08", png: pngOf(256) },                 // 256x256
    { type: "ic09", png: pngOf(512) },                 // 512x512
    { type: "ic10", png: pngOf(1024) },                // 1024x1024
    { type: "ic11", png: pngOf(32) },                  // 16x16@2x
    { type: "ic12", png: pngOf(64) },                  // 32x32@2x
    { type: "ic13", png: pngOf(256) },                 // 128x128@2x
    { type: "ic14", png: pngOf(512) },                 // 256x256@2x
  ]),
);

// —— MS Store / MSIX 磁贴（bundle.active 目前是 false，保持齐全以防以后开）——
for (const size of [30, 44, 71, 89, 107, 142, 150, 284, 310]) {
  write(`src-tauri/icons/Square${size}x${size}Logo.png`, pngOf(size));
}
write("src-tauri/icons/StoreLogo.png", pngOf(50));

// —— 界面左上角 Logo ——
write("src/assets/mushroom.png", pngOf(128));

// —— favicon.svg：内嵌 base64 PNG（浏览器 / 开发时标签页）——
const fav = pngOf(64);
write(
  "public/favicon.svg",
  `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" width="64" height="64">` +
    `<image width="64" height="64" href="data:image/png;base64,${fav.toString("base64")}"/></svg>`,
);

console.log("完成。");
