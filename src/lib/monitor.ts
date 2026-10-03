import type { ServerMonitorStatus } from "../types";
import { formatAge } from "./format";

/**
 * 「小册子开服监控」状态的统一说法。
 *
 * 这个功能的全部依据是站点自己的判断（tools/server-monitor），所以界面上的措辞
 * 必须把**来源**写清楚：是「小册子说的」，不是「我们探到的」。用户对照小册子网页
 * 时能一眼看出两边是不是一回事，出问题时也知道该找谁。
 *
 * 另一个必须说清的点：**抓不到数据 ≠ 服务器挂了**。站点抖一下、本机断网、
 * 被风控挡一下，都会让这里拿不到结论；把那种情况显示成「服务器异常」会让用户
 * 白白紧张一场。所以「取不到数据」独立成一档，并把原因照实写出来。
 */

/** Rust 事件还没到时的初始值：什么都别断言。 */
export const EMPTY_MONITOR_STATUS: ServerMonitorStatus = {
  enabled: true,
  interval_sec: 30,
  status: "",
  status_label: "还没拿到数据",
  open: false,
  changed_at: "",
  checked_at: "",
  checked_at_epoch: 0,
  checked_ago_sec: null,
  // 还没有任何采样：按「不新鲜」算，直到第一次成功采到东西
  snapshot_stale: true,
  official_label: "暂无官方消息",
  official_detail: "",
  login_label: "登录入口状态未知",
  login_detail: "",
  history: [],
  error: null,
  source: "小册子 · 开服监控",
};

export type MonitorTone = "open" | "maintenance" | "pending" | "error" | "unknown";

export interface MonitorSummary {
  tone: MonitorTone;
  /** 顶栏胶囊里的短文案 */
  text: string;
  /** 鼠标悬停时的完整解释 */
  tooltip: string;
}

export function summarizeMonitor(status: ServerMonitorStatus): MonitorSummary {
  const target = `数据来自小册子「开服监控」（${status.source}）`;

  // 一次都没成功抓过：只有失败原因可讲
  if (!status.checked_at && status.error) {
    return {
      tone: "error",
      text: "开服监控取不到数据",
      tooltip: `${target}\n这次没拿到站点数据：${status.error}\n\n只是没问到，不代表服务器状态异常。`,
    };
  }

  const when = status.checked_at
    ? `检测于 ${status.checked_at}（${formatAge(status.checked_ago_sec)}前）`
    : "还没有采样结果";

  const detail = [
    target,
    `当前状态：${status.status_label}（站点状态码 ${status.status || "未知"}）`,
    when,
    `游戏连接：${status.login_label}${status.login_detail ? ` · ${status.login_detail}` : ""}`,
    `官方消息：${status.official_label}${
      status.official_detail ? ` · ${status.official_detail}` : ""
    }`,
    status.changed_at ? `状态从 ${status.changed_at} 起` : "",
    status.error ? `⚠️ 最近一次抓取失败：${status.error}` : "",
  ]
    .filter(Boolean)
    .join("\n");

  // 监控被关掉时，不能继续拿停用前那句结论渲染：以前它一直绿着，
  // 用户不知道背后已经一小时没采样了（「把没数据显示成数据很新」的另一种写法）。
  if (!status.enabled) {
    return {
      tone: "unknown",
      text: "开服监控已停用",
      tooltip: `${detail}\n\n监控已在设置里关掉，这一句是停用前最后一次拿到的结论，之后不会再更新。`,
    };
  }

  // 开着但很久没采到新的（断网 / 站点一直报错）：结论照实保留，
  // 但必须让用户看出这句话已经旧了 —— 不能拿一份一小时前的结论当实时状态渲染。
  if (status.snapshot_stale) {
    const age = status.checked_at
      ? `已经${formatAge(status.checked_ago_sec)}没更新了`
      : "从没拿到过站点数据";
    return {
      tone: "unknown",
      text: "小册子：数据已陈旧",
      tooltip: `${detail}\n\n这是最后一次成功拿到的结论，${age}。抓不到数据 ≠ 服务器异常。`,
    };
  }

  if (status.open) {
    return { tone: "open", text: "小册子：服务器正常", tooltip: `${detail}\n\n小册子已确认可以登录，开服提醒在这一刻响过。` };
  }

  if (status.status === "maintenance") {
    return { tone: "maintenance", text: "小册子：维护中", tooltip: detail };
  }

  if (status.status === "confirming" || status.status === "unavailable") {
    return { tone: "pending", text: "小册子：状态确认中", tooltip: detail };
  }

  // 空状态 / 还没拿到有效结论
  if (!status.checked_at) {
    return {
      tone: "unknown",
      text: "开服监控待采样",
      tooltip: `${target}\n${status.enabled ? "正在等第一次采样结果…" : "监控已关闭（在「开服提醒」页可开启）"}`,
    };
  }

  return { tone: "unknown", text: `小册子：${status.status_label}`, tooltip: detail };
}

/** 状态变化列表里那条「是不是开服了」，用来给历史行上色。 */
export function historyTone(point: { open: boolean; status: string }): MonitorTone {
  if (point.open) return "open";
  if (point.status === "maintenance") return "maintenance";
  return "pending";
}
