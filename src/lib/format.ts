/** 把「多少秒前」变成中文短描述，用于显示本地缓存的年龄。 */
export function formatAge(seconds: number | null | undefined): string {
  if (seconds == null) return "刚刚";
  if (seconds < 60) return `${seconds} 秒`;
  return `${Math.floor(seconds / 60)} 分钟`;
}

/** 1234567 → 1,234,567（经验值位数多，不分段根本读不出量级）。 */
export function formatNumber(value: number): string {
  return value.toLocaleString("en-US");
}

/** 秒 → 「1 小时 12 分」/「48 秒」。 */
export function formatDuration(secs: number): string {
  const total = Math.max(0, Math.round(secs));
  if (total < 60) return `${total} 秒`;
  const minutes = Math.floor(total / 60);
  if (minutes < 60) return `${minutes} 分`;
  const hours = Math.floor(minutes / 60);
  return `${hours} 小时 ${minutes % 60} 分`;
}

/**
 * 秒 → `mm:ss`（超过一小时变 `h:mm:ss`）。
 *
 * 「记了多久」这件事，秒是必需的信息：`1 分` 在 61 秒和 119 秒是同一个显示，
 * 而盯着它看的人正在判断「刚才那一下到底记上没有」。秒在跳，就说明在记。
 */
export function formatStopwatch(secs: number): string {
  const total = Math.max(0, Math.floor(secs));
  const seconds = total % 60;
  const minutes = Math.floor(total / 60) % 60;
  const hours = Math.floor(total / 3600);
  const pad = (value: number) => String(value).padStart(2, "0");
  return hours > 0 ? `${hours}:${pad(minutes)}:${pad(seconds)}` : `${pad(minutes)}:${pad(seconds)}`;
}

/** 时刻（秒级时间戳）→ 「20:31」。 */
export function formatClock(unix: number): string {
  return new Date(unix * 1000).toLocaleTimeString("zh-CN", {
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  });
}

/** 日期（秒级时间戳）→ 「09-24 20:31」。 */
export function formatShortDate(unix: number): string {
  const at = new Date(unix * 1000);
  return `${String(at.getMonth() + 1).padStart(2, "0")}-${String(at.getDate()).padStart(2, "0")} ${formatClock(unix)}`;
}

/** 秒 → 「4 时 41 分」，一眼能读完（精简版，给悬浮窗 / 总览用）。 */
export function formatEtaCompact(secs: number): string {
  if (secs < 60) return "不到 1 分";
  const minutes = Math.round(secs / 60);
  if (minutes < 60) return `${minutes} 分`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} 时 ${minutes % 60} 分`;
  return `${Math.floor(hours / 24)} 天 ${hours % 24} 时`;
}

/**
 * 大数收短：≥1 亿报到「亿」，≥1 万报到「万」，其余原样。
 * 数字和单位分开返回，单位可以缩小显示。
 */
export function splitBig(value: number | null | undefined): { n: string; unit: string } {
  if (value == null || !Number.isFinite(value)) return { n: "—", unit: "" };
  const abs = Math.abs(value);
  if (abs >= 1e8) return { n: (value / 1e8).toFixed(2), unit: "亿" };
  if (abs >= 1e4) return { n: formatNumber(Math.round(value / 1e4)), unit: "万" };
  return { n: formatNumber(Math.round(value)), unit: "" };
}

/** `splitBig` 的一句话版：「2,603 万」/「+1,835 万」。 */
export function joinBig(value: number | null | undefined, sign = false): string {
  const { n, unit } = splitBig(value);
  if (n === "—") return n;
  return `${sign && (value ?? 0) >= 0 ? "+" : ""}${n}${unit ? ` ${unit}` : ""}`;
}
