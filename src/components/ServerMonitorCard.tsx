import React, { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { COOLDOWN, guard } from "../lib/throttle";
import { listen } from "@tauri-apps/api/event";
import {
  Radio,
  RefreshCw,
  BellRing,
  ExternalLink,
  History,
  Inbox,
} from "lucide-react";
import type { ServerMonitorStatus } from "../types";
import { EMPTY_MONITOR_STATUS, historyTone, summarizeMonitor } from "../lib/monitor";
import { formatAge } from "../lib/format";
import { InlineNotice, type Notice } from "./Notice";
import { MXDC_SERVER_MONITOR } from "../lib/links";

interface Props {
  enabled: boolean;
  intervalSec: number;
  onEnabledChange: (value: boolean) => void;
  onIntervalChange: (value: number) => void;
}

/** 轮询间隔的可选档位。站点自己每 20 秒采集一次，问得比它更勤没有意义。 */
const INTERVAL_CHOICES = [20, 30, 60, 120];

/**
 * 小册子「开服监控」的状态卡片。
 *
 * 为什么接站点的数据而不是只用我们自己的 TCP 握手：
 *
 * * 用户真正要的语义是「官方开服了没有」，而小册子已经在为整个社区每分钟探一次
 *   登录入口，比我们自己盯一个可能轮换掉的网关端口更接近标准答案；
 * * 自填端口的探测地址一旦轮换就会**默默地永远连不通** —— 对告警工具来说，
 *   「静静地变错」是最糟的失败模式，而这条通道不需要用户填任何地址。
 *
 * 现在只有这一条来源 —— 自填端口的 TCP 握手探测已经删掉了：地址跟着机房
 * 轮换，那种探测会**静静地失效**，对告警工具来说是最糟的失败模式。
 */
export const ServerMonitorCard: React.FC<Props> = ({
  enabled,
  intervalSec,
  onEnabledChange,
  onIntervalChange,
}) => {
  const [status, setStatus] = useState<ServerMonitorStatus>(EMPTY_MONITOR_STATUS);
  const [refreshing, setRefreshing] = useState(false);
  const [alertTesting, setAlertTesting] = useState(false);
  /** 失败信息留在页面上（原生 alert 关掉就没了，用户截个图也说不清） */
  const [notice, setNotice] = useState<Notice | null>(null);

  useEffect(() => {
    invoke<ServerMonitorStatus>("get_server_monitor_status")
      .then(setStatus)
      .catch(() => {});

    const unlisten = listen<ServerMonitorStatus>("server-monitor-update", (event) => {
      setStatus(event.payload);
    });

    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  const summary = summarizeMonitor(status);

  const handleRefresh = async () => {
    setRefreshing(true);
    try {
      // 刷新监控要打站点
    if (!guard("refresh_server_monitor", "刷新开服监控", COOLDOWN.query)) return;
    const fresh = await invoke<ServerMonitorStatus>("refresh_server_monitor");
      setStatus(fresh);
    } catch (err) {
      setNotice({ kind: "error", message: `刷新开服监控失败：${err}` });
    } finally {
      setRefreshing(false);
    }
  };

  const handleTestAlert = async () => {
    setAlertTesting(true);
    try {
      // 试一次提醒：这是本地动作，但连点会连着响好几次铃
    if (!guard("test_open_alert", "测试提醒", COOLDOWN.mutate)) return;
    await invoke("test_open_alert");
    } catch (err) {
      setNotice({ kind: "error", message: `测试提醒失败：${err}` });
    } finally {
      setAlertTesting(false);
    }
  };

  const dotColor =
    summary.tone === "open"
      ? "bg-emerald-400"
      : summary.tone === "maintenance"
        ? "bg-amber-400"
        : summary.tone === "error"
          ? "bg-red-400"
          : summary.tone === "pending"
            ? "bg-sky-400"
            : "bg-slate-500";

  return (
    <div className="p-5 rounded-2xl bg-slate-900/70 border border-white/10 space-y-4 md:col-span-2">
      <div className="flex items-center justify-between gap-3">
        <div className="flex items-center gap-2 text-sm font-bold text-slate-200">
          <Radio className="w-4 h-4 text-emerald-400" />
          小册子开服监控（站点数据源）
          <span className="text-[10px] px-1.5 py-0.5 rounded bg-emerald-500/10 text-emerald-300 border border-emerald-500/25 font-normal">
            推荐开启
          </span>
        </div>

        <label className="flex items-center gap-2 cursor-pointer">
          <span className="text-[11px] text-slate-400">
            {enabled ? `每 ${intervalSec} 秒检查` : "已关闭"}
          </span>
          <div className="relative">
            <input
              type="checkbox"
              checked={enabled}
              onChange={(e) => onEnabledChange(e.target.checked)}
              className="sr-only peer"
            />
            <div className="w-9 h-5 bg-slate-700 peer-focus:outline-none rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-4 after:w-4 after:transition-all peer-checked:bg-amber-500"></div>
          </div>
        </label>
      </div>

      <p className="text-xs text-slate-400 leading-relaxed">
        直接订阅小册子的开服监控，不需要你填任何地址；真的恢复正常时才提醒（「正在确认」不算）。
      </p>

      {/* 当前状态 */}
      <div className="p-3 rounded-lg bg-slate-950/50 border border-white/10 space-y-2">
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs">
          <span className="flex items-center gap-1.5 font-semibold text-slate-200">
            <span className={`w-2 h-2 rounded-full ${dotColor}`} />
            {summary.text.replace(/^小册子：/, "")}
          </span>
          <span className="text-slate-500">
            {status.checked_at
              ? `检测于 ${status.checked_at}（${formatAge(status.checked_ago_sec)}前）`
              : "还没有采样结果"}
          </span>
          {status.changed_at && (
            <span className="text-slate-500">状态从 {status.changed_at} 起</span>
          )}
        </div>

        <div className="grid grid-cols-1 sm:grid-cols-3 gap-2 text-[11px]">
          <div className="p-2 rounded bg-slate-900/60 border border-white/5">
            <div className="text-slate-500 mb-1">01 / 游戏连接</div>
            <div className="text-slate-200">{status.login_label}</div>
            {status.login_detail && (
              <div className="text-slate-500 mt-0.5">{status.login_detail}</div>
            )}
          </div>
          <div className="p-2 rounded bg-slate-900/60 border border-white/5">
            <div className="text-slate-500 mb-1">02 / 官方消息</div>
            <div className="text-slate-200">{status.official_label}</div>
            {status.official_detail && (
              <div className="text-slate-500 mt-0.5">{status.official_detail}</div>
            )}
          </div>
          <div className="p-2 rounded bg-slate-900/60 border border-white/5">
            <div className="text-slate-500 mb-1">03 / 开服提醒</div>
            <div className="text-slate-200">
              {enabled ? "恢复后自动提醒" : "已关闭"}
            </div>
            <div className="text-slate-500 mt-0.5">同一次开服只提醒一次</div>
          </div>
        </div>

        {status.error && (
          <p className="text-[11px] text-amber-200/80 leading-relaxed">
            ⚠️ {status.error}
            <span className="text-slate-500"> —— 只是没问到站点数据，不代表服务器异常。</span>
          </p>
        )}
      </div>

      <div className="flex flex-wrap items-center gap-2">
        <button
          type="button"
          onClick={handleRefresh}
          disabled={refreshing}
          className="px-3 py-1.5 rounded-lg bg-slate-800 hover:bg-slate-700 text-slate-200 border border-white/15 text-xs font-semibold flex items-center gap-1.5 cursor-pointer disabled:opacity-50"
        >
          <RefreshCw className={`w-3.5 h-3.5 text-sky-400 ${refreshing ? "animate-spin" : ""}`} />
          {refreshing ? "正在拉取…" : "立即刷新"}
        </button>

        <button
          type="button"
          onClick={handleTestAlert}
          disabled={alertTesting}
          className="px-3 py-1.5 rounded-lg bg-slate-800 hover:bg-slate-700 text-slate-200 border border-white/15 text-xs font-semibold flex items-center gap-1.5 cursor-pointer disabled:opacity-50"
        >
          <BellRing className="w-3.5 h-3.5 text-amber-400" />
          {alertTesting ? "提醒测试中…" : "测试开服提醒（响铃 + 右下角弹窗）"}
        </button>

        <button
          type="button"
          onClick={() =>
            invoke("open_external_url", { url: MXDC_SERVER_MONITOR })
          }
          className="px-3 py-1.5 rounded-lg bg-slate-800/60 hover:bg-slate-700 text-slate-300 border border-white/10 text-xs font-semibold flex items-center gap-1.5 cursor-pointer"
        >
          <ExternalLink className="w-3.5 h-3.5 text-amber-400" />
          打开小册子监控页对照
        </button>

        <span className="text-[11px] text-slate-500">
          数据源：{status.source}
        </span>
      </div>

      {/* 轮询间隔 */}
      {enabled && (
        <div className="flex flex-wrap items-center gap-2 text-[11px] text-slate-400">
          <span>拉取间隔</span>
          {INTERVAL_CHOICES.map((choice) => (
            <button
              key={choice}
              type="button"
              onClick={() => onIntervalChange(choice)}
              className={`px-2 py-0.5 rounded border cursor-pointer tabular-nums ${
                intervalSec === choice
                  ? "bg-emerald-500/20 border-emerald-500/40 text-emerald-300"
                  : "bg-slate-800/60 border-white/10 text-slate-400 hover:text-slate-200"
              }`}
            >
              {choice}s
            </button>
          ))}
          <span className="text-slate-500">
            （小册子自己每 20 秒采集一次，问得更勤没有意义）
          </span>
        </div>
      )}

      {/* 状态变化历史：站点页面上那个列表，这里是它最近的变化 */}
      <div className="space-y-1.5">
        <div className="flex items-center gap-1.5 text-xs font-semibold text-slate-300">
          <History className="w-3.5 h-3.5 text-slate-400" />
          状态变化（最近 {Math.min(status.history.length, 6)} 条）
        </div>

        {status.history.length === 0 ? (
          <div className="flex items-center gap-2 text-[11px] text-slate-500">
            <Inbox className="w-3.5 h-3.5" />
            还没有拿到站点的状态变化记录
          </div>
        ) : (
          <ul className="divide-y divide-white/5 rounded-lg border border-white/10 overflow-hidden">
            {status.history.slice(0, 6).map((point) => (
              <li
                key={`${point.at}-${point.status}`}
                title={`${point.login_label} · ${point.official_label}`}
                className="flex items-center gap-2 px-3 py-1.5 text-[11px] bg-slate-950/40"
              >
                <span
                  className={`w-1.5 h-1.5 rounded-full shrink-0 ${
                    historyTone(point) === "open"
                      ? "bg-emerald-400"
                      : historyTone(point) === "maintenance"
                        ? "bg-amber-400"
                        : "bg-sky-400"
                  }`}
                />
                <span className="text-slate-500 tabular-nums shrink-0">{point.at}</span>
                <span
                  className={
                    point.open ? "text-emerald-300" : "text-slate-300"
                  }
                >
                  {point.label}
                </span>
              </li>
            ))}
          </ul>
        )}
      </div>

      <InlineNotice notice={notice} onClose={() => setNotice(null)} />
    </div>
  );
};
