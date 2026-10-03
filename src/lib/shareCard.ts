/**
 * 经验小结卡片：把一段练级成绩画成**一张能直接发出去的卡**。
 *
 * 为什么是 canvas 而不是截图整个界面：
 * 界面截图里有一半是按钮、滚动条和别人的聊天框（悬浮在主窗口上的其它窗），
 * 发给群里既看不清重点也泄露多余信息。这里只画这一段的结论。
 *
 * 样子是一张**竖版集换卡**：上半是角色形象，下半是成绩，按这段打得怎么样分四档
 * 稀有度（N / R / SR / SSR），档位越高镭射越亮。两处要守住的规矩：
 *
 * 1. **稀有度只跟自己比**（最近那些历史记录），数据不完整的一段不参与评级 ——
 *    卡片是拿去发群的，不能让一段读不全的数据顶着「SSR」出去。
 * 2. **角色形象不靠识别**：是用户自己在游戏截图上框出来的一小块（见 ShareCardDialog），
 *    框一次存在本机，之后每张卡都用它。没框过就画一幅不带形象的卡面，不留空洞。
 *
 * 分工：`drawExpCard` 只画卡面（弹窗里的预览就是它，镭射由 CSS 跟着鼠标动）；
 * `renderShareImage` 把卡面摆到底板上、把镭射**烤进去**，得到发出去的那张静态图。
 */
import { invoke } from "@tauri-apps/api/core";
import { formatClock, formatDuration, formatNumber, splitBig } from "./format";

export interface ExpCardStage {
  level: number;
  secs: number;
  perHour: number | null;
  ongoing: boolean;
}

export interface ExpCardData {
  /** 练级地图（用户自己填的，可空 —— 空着就少画一行，不编一个地名上去） */
  mapName: string;
  /** 开始 / 结束时刻（秒级时间戳） */
  startedAt: number;
  endedAt: number;
  /** 会话内有效时间（秒，暂停那几段不算） */
  activeSecs: number;
  gainedExp: number;
  /** 每小时（按有效时间算）；没有可信速率时为 null */
  perHour: number | null;
  startLevel: number | null;
  startPercent: number | null;
  endLevel: number | null;
  endPercent: number | null;
  levelUps: number;
  stages: ExpCardStage[];
  /** 2 = 数据完整 / 1 = 可参考 / 0 = 不建议用于比较 */
  qualityGrade: number;
  qualityLabel: string;
  /** 能读到画面的时间占比（0~1） */
  coverage: number;
  /** 真正在涨经验的时间占比（0~1）；老记录里没有就是 null */
  activeRatio: number | null;
}

export type Rarity = "N" | "R" | "SR" | "SSR";

/** 卡面上那些「好玩」的部分：稀有度、称号、小标签。全部由数据推出来，不让用户自己选。 */
export interface CardFlair {
  rarity: Rarity;
  /** 称号（卡片左上角那几个字） */
  title: string;
  /** 为什么是这一档 —— 弹窗里写给用户看，免得他以为是随机抽的 */
  reason: string;
  tags: string[];
}

/** 评级至少要这么长的一段：两三分钟的时速抖动太大，拿来比没有意义。 */
const MIN_RATED_SECS = 300;
/** 历史不到这么多段就不做排名（一共两段，「超过 100%」是句空话）。 */
const MIN_HISTORY = 3;

const TIERS: Rarity[] = ["N", "R", "SR", "SSR"];

/**
 * 这一段配什么稀有度 / 称号。
 *
 * `pastRates` 是**别的**历史记录的时速（调用方负责把这一段自己剔掉、把太短和
 * 数据不完整的剔掉）。只跟自己的历史比：别人的等级、职业、地图都不一样，没法比。
 */
export function cardFlair(data: ExpCardData, pastRates: number[]): CardFlair {
  const tags: string[] = [];
  let tier = 0;
  let reason: string;
  let record = false;

  const rated =
    data.qualityGrade > 0 && data.perHour != null && data.activeSecs >= MIN_RATED_SECS;
  if (!rated) {
    reason =
      data.qualityGrade <= 0
        ? "这一段数据不完整，不参与评级"
        : `不满 ${Math.round(MIN_RATED_SECS / 60)} 分钟的一段不参与评级`;
  } else if (pastRates.length >= MIN_HISTORY) {
    const rate = data.perHour ?? 0;
    const beaten = pastRates.filter((past) => past < rate).length;
    const share = beaten / pastRates.length;
    if (beaten === pastRates.length) {
      tier = 3;
      record = true;
      reason = `比最近 ${pastRates.length} 段都快，刷新了你的最快时速`;
      tags.push("个人最快");
    } else {
      const percent = Math.round(share * 100);
      tier = share >= 0.75 ? 2 : share >= 0.4 ? 1 : 0;
      reason = `时速超过了你最近 ${pastRates.length} 段里的 ${percent}%`;
      if (tier > 0) tags.push(`超过 ${percent}% 历史`);
    }
  } else {
    // 历史还不够排名：按这一段自己的分量给，至少不让每张卡都是 N
    tier = data.activeSecs >= 3600 ? 1 : 0;
    reason = `历史不足 ${MIN_HISTORY} 段，攒够了才按时速排名`;
  }

  if (rated && data.levelUps > 0 && tier < 2) {
    tier += 1;
    reason += "；这一段升了级，提一档";
  }
  if (data.levelUps > 0) tags.push(`升级 ×${data.levelUps}`);
  if (data.activeSecs >= 3600) tags.push(`连续 ${formatDuration(data.activeSecs)}`);
  if (data.activeRatio != null && data.activeSecs >= MIN_RATED_SECS) {
    tags.push(`有效打怪 ${Math.round(data.activeRatio * 100)}%`);
  }

  return {
    rarity: TIERS[tier],
    title: cardTitle(data, record),
    reason,
    tags: tags.slice(0, 3),
  };
}

/** 称号：挑这一段最值得说的那一件事。 */
function cardTitle(data: ExpCardData, record: boolean): string {
  if (record) return "新纪录";
  if (data.levelUps >= 3) return "连升三级";
  if (data.levelUps > 0) return "升级啦";
  if (data.activeSecs >= 4 * 3600) return "肝帝";
  if (data.activeSecs >= 2 * 3600) return "马拉松";
  const startHour = new Date(data.startedAt * 1000).getHours();
  if (data.startedAt > 0 && startHour < 5) return "夜猫子";
  if (data.activeRatio != null && data.activeRatio >= 0.95 && data.activeSecs >= 1800) {
    return "全程无休";
  }
  if (data.activeSecs < 900) return "热身";
  return "稳扎稳打";
}

// ---------------------------------------------------------------------------
// 卡面
// ---------------------------------------------------------------------------

/** 卡面的逻辑尺寸；真实 canvas 按 `CARD_SCALE` 放大，发出去的字才不糊。 */
export const CARD_W = 600;
export const CARD_H = 840;
const CARD_R = 30;
const CARD_SCALE = 2;
const PAD = 28;

/** 形象窗：角色那一块。裁剪框用同一个宽高比，框出来多大卡上就是多大。 */
const ART_X = PAD;
const ART_Y = 86;
const ART_W = CARD_W - PAD * 2;
const ART_H = 320;
export const ART_ASPECT = ART_W / ART_H;
/** 形象窗底部那条字（地图名 / 时间段）占的高度 */
const CAPTION_H = 48;

const FONT_STACK = '"Microsoft YaHei UI", "Microsoft YaHei", "PingFang SC", sans-serif';
/** 大数字用系统自带的 Bahnschrift（窄、硬朗）；没有这个字体的机器落回雅黑 */
const DIGIT_STACK = `"Bahnschrift", "DIN Alternate", ${FONT_STACK}`;
const text = (size: number, weight = 400) => `${weight} ${size}px ${FONT_STACK}`;
const digits = (size: number, weight = 700) => `${weight} ${size}px ${DIGIT_STACK}`;

const INK = "#f4ece0";
const MUTED = "#a89c8a";
const AMBER = "#f5a04a";
const EMERALD = "#5fd3a0";

interface RarityStyle {
  /** 边框的渐变色（从左上到右下） */
  frame: string[];
  accent: string;
  /** 镭射强度 0~1：预览里的 CSS 和导出时烤进去的那层都按它来 */
  foil: number;
}

export const RARITY_STYLE: Record<Rarity, RarityStyle> = {
  N: { frame: ["#a39584", "#5f5549", "#8a7d6d"], accent: "#cbbba6", foil: 0.12 },
  R: { frame: ["#9ad8ff", "#3f78dc", "#8fd0ff"], accent: "#8fd0ff", foil: 0.45 },
  SR: { frame: ["#e3b3ff", "#7d55ff", "#ffb877"], accent: "#d9aaff", foil: 0.72 },
  SSR: { frame: ["#fff0a8", "#ff9f43", "#ff7ab8", "#7ad7ff", "#ffe38a"], accent: "#ffd76a", foil: 1 },
};

function qualityColor(grade: number): string {
  if (grade >= 2) return EMERALD;
  return grade === 1 ? AMBER : "#f08a7c";
}

function levelPercent(level: number | null, percent: number | null): string {
  if (level == null) return "Lv.—";
  return `Lv.${level}${percent != null ? ` ${percent.toFixed(2)}%` : ""}`;
}

function roundRect(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  w: number,
  h: number,
  r: number,
) {
  const radius = Math.min(r, w / 2, h / 2);
  ctx.beginPath();
  ctx.moveTo(x + radius, y);
  ctx.lineTo(x + w - radius, y);
  ctx.arcTo(x + w, y, x + w, y + radius, radius);
  ctx.lineTo(x + w, y + h - radius);
  ctx.arcTo(x + w, y + h, x + w - radius, y + h, radius);
  ctx.lineTo(x + radius, y + h);
  ctx.arcTo(x, y + h, x, y + h - radius, radius);
  ctx.lineTo(x, y + radius);
  ctx.arcTo(x, y, x + radius, y, radius);
  ctx.closePath();
}

function gradient(
  ctx: CanvasRenderingContext2D,
  x0: number,
  y0: number,
  x1: number,
  y1: number,
  colors: string[],
): CanvasGradient {
  const fill = ctx.createLinearGradient(x0, y0, x1, y1);
  colors.forEach((color, index) => fill.addColorStop(index / Math.max(1, colors.length - 1), color));
  return fill;
}

/** 给一行字找个放得下的字号（不换行 —— 换行会把下面的版式全顶下去）。 */
function fitText(
  ctx: CanvasRenderingContext2D,
  value: string,
  room: number,
  size: number,
  font: (size: number) => string,
  floor = 12,
): number {
  let current = size;
  ctx.font = font(current);
  while (current > floor && ctx.measureText(value).width > room) {
    current -= 1;
    ctx.font = font(current);
  }
  return current;
}

/** 可重复的随机数：同一段每次画出来的星点都在同一个位置（预览和导出对得上）。 */
function seeded(seed: number): () => number {
  let state = seed >>> 0 || 1;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let mixed = Math.imul(state ^ (state >>> 15), 1 | state);
    mixed = (mixed + Math.imul(mixed ^ (mixed >>> 7), 61 | mixed)) ^ mixed;
    return ((mixed ^ (mixed >>> 14)) >>> 0) / 4294967296;
  };
}

/** 四角星：镭射卡上那种一闪一闪的小亮点。 */
function sparkle(ctx: CanvasRenderingContext2D, x: number, y: number, size: number) {
  ctx.beginPath();
  ctx.moveTo(x, y - size);
  ctx.quadraticCurveTo(x, y, x + size, y);
  ctx.quadraticCurveTo(x, y, x, y + size);
  ctx.quadraticCurveTo(x, y, x - size, y);
  ctx.quadraticCurveTo(x, y, x, y - size);
  ctx.closePath();
  ctx.fill();
}

/** 形象窗的底：放射光 + 一团光晕。没有形象、或形象去过背景时垫在后面。 */
function drawArtBackdrop(ctx: CanvasRenderingContext2D, style: RarityStyle) {
  const cx = ART_X + ART_W / 2;
  const cy = ART_Y + ART_H / 2 - 6;
  ctx.fillStyle = gradient(ctx, ART_X, ART_Y, ART_X, ART_Y + ART_H, ["#33261b", "#17110d"]);
  ctx.fillRect(ART_X, ART_Y, ART_W, ART_H);

  ctx.save();
  ctx.translate(cx, cy);
  ctx.globalAlpha = 0.1;
  ctx.fillStyle = style.accent;
  const rays = 18;
  for (let index = 0; index < rays; index += 1) {
    ctx.rotate((Math.PI * 2) / rays);
    ctx.beginPath();
    ctx.moveTo(0, 0);
    ctx.lineTo(-22, -ART_W);
    ctx.lineTo(22, -ART_W);
    ctx.closePath();
    ctx.fill();
  }
  ctx.restore();

  const halo = ctx.createRadialGradient(cx, cy, 10, cx, cy, 210);
  halo.addColorStop(0, "rgba(255,255,255,0.16)");
  halo.addColorStop(1, "rgba(255,255,255,0)");
  ctx.fillStyle = halo;
  ctx.fillRect(ART_X, ART_Y, ART_W, ART_H);
}

/** 没有形象时的卡面：一个大大的等级。不留一块「此处应有图」的空洞。 */
function drawEmblem(ctx: CanvasRenderingContext2D, data: ExpCardData, style: RarityStyle) {
  const cx = ART_X + ART_W / 2;
  const cy = ART_Y + ART_H / 2 - 6;
  drawArtBackdrop(ctx, style);

  ctx.strokeStyle = style.accent;
  ctx.globalAlpha = 0.5;
  ctx.lineWidth = 2;
  ctx.beginPath();
  ctx.arc(cx, cy, 104, 0, Math.PI * 2);
  ctx.stroke();
  ctx.globalAlpha = 0.25;
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.arc(cx, cy, 116, 0, Math.PI * 2);
  ctx.stroke();
  ctx.globalAlpha = 1;

  const level = data.endLevel != null ? String(data.endLevel) : "—";
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  ctx.font = digits(22, 600);
  ctx.fillStyle = MUTED;
  ctx.fillText("LEVEL", cx, cy - 58);
  ctx.font = digits(108, 700);
  ctx.fillStyle = gradient(ctx, cx, cy - 50, cx, cy + 60, ["#ffffff", style.accent]);
  ctx.fillText(level, cx, cy + 12);
  ctx.textAlign = "left";
  ctx.textBaseline = "top";
}

/** 卡面上用的角色形象。 */
export interface CardAvatar {
  image: HTMLImageElement;
  /**
   * 这张图带透明（导入的透明 PNG，比如纸娃娃模拟器导出的）：整个摆进形象窗、
   * 垫上卡面自己的底、描一圈光；不带透明的就是一块矩形画面（游戏截图），铺满形象窗。
   */
  cutout: boolean;
}

/**
 * 把卡面画到 `canvas` 上（不含镭射）。
 *
 * 形象放大时关掉平滑 —— 像素角色糊成一团最难看。
 * `serial` 是「第几段」，没有就不画编号。
 */
export function drawExpCard(
  canvas: HTMLCanvasElement,
  data: ExpCardData,
  flair: CardFlair,
  avatar: CardAvatar | null,
  serial: number | null,
) {
  canvas.width = CARD_W * CARD_SCALE;
  canvas.height = CARD_H * CARD_SCALE;
  const ctx = canvas.getContext("2d");
  // 画不了就留一张空卡：导出时 renderShareImage 会把「canvas 不可用」报出来
  if (!ctx) return;
  ctx.setTransform(CARD_SCALE, 0, 0, CARD_SCALE, 0, 0);
  ctx.clearRect(0, 0, CARD_W, CARD_H);

  const style = RARITY_STYLE[flair.rarity];
  const frame = gradient(ctx, 0, 0, CARD_W, CARD_H, style.frame);
  const right = CARD_W - PAD;

  ctx.save();
  roundRect(ctx, 0, 0, CARD_W, CARD_H, CARD_R);
  ctx.clip();

  // ---- 底：暖夜色（和软件界面同一套），顶上透一点稀有度的颜色 ----
  ctx.fillStyle = gradient(ctx, 0, 0, 0, CARD_H, ["#2a1f17", "#17110d", "#100c09"]);
  ctx.fillRect(0, 0, CARD_W, CARD_H);
  const tint = ctx.createRadialGradient(CARD_W / 2, 0, 20, CARD_W / 2, 0, 520);
  tint.addColorStop(0, style.accent);
  tint.addColorStop(1, "rgba(0,0,0,0)");
  ctx.globalAlpha = 0.2;
  ctx.fillStyle = tint;
  ctx.fillRect(0, 0, CARD_W, CARD_H);
  // 斜纹：很淡，只是让大片底色不那么「平」
  ctx.globalAlpha = 0.035;
  ctx.strokeStyle = "#ffffff";
  ctx.lineWidth = 1;
  for (let offset = -CARD_H; offset < CARD_W; offset += 14) {
    ctx.beginPath();
    ctx.moveTo(offset, 0);
    ctx.lineTo(offset + CARD_H, CARD_H);
    ctx.stroke();
  }
  ctx.globalAlpha = 1;
  ctx.textBaseline = "top";
  ctx.textAlign = "left";

  // ---- 头部：稀有度 + 称号，右边编号和日期 ----
  ctx.font = `italic ${digits(22, 700)}`;
  const chipW = Math.max(52, ctx.measureText(flair.rarity).width + 28);
  ctx.fillStyle = frame;
  roundRect(ctx, PAD, 30, chipW, 36, 10);
  ctx.fill();
  ctx.fillStyle = "#1a120b";
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  ctx.fillText(flair.rarity, PAD + chipW / 2 - 1, 49);
  ctx.textAlign = "left";
  ctx.textBaseline = "top";

  ctx.font = text(24, 800);
  ctx.fillStyle = INK;
  ctx.fillText(flair.title, PAD + chipW + 14, 34);

  const stamp = new Date(data.endedAt * 1000);
  const dateText = `${stamp.getFullYear()}.${String(stamp.getMonth() + 1).padStart(2, "0")}.${String(
    stamp.getDate(),
  ).padStart(2, "0")}`;
  ctx.textAlign = "right";
  if (serial != null) {
    ctx.font = digits(18, 700);
    ctx.fillStyle = style.accent;
    ctx.fillText(`No.${String(serial).padStart(3, "0")}`, right, 30);
  }
  if (data.endedAt > 0) {
    ctx.font = digits(13, 400);
    ctx.fillStyle = MUTED;
    ctx.fillText(dateText, right, serial != null ? 53 : 42);
  }
  ctx.textAlign = "left";

  // ---- 形象窗 ----
  ctx.save();
  roundRect(ctx, ART_X, ART_Y, ART_W, ART_H, 18);
  ctx.clip();
  // 底部压暗：地图名写在画面上，得有地方衬着
  const shadeArt = () => {
    const shade = ctx.createLinearGradient(0, ART_Y + ART_H - 120, 0, ART_Y + ART_H);
    shade.addColorStop(0, "rgba(12,9,7,0)");
    shade.addColorStop(1, "rgba(12,9,7,0.88)");
    ctx.fillStyle = shade;
    ctx.fillRect(ART_X, ART_Y + ART_H - 120, ART_W, 120);
  };
  if (avatar?.cutout) {
    // 带透明的形象：整个摆进来（不裁），站在那条字的上面，身后是卡面自己的光
    drawArtBackdrop(ctx, style);
    shadeArt();
    const sourceW = avatar.image.naturalWidth;
    const sourceH = avatar.image.naturalHeight;
    // 底下留一条给地图名和时间：形象站在这条字的上面，不和它叠在一起
    const room = ART_H - CAPTION_H - 16;
    let scale = Math.min((ART_W - 36) / sourceW, room / sourceH);
    // 放大两倍以上时凑成整数倍：非整数倍的最近邻放大，像素会一格宽一格窄
    const device = scale * CARD_SCALE;
    if (device >= 2) scale = Math.floor(device) / CARD_SCALE;
    const drawW = sourceW * scale;
    const drawH = sourceH * scale;
    ctx.imageSmoothingEnabled = scale * CARD_SCALE < 1.25;
    // 描一圈稀有度颜色的光：把形象从底上「托」出来
    ctx.shadowColor = style.accent;
    ctx.shadowBlur = 22;
    ctx.drawImage(
      avatar.image,
      ART_X + (ART_W - drawW) / 2,
      ART_Y + ART_H - CAPTION_H - drawH,
      drawW,
      drawH,
    );
    ctx.shadowBlur = 0;
    ctx.shadowColor = "transparent";
    ctx.imageSmoothingEnabled = true;
  } else if (avatar) {
    const sourceW = avatar.image.naturalWidth;
    const sourceH = avatar.image.naturalHeight;
    // 按「填满」摆：比例不一样时裁掉多出来的两边，不拉伸
    let cropW = sourceW;
    let cropH = sourceH;
    if (sourceW / sourceH > ART_ASPECT) cropW = sourceH * ART_ASPECT;
    else cropH = sourceW / ART_ASPECT;
    ctx.imageSmoothingEnabled = (ART_W * CARD_SCALE) / cropW < 1.25;
    ctx.drawImage(
      avatar.image,
      (sourceW - cropW) / 2,
      (sourceH - cropH) / 2,
      cropW,
      cropH,
      ART_X,
      ART_Y,
      ART_W,
      ART_H,
    );
    ctx.imageSmoothingEnabled = true;
    shadeArt();
  } else {
    drawEmblem(ctx, data, style);
    shadeArt();
  }
  ctx.restore();
  ctx.strokeStyle = frame;
  ctx.lineWidth = 2;
  roundRect(ctx, ART_X, ART_Y, ART_W, ART_H, 18);
  ctx.stroke();

  const span =
    data.startedAt > 0 ? `${formatClock(data.startedAt)} → ${formatClock(data.endedAt)}` : "";
  ctx.font = digits(15, 400);
  const spanW = span ? ctx.measureText(span).width : 0;
  const map = data.mapName.trim();
  ctx.shadowColor = "rgba(0,0,0,0.8)";
  ctx.shadowBlur = 8;
  if (map) {
    // 没填地图就不编一个：这一行直接不画
    const mapSize = fitText(ctx, map, ART_W - 40 - spanW - 24, 26, (size) => text(size, 800), 16);
    ctx.fillStyle = INK;
    ctx.fillText(map, ART_X + 20, ART_Y + ART_H - 22 - mapSize);
  }
  if (span) {
    ctx.font = digits(15, 400);
    ctx.fillStyle = "#e6dccd";
    ctx.textAlign = "right";
    ctx.fillText(span, ART_X + ART_W - 20, ART_Y + ART_H - 38);
    ctx.textAlign = "left";
  }
  ctx.shadowBlur = 0;
  ctx.shadowColor = "transparent";

  // ---- 等级区间 + 进度条（亮的那一截 = 这一段打出来的）----
  let y = ART_Y + ART_H + 20;
  const from = levelPercent(data.startLevel, data.startPercent);
  const to = levelPercent(data.endLevel, data.endPercent);
  ctx.font = digits(20, 600);
  ctx.fillStyle = "#d8ccba";
  ctx.fillText(from, PAD, y);
  let cursor = PAD + ctx.measureText(from).width + 10;
  ctx.fillStyle = MUTED;
  ctx.fillText("→", cursor, y);
  cursor += ctx.measureText("→").width + 10;
  ctx.font = digits(20, 700);
  ctx.fillStyle = style.accent;
  ctx.fillText(to, cursor, y);
  if (data.levelUps > 0) {
    ctx.font = text(14, 700);
    ctx.fillStyle = EMERALD;
    ctx.textAlign = "right";
    ctx.fillText(`升级 ${data.levelUps} 次`, right, y + 3);
    ctx.textAlign = "left";
  }
  y += 32;
  const barW = CARD_W - PAD * 2;
  ctx.fillStyle = "rgba(255,255,255,0.09)";
  roundRect(ctx, PAD, y, barW, 8, 4);
  ctx.fill();
  if (data.endPercent != null) {
    const end = Math.min(100, Math.max(0, data.endPercent)) / 100;
    const sameLevel = data.levelUps === 0 && data.startPercent != null;
    const start = sameLevel ? Math.min(end, Math.max(0, (data.startPercent ?? 0) / 100)) : 0;
    if (start > 0) {
      ctx.fillStyle = "rgba(255,255,255,0.22)";
      roundRect(ctx, PAD, y, barW * start, 8, 4);
      ctx.fill();
    }
    const gainW = Math.max(6, barW * (end - start));
    ctx.fillStyle = frame;
    roundRect(ctx, PAD + barW * start, y, Math.min(gainW, barW - barW * start), 8, 4);
    ctx.fill();
  }
  y += 28;

  // ---- 主数字：每小时经验 ----
  ctx.font = text(13, 400);
  ctx.fillStyle = MUTED;
  ctx.fillText("每小时经验（按有效时间）", PAD, y);
  y += 18;
  const hour = splitBig(data.perHour);
  const heroSize = fitText(ctx, hour.n, barW - 150, 76, (size) => digits(size, 700), 40);
  ctx.fillStyle = gradient(ctx, 0, y, 0, y + heroSize, ["#fff6df", style.accent]);
  ctx.fillText(hour.n, PAD - 2, y + (76 - heroSize) / 2);
  if (data.perHour != null) {
    const numberW = ctx.measureText(hour.n).width;
    ctx.font = text(20, 700);
    ctx.fillStyle = "#d8ccba";
    ctx.fillText(`${hour.unit} / 时`.trim(), PAD + numberW + 10, y + 42);
  }
  y += 86;

  // ---- 三个小格：获得 / 时长 / 有效打怪（或画面覆盖）----
  const gained = splitBig(data.gainedExp);
  const third =
    data.activeRatio != null
      ? { label: "有效打怪", value: `${Math.round(data.activeRatio * 100)}`, unit: "%" }
      : { label: "画面覆盖", value: `${Math.round(data.coverage * 100)}`, unit: "%" };
  const tiles = [
    { label: "获得经验", value: `+${gained.n}`, unit: gained.unit },
    { label: "有效时长", value: formatDuration(data.activeSecs).replace(/\s/g, ""), unit: "" },
    third,
  ];
  const gap = 12;
  const tileW = (barW - gap * 2) / 3;
  tiles.forEach((tile, index) => {
    const x = PAD + index * (tileW + gap);
    ctx.fillStyle = "rgba(255,255,255,0.055)";
    roundRect(ctx, x, y, tileW, 76, 14);
    ctx.fill();
    ctx.strokeStyle = "rgba(255,255,255,0.09)";
    ctx.lineWidth = 1;
    roundRect(ctx, x, y, tileW, 76, 14);
    ctx.stroke();
    ctx.font = text(12, 400);
    ctx.fillStyle = MUTED;
    ctx.fillText(tile.label, x + 14, y + 12);
    // 中文时长（「2小时43分」）没有数字字体，用雅黑；纯数字的用 Bahnschrift
    const numeric = /^[+\d.,]+$/.test(tile.value);
    const font = (size: number) => (numeric ? digits(size, 700) : text(size - 4, 700));
    const unitW = tile.unit ? 22 : 0;
    const size = fitText(ctx, tile.value, tileW - 28 - unitW, 28, font, 16);
    ctx.fillStyle = INK;
    ctx.fillText(tile.value, x + 14, y + 34 + (28 - size) / 2);
    if (tile.unit) {
      const valueW = ctx.measureText(tile.value).width;
      ctx.font = text(14, 700);
      ctx.fillStyle = "#d8ccba";
      ctx.fillText(tile.unit, x + 14 + valueW + 5, y + 44);
    }
  });
  y += 76 + 18;

  // ---- 小标签（成就）+ 分段效率 ----
  let chipX = PAD;
  ctx.font = text(13, 700);
  flair.tags.forEach((tag) => {
    const width = ctx.measureText(tag).width + 24;
    if (chipX + width > right) return;
    ctx.fillStyle = "rgba(255,255,255,0.05)";
    roundRect(ctx, chipX, y, width, 28, 14);
    ctx.fill();
    ctx.strokeStyle = style.accent;
    ctx.globalAlpha = 0.55;
    roundRect(ctx, chipX, y, width, 28, 14);
    ctx.stroke();
    ctx.globalAlpha = 1;
    ctx.fillStyle = style.accent;
    ctx.fillText(tag, chipX + 12, y + 6);
    chipX += width + 8;
  });
  if (flair.tags.length > 0) y += 28 + 14;

  // 每一级各自的真实时速；没升过级就写精确的经验数（上面的「万」是约数）
  const stages = data.stages.filter((stage) => stage.perHour != null);
  const detail =
    stages.length > 1
      ? stages
          .slice(-3)
          .map((stage) => {
            const rate = splitBig(stage.perHour);
            return `Lv.${stage.level} ${rate.n}${rate.unit}/时`;
          })
          .join("  ·  ")
      : `净经验 ${formatNumber(data.gainedExp)}`;
  fitText(ctx, detail, barW, 13, (size) => text(size, 400), 10);
  ctx.fillStyle = MUTED;
  ctx.fillText(detail, PAD, y);

  // ---- 页脚：软件名（宣传）+ 数据可信度 ----
  const footY = CARD_H - 74;
  ctx.strokeStyle = "rgba(255,255,255,0.12)";
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(PAD, footY);
  ctx.lineTo(right, footY);
  ctx.stroke();
  ctx.font = text(17, 800);
  ctx.fillStyle = AMBER;
  ctx.fillText("枫之助", PAD, footY + 14);
  const brandW = ctx.measureText("枫之助").width;
  ctx.font = text(12, 400);
  ctx.fillStyle = MUTED;
  ctx.fillText("冒险岛怀旧服小助手", PAD + brandW + 8, footY + 19);
  ctx.fillText("本机读数 · 不联网不上传", PAD, footY + 40);

  // 可信度和成绩分开一格：别人拿去比较前能自己判断
  const tone = qualityColor(data.qualityGrade);
  const badge = `${data.qualityLabel} · 画面 ${Math.round(data.coverage * 100)}%`;
  ctx.font = text(12, 700);
  const badgeW = ctx.measureText(badge).width + 22;
  ctx.strokeStyle = tone;
  roundRect(ctx, right - badgeW, footY + 18, badgeW, 26, 13);
  ctx.stroke();
  ctx.fillStyle = tone;
  ctx.fillText(badge, right - badgeW + 11, footY + 24);

  ctx.restore();

  // ---- 边框：画在最后，压住所有内容的边缘 ----
  ctx.strokeStyle = frame;
  ctx.lineWidth = 5;
  roundRect(ctx, 2.5, 2.5, CARD_W - 5, CARD_H - 5, CARD_R - 2);
  ctx.stroke();
  ctx.strokeStyle = "rgba(255,255,255,0.16)";
  ctx.lineWidth = 1;
  roundRect(ctx, 10.5, 10.5, CARD_W - 21, CARD_H - 21, CARD_R - 9);
  ctx.stroke();
}

// ---------------------------------------------------------------------------
// 导出
// ---------------------------------------------------------------------------

/** 卡片四周留的底板宽度（逻辑像素）：放得下投影，贴进聊天框也不顶着气泡边 */
const MARGIN = 40;
/**
 * 导出倍率。卡面本身是 2 倍画的，这里收一档 ——
 * 2 倍的整图过 IPC 要传好几 MB 的 base64，而手机上看 1000 像素宽已经足够清楚。
 */
const EXPORT_SCALE = 1.5;

const RAINBOW = ["#ff7a7a", "#ffe066", "#8dff8a", "#7df3ff", "#8a9bff", "#e38aff", "#ff7a7a"];

/**
 * 把镭射**烤**到静态图上：彩虹斜带 + 一道高光 + 星点。
 *
 * 预览里这些是跟着鼠标动的 CSS 图层；发出去的是 PNG，动不了，
 * 所以挑一个好看的角度定格。强度跟稀有度走，N 卡几乎没有。
 */
function bakeFoil(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  rarity: Rarity,
  seed: number,
) {
  const foil = RARITY_STYLE[rarity].foil;
  ctx.save();
  roundRect(ctx, x, y, CARD_W, CARD_H, CARD_R);
  ctx.clip();

  // 彩虹斜带：重复五轮（带子细一点才像镭射膜，宽了就成了一块块色斑），角度和 CSS 里那层一致
  const bands: string[] = [];
  for (let round = 0; round < 5; round += 1) bands.push(...RAINBOW.slice(round === 0 ? 0 : 1));
  ctx.globalCompositeOperation = "color-dodge";
  ctx.globalAlpha = 0.13 * foil;
  ctx.fillStyle = gradient(ctx, x, y + CARD_H, x + CARD_W, y, bands);
  ctx.fillRect(x, y, CARD_W, CARD_H);

  // 一道斜着扫过去的高光
  ctx.globalCompositeOperation = "soft-light";
  ctx.globalAlpha = 0.25 + 0.45 * foil;
  const sweep = ctx.createLinearGradient(x, y + CARD_H * 0.55, x + CARD_W, y + CARD_H * 0.1);
  sweep.addColorStop(0.3, "rgba(255,255,255,0)");
  sweep.addColorStop(0.5, "rgba(255,255,255,0.85)");
  sweep.addColorStop(0.7, "rgba(255,255,255,0)");
  ctx.fillStyle = sweep;
  ctx.fillRect(x, y, CARD_W, CARD_H);

  // 星点：只有 SR 以上才撒
  if (foil > 0.6) {
    const random = seeded(seed);
    ctx.globalCompositeOperation = "lighter";
    const count = Math.round(46 * foil);
    for (let index = 0; index < count; index += 1) {
      const px = x + random() * CARD_W;
      const py = y + random() * CARD_H;
      const size = 2 + random() * 6;
      ctx.globalAlpha = 0.25 + random() * 0.5;
      ctx.fillStyle = RAINBOW[Math.floor(random() * (RAINBOW.length - 1))];
      sparkle(ctx, px, py, size);
      ctx.globalAlpha *= 0.9;
      ctx.fillStyle = "#ffffff";
      sparkle(ctx, px, py, size * 0.45);
    }
  }
  ctx.restore();
}

/**
 * 发出去的那张图：底板 + 投影 + 卡面 + 烤进去的镭射。
 * 返回 base64（不含 `data:image/png;base64,` 前缀）。
 *
 * 四周是**实心**底板而不是透明：微信 / QQ 贴图时透明角会变成黑块或白块，看平台心情。
 */
export function renderShareImage(card: HTMLCanvasElement, flair: CardFlair, seed: number): string {
  const out = document.createElement("canvas");
  const width = CARD_W + MARGIN * 2;
  const height = CARD_H + MARGIN * 2;
  out.width = Math.round(width * EXPORT_SCALE);
  out.height = Math.round(height * EXPORT_SCALE);
  const ctx = out.getContext("2d");
  if (!ctx) throw new Error("这台机器上画不出图片（canvas 不可用）");
  ctx.scale(EXPORT_SCALE, EXPORT_SCALE);

  const style = RARITY_STYLE[flair.rarity];
  ctx.fillStyle = "#0e0a08";
  ctx.fillRect(0, 0, width, height);
  const glow = ctx.createRadialGradient(width / 2, height / 2, 80, width / 2, height / 2, height * 0.62);
  glow.addColorStop(0, style.accent);
  glow.addColorStop(1, "rgba(0,0,0,0)");
  ctx.globalAlpha = 0.1 + 0.16 * style.foil;
  ctx.fillStyle = glow;
  ctx.fillRect(0, 0, width, height);
  ctx.globalAlpha = 1;

  ctx.save();
  ctx.shadowColor = "rgba(0,0,0,0.65)";
  ctx.shadowBlur = 30;
  ctx.shadowOffsetY = 10;
  ctx.fillStyle = "#17110d";
  roundRect(ctx, MARGIN, MARGIN, CARD_W, CARD_H, CARD_R);
  ctx.fill();
  ctx.restore();
  ctx.drawImage(card, MARGIN, MARGIN, CARD_W, CARD_H);
  bakeFoil(ctx, MARGIN, MARGIN, flair.rarity, seed);

  const dataUrl = out.toDataURL("image/png");
  const comma = dataUrl.indexOf(",");
  if (comma < 0) throw new Error("图片编码失败");
  return dataUrl.slice(comma + 1);
}

/**
 * 存到 `图片\枫之助`，返回图片路径（落盘、校验 PNG 头、在资源管理器里选中都在 Rust 那边）。
 *
 * 文件名带上地图名：一次练好几个图、连着生成好几张卡片时，光看文件名就能分辨。
 */
export function saveShareImage(pngBase64: string, mapName: string): Promise<string> {
  const map = mapName.trim();
  const fileName = map ? `${map}-经验小结` : "经验小结";
  return invoke<string>("save_share_card", { pngBase64, fileName });
}

/** 放进剪贴板：到微信 / QQ 的聊天框里 Ctrl+V 就是一张图。 */
export function copyShareImage(pngBase64: string): Promise<void> {
  return invoke<void>("copy_share_card", { pngBase64 });
}

// ---------------------------------------------------------------------------
// 角色形象（存在本机）
// ---------------------------------------------------------------------------

const AVATAR_KEY = "mxd-box:share-card:avatar";
const CROP_KEY = "mxd-box:share-card:crop";

/** 裁剪框在游戏画面里的位置（都是占画面宽 / 高的比例，换分辨率也还对得上）。 */
export interface AvatarCrop {
  /** 框中心 */
  cx: number;
  cy: number;
  /** 框宽占画面宽的比例；高由 `ART_ASPECT` 定 */
  width: number;
}

/** 角色一般就在画面正中偏下一点：第一次打开时框先摆在那儿，用户再拖。 */
const DEFAULT_CROP: AvatarCrop = { cx: 0.5, cy: 0.55, width: 0.22 };

export function loadAvatar(): string | null {
  try {
    return window.localStorage.getItem(AVATAR_KEY);
  } catch {
    return null;
  }
}

/** 存进本机，返回存没存上。存不下（配额满）时这一次的卡照样能画，只是下次要重新选。 */
export function storeAvatar(dataUrl: string | null, crop?: AvatarCrop): boolean {
  try {
    if (dataUrl == null) window.localStorage.removeItem(AVATAR_KEY);
    else window.localStorage.setItem(AVATAR_KEY, dataUrl);
    if (crop) window.localStorage.setItem(CROP_KEY, JSON.stringify(crop));
    return true;
  } catch {
    return false;
  }
}

export function loadCrop(): AvatarCrop {
  try {
    const raw = window.localStorage.getItem(CROP_KEY);
    const parsed = raw ? (JSON.parse(raw) as Partial<AvatarCrop>) : null;
    if (
      parsed &&
      [parsed.cx, parsed.cy, parsed.width].every(
        (value) => typeof value === "number" && value > 0 && value <= 1,
      )
    ) {
      return { cx: parsed.cx, cy: parsed.cy, width: parsed.width } as AvatarCrop;
    }
  } catch {
    // 存的东西坏了就当没存过
  }
  return DEFAULT_CROP;
}

/** 这张图有没有透明的部分（导入的透明 PNG）—— 决定它在卡面上怎么摆。 */
export function hasTransparency(image: HTMLImageElement): boolean {
  const canvas = document.createElement("canvas");
  canvas.width = image.naturalWidth;
  canvas.height = image.naturalHeight;
  const context = canvas.getContext("2d", { willReadFrequently: true });
  if (!context || canvas.width === 0 || canvas.height === 0) return false;
  context.drawImage(image, 0, 0);
  const { data } = context.getImageData(0, 0, canvas.width, canvas.height);
  let clear = 0;
  for (let offset = 3; offset < data.length; offset += 4) {
    if (data[offset] < 128) clear += 1;
  }
  // 几个零星的透明点不算（有的截图工具会在角上留一两个）
  return clear / (data.length / 4) > 0.03;
}

/** 预览里那层会闪的星点贴图（一小块平铺的 PNG）；只生成一次。 */
let sparkleTile: string | null = null;
export function sparkleTexture(): string {
  if (sparkleTile) return sparkleTile;
  const size = 220;
  const canvas = document.createElement("canvas");
  canvas.width = size;
  canvas.height = size;
  const ctx = canvas.getContext("2d");
  if (!ctx) return "";
  const random = seeded(20260930);
  for (let index = 0; index < 26; index += 1) {
    ctx.globalAlpha = 0.35 + random() * 0.65;
    ctx.fillStyle = RAINBOW[Math.floor(random() * (RAINBOW.length - 1))];
    const x = random() * size;
    const y = random() * size;
    const radius = 1.5 + random() * 5;
    sparkle(ctx, x, y, radius);
    ctx.fillStyle = "#ffffff";
    sparkle(ctx, x, y, radius * 0.45);
  }
  sparkleTile = canvas.toDataURL("image/png");
  return sparkleTile;
}
