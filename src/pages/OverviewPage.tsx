import React, { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { BellRing, Cpu, Monitor, Pause, Play, RefreshCw, ShieldAlert, Square, Wand2 } from "lucide-react";
import type { AppSettings, ServerMonitorStatus } from "../types";
import { Panel, Switch, Kbd } from "../components/ui/kit";
import { usePresetServer } from "../lib/presetServer";
import { useExpSession } from "../lib/useExpSession";
import { useHotkey } from "../lib/hotkeys";
import { useTween } from "../lib/useTween";
import { useMotion } from "../lib/motion";
import { useMesoReport } from "../lib/mesoStore";
import { loadQueryHistory } from "../lib/queryHistory";
import { joinBig, splitBig, formatEtaCompact, formatNumber, formatStopwatch } from "../lib/format";
import { summarizeMonitor } from "../lib/monitor";
import { gameAway, gameAwayText } from "../lib/expRead";

/**
 * 总览：打开程序看到的第一页。
 *
 * 只**汇总**，不重复实现 —— 每块右上角的「›」都跳到对应的完整页面：
 * 金价 → 行情；练级 → 练级；最近查价 → 查询；提醒 → 提醒。
 *
 * 金价和行情页共用同一个仓库（`lib/mesoStore.ts`）：数据切页不丢，同一时刻只有一个请求在飞，
 * 「够新」就不再请求 —— 所以总览 ⇄ 行情 来回切不会互相拦截。
 */

export type PageId = "overview" | "meso" | "query" | "exp" | "alert" | "settings" | "about";

interface Props {
  onNavigate: (page: PageId, opts?: { keyword?: string }) => void;
  monitor: ServerMonitorStatus;
}

/** 折线：最近 N 个点，铺满容器宽度。 */
const Sparkline: React.FC<{ values: number[] }> = ({ values }) => {
  if (values.length < 2) return <div className="h-12" />;
  const min = Math.min(...values);
  const max = Math.max(...values);
  const span = max - min || 1;
  const w = 280;
  const h = 44;
  const pts = values.map((v, i) => [(i / (values.length - 1)) * w, h - 4 - ((v - min) / span) * (h - 10)]);
  const line = pts.map(([x, y], i) => `${i ? "L" : "M"}${x.toFixed(1)},${y.toFixed(1)}`).join(" ");
  return (
    <svg viewBox={`0 0 ${w} ${h}`} width="100%" height={h} preserveAspectRatio="none" className="block">
      <defs>
        <linearGradient id="ov-spark" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="var(--ui-ac)" stopOpacity="0.32" />
          <stop offset="1" stopColor="var(--ui-ac)" stopOpacity="0" />
        </linearGradient>
      </defs>
      <path d={`${line} L${w},${h} L0,${h}Z`} fill="url(#ov-spark)" />
      <path d={line} fill="none" stroke="var(--ui-ac)" strokeWidth="2" strokeLinejoin="round" vectorEffect="non-scaling-stroke" />
    </svg>
  );
};

const MoreLink: React.FC<{ onClick: () => void; children?: React.ReactNode }> = ({ onClick, children }) => (
  <button
    type="button"
    onClick={onClick}
    className="text-[11px] text-slate-400 hover:text-amber-300 cursor-pointer whitespace-nowrap"
  >
    {children ?? "全部"} ›
  </button>
);

export const OverviewPage: React.FC<Props> = ({ onNavigate, monitor }) => {
  const { serverId, servers } = usePresetServer();
  const hudKey = useHotkey("price_hud", "Alt+F");
  const floatKey = useHotkey("lite_float", "F10");
  const session = useExpSession();
  const { exp } = session;

  // ---- 金价 ----
  const { report: meso, loading, error: mesoError, updatedAt, refresh } = useMesoReport();
  const fetchedAt = updatedAt
    ? new Date(updatedAt).toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit", hour12: false })
    : "";
  const lastQuote = useRef<{ server: string; wan: number } | null>(null);
  const [trend, setTrend] = useState<{ dir: "up" | "down"; diff: number } | null>(null);

  const serverName = servers.find((server) => server.id === serverId)?.name ?? "当前区服";
  const rate = meso?.latest.find((item) => item.server_name === serverName);
  const rateServer = rate?.server_name;
  const rateValue = rate?.wan_rate;

  /** 较上次的涨跌：只在**同一个区服**的两次报价之间比，换服不算涨跌（那是用户点了另一个服）。 */
  useEffect(() => {
    if (!rateServer || !rateValue) return;
    const wan = Number(rateValue);
    if (!Number.isFinite(wan)) return;
    const previous = lastQuote.current;
    lastQuote.current = { server: rateServer, wan };
    if (!previous || previous.server !== rateServer || previous.wan === wan) {
      setTrend(null);
      return;
    }
    setTrend({ dir: wan > previous.wan ? "up" : "down", diff: Math.abs(wan - previous.wan) });
  }, [rateServer, rateValue]);

  const ranked = useMemo(
    () => [...(meso?.latest ?? [])].sort((a, b) => Number(b.wan_rate ?? 0) - Number(a.wan_rate ?? 0)),
    [meso],
  );
  const spark = useMemo(
    () =>
      (meso?.history.hour ?? [])
        .map((point) => point.values[serverName])
        .filter((v): v is number => typeof v === "number")
        .slice(-36),
    [meso, serverName],
  );

  // ---- 提醒开关（读真实设置，改完立刻写）----
  const [monitorOn, setMonitorOn] = useState<boolean | null>(null);
  const [watchOn, setWatchOn] = useState<boolean | null>(null);
  const [memoryAlertOn, setMemoryAlertOn] = useState<boolean | null>(null);
  // ---- 偏好开关：和设置页是同一份（那边有完整说明，这里是顺手能拨的入口）----
  const [trayOn, setTrayOn] = useState<boolean | null>(null);
  const [motion, setMotion] = useMotion();
  useEffect(() => {
    invoke<AppSettings>("get_settings")
      .then((s) => {
        setMonitorOn(s.monitor_enabled);
        setWatchOn(s.blacklist_watch);
        setMemoryAlertOn(s.memory_alert_enabled);
        setTrayOn(s.close_to_tray);
      })
      .catch(() => {});
  }, []);
  const changeTray = (value: boolean) => {
    setTrayOn(value);
    invoke("patch_settings", { patch: { close_to_tray: value } }).catch(() => setTrayOn(!value));
  };
  const changeMonitor = (value: boolean) => {
    setMonitorOn(value);
    invoke("patch_settings", { patch: { monitor_enabled: value } }).catch(() => setMonitorOn(!value));
  };
  const changeWatch = (value: boolean) => {
    setWatchOn(value);
    invoke("set_blacklist_watch", { enabled: value }).catch(() => setWatchOn(!value));
  };
  const changeMemoryAlert = (value: boolean) => {
    setMemoryAlertOn(value);
    invoke("set_memory_alert", { enabled: value }).catch(() => setMemoryAlertOn(!value));
  };

  // ---- 最近查价 ----
  const recent = useMemo(() => loadQueryHistory().slice(0, 6), []);
  const monitorSummary = summarizeMonitor(monitor);

  // ---- 练级 ----
  const phase = exp?.phase ?? "idle";
  const running = phase === "running";
  const active = running || phase === "paused";
  const unreadable = exp != null && exp.read_state !== "ok";
  const blind = !!exp?.stopped || unreadable;
  const s = exp?.session ?? null;
  const level = s?.current_level ?? exp?.level ?? null;
  const percent = s?.current_percent ?? exp?.percent ?? null;
  // 大数字滚动：首次从 0 滚到实际值，之后每次变化滚过去
  const perHourTween = useTween(blind ? null : (exp?.per_hour ?? null), { fromZero: true });
  const perHour = splitBig(perHourTween);
  const rateTween = useTween(rate ? Number(rate.wan_rate) : null, { fromZero: true, duration: 700 });
  // 没开始统计就没有速率可言：和旁边两格一样留「—」，「测量中」只在统计开始后才成立
  const eta = blind || !active
    ? "—"
    : s?.eta_secs != null
      ? formatEtaCompact(s.eta_secs)
      : exp?.per_hour === 0
        ? "未涨经验"
        : "测量中";

  return (
    <div className="anim-stagger grid grid-cols-12 gap-4">
      {/* 金价 */}
      <Panel
        className="col-span-12 lg:col-span-5"
        title={
          <>
            金价<span className="font-normal text-slate-400">· {serverName}</span>
          </>
        }
        meta={
          <>
            <span className="font-mono">{fetchedAt}</span>
            <button
              type="button"
              title="刷新金价"
              onClick={refresh}
              disabled={loading}
              className="text-slate-400 hover:text-amber-300 disabled:opacity-40 cursor-pointer"
            >
              <RefreshCw className={`w-3.5 h-3.5 ${loading ? "animate-spin" : ""}`} />
            </button>
            <MoreLink onClick={() => onNavigate("meso")}>走势</MoreLink>
          </>
        }
      >
        {rate ? (
          <div className="flex items-baseline gap-2">
            <span className="text-[44px] font-extrabold leading-none text-amber-400 tabular-nums">
              {rateTween != null && Number.isFinite(rateTween) ? rateTween.toFixed(2) : rate.wan_rate}
            </span>
            <span className="text-xs text-slate-400">万 / 元</span>
            {trend && (
              <span
                className={`ml-auto text-xs font-bold tabular-nums ${trend.dir === "up" ? "text-emerald-400" : "text-red-400"}`}
              >
                {trend.dir === "up" ? "▲" : "▼"} {trend.diff.toFixed(2)}
              </span>
            )}
          </div>
        ) : (
          <div className="text-sm text-slate-500 py-3">
            {loading ? "读取中…" : mesoError || "暂无报价"}
          </div>
        )}
        <div className="my-2">
          <Sparkline values={spark} />
        </div>
        <div className="space-y-0.5">
          {ranked.map((item) => {
            const active = item.server_name === serverName;
            return (
              <div
                key={item.server_name}
                className={`w-full flex items-center justify-between rounded-lg px-2.5 py-1 text-xs ${
                  active ? "bg-amber-500/15 text-slate-100 font-bold" : "text-slate-400"
                }`}
              >
                <span>
                  {item.server_name}
                  {active && <span className="ml-1.5 text-[10px] text-amber-400 font-normal">预设</span>}
                </span>
                <span className="tabular-nums">{item.wan_rate}</span>
              </div>
            );
          })}
        </div>
      </Panel>

      {/* 练级 */}
      <Panel
        className="col-span-12 lg:col-span-7 flex flex-col"
        title={
          <>
            练级
            <span
              className={`inline-flex items-center gap-1.5 px-2.5 py-0.5 rounded-full text-[11px] font-normal ${
                running ? "bg-emerald-500/15 text-emerald-300" : "bg-slate-800 text-slate-400"
              }`}
            >
              {running && <span className="w-1.5 h-1.5 rounded-full bg-emerald-400 animate-pulse" />}
              {active ? (running ? "记录中" : "已暂停") : "未开始"}
              {active && <span className="tabular-nums">{formatStopwatch(s?.active_secs ?? 0)}</span>}
            </span>
          </>
        }
        meta={
          <>
            <span className="truncate">{exp?.map_name ?? ""}</span>
            <MoreLink onClick={() => onNavigate("exp")}>详情</MoreLink>
          </>
        }
      >
        <div className="flex items-end gap-6">
          <div className="flex-1 min-w-0">
            <div className="text-[11px] text-slate-500">经验 / 小时</div>
            <div className={`text-[44px] font-extrabold leading-tight tabular-nums ${blind || perHour.n === "—" ? "text-slate-600" : "text-amber-400"}`}>
              {perHour.n}
              {perHour.unit && <span className="text-base font-semibold text-slate-400 ml-1">{perHour.unit}</span>}
            </div>
          </div>
          <div className="w-52 shrink-0">
            <div className="flex items-baseline justify-between mb-1.5">
              <span className="text-xl font-extrabold tabular-nums">{level != null ? `Lv.${level}` : "Lv.—"}</span>
              <span className="text-xs text-slate-400 tabular-nums">{percent != null ? `${percent.toFixed(2)}%` : ""}</span>
            </div>
            <div className="h-2 rounded-full bg-slate-800 overflow-hidden">
              <div
                className="h-full rounded-full bg-gradient-to-r from-orange-600 to-amber-500 transition-all duration-700"
                style={{ width: `${Math.min(100, Math.max(0, percent ?? 0))}%` }}
              />
            </div>
          </div>
        </div>

        <div className="grid grid-cols-3 gap-3 mt-4 text-sm">
          <div>
            <div className="text-[11px] text-slate-500">净经验</div>
            <div className="font-bold text-emerald-400 tabular-nums">{s ? joinBig(s.gained_exp, true) : "—"}</div>
          </div>
          <div>
            <div className="text-[11px] text-slate-500">每分钟</div>
            <div className="font-bold tabular-nums">
              {blind || exp?.per_minute == null ? "—" : formatNumber(Math.round(exp.per_minute))}
            </div>
          </div>
          <div>
            <div className="text-[11px] text-slate-500">距升级</div>
            <div className="font-bold tabular-nums">{eta}</div>
          </div>
        </div>

        <div className="flex items-center gap-2 mt-auto pt-4">
          <button
            type="button"
            disabled={session.busy}
            onClick={session.toggle}
            className={`inline-flex items-center gap-1.5 px-3.5 py-1.5 rounded-lg text-xs font-bold cursor-pointer disabled:opacity-40 ${
              running ? "bg-slate-800 text-slate-200 hover:bg-slate-700" : "bg-amber-500 text-slate-950 hover:bg-amber-400"
            }`}
          >
            {running ? <Pause className="w-3.5 h-3.5" /> : <Play className="w-3.5 h-3.5" />}
            {running ? "暂停" : phase === "paused" ? "继续" : "开始统计"}
          </button>
          <button
            type="button"
            disabled={session.busy || phase === "idle" || exp?.pending != null}
            onClick={session.end}
            className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs bg-slate-800 text-slate-300 hover:bg-slate-700 disabled:opacity-40 cursor-pointer"
          >
            <Square className="w-3.5 h-3.5" />
            结束
          </button>
          <span className="text-[11px] text-slate-500 ml-2 truncate">
            {session.actionError ||
              (gameAway(exp?.read_state)
                ? gameAwayText(exp?.read_state)
                : unreadable
                  ? "未识别到经验条 —— 去「练级」页校准"
                  : exp?.pending ? "有一段待确认，去「练级」页处理" : "")}
          </span>
        </div>
      </Panel>

      {/* 最近查价 */}
      <Panel
        className="col-span-12 md:col-span-6 lg:col-span-3"
        title="最近查价"
        meta={<MoreLink onClick={() => onNavigate("query")}>去查询</MoreLink>}
      >
        {recent.length === 0 ? (
          <div className="text-xs text-slate-500 leading-relaxed py-2">
            还没有查过价。按 <Kbd>{hudKey}</Kbd> 呼出查价窗，或去「查询」页。
          </div>
        ) : (
          <div className="flex flex-wrap gap-2">
            {recent.map((term) => (
              <button
                key={term}
                type="button"
                onClick={() => onNavigate("query", { keyword: term })}
                title="在查询页重查这个词"
                className="lift px-3 py-1.5 rounded-lg bg-slate-800/80 border border-white/5 text-xs text-slate-200 hover:border-amber-500/40 hover:text-amber-300 cursor-pointer"
              >
                {term}
              </button>
            ))}
          </div>
        )}
      </Panel>

      {/* 提醒 */}
      <Panel
        className="col-span-12 md:col-span-6 lg:col-span-3"
        title="提醒"
        meta={<MoreLink onClick={() => onNavigate("alert")}>设置</MoreLink>}
      >
        <div className="space-y-3 text-sm">
          <div className="flex items-center justify-between gap-3">
            <span className="flex items-center gap-2">
              <BellRing className="w-4 h-4 text-amber-400" />
              开服提醒
            </span>
            <Switch checked={!!monitorOn} disabled={monitorOn === null} onChange={changeMonitor} label="开服提醒" />
          </div>
          <div className="flex items-center justify-between gap-3">
            <span className="flex items-center gap-2">
              <ShieldAlert className="w-4 h-4 text-amber-400" />
              黑名单命中
            </span>
            <Switch checked={!!watchOn} disabled={watchOn === null} onChange={changeWatch} label="黑名单命中提醒" />
          </div>
          <div className="flex items-center justify-between gap-3">
            <span className="flex items-center gap-2">
              <Cpu className="w-4 h-4 text-amber-400" />
              内存到线
            </span>
            <Switch
              checked={!!memoryAlertOn}
              disabled={memoryAlertOn === null}
              onChange={changeMemoryAlert}
              label="内存到线提醒"
            />
          </div>
          <div className="text-[11px] text-slate-500 leading-relaxed" title={monitorSummary.tooltip}>
            官方开服：{monitorSummary.text}
          </div>
        </div>
      </Panel>

      {/* 偏好：两个最常拨的开关，完整说明在设置页 */}
      <Panel
        className="col-span-12 md:col-span-6 lg:col-span-3"
        title="偏好"
        meta={<MoreLink onClick={() => onNavigate("settings")}>设置</MoreLink>}
      >
        <div className="space-y-3 text-sm">
          <div className="flex items-center justify-between gap-3">
            <span className="flex items-center gap-2">
              <Monitor className="w-4 h-4 text-amber-400" />
              关窗收进托盘
            </span>
            <Switch checked={!!trayOn} disabled={trayOn === null} onChange={changeTray} label="关窗收进托盘" />
          </div>
          <div className="flex items-center justify-between gap-3">
            <span className="flex items-center gap-2">
              <Wand2 className="w-4 h-4 text-amber-400" />
              界面动效
            </span>
            <Switch checked={motion} onChange={setMotion} label="界面动效" />
          </div>
          <div className="text-[11px] text-slate-500 leading-relaxed">
            {trayOn === false
              ? "点 X 直接退出，后台提醒和快捷键一起停。"
              : "点 X 只收进托盘，提醒和快捷键继续跑。"}
          </div>
        </div>
      </Panel>

      {/* 悬浮窗入口 */}
      <Panel className="col-span-12 md:col-span-6 lg:col-span-3" title="悬浮窗">
        <div className="space-y-2">
          <button
            type="button"
            onClick={() => invoke("toggle_hud").catch(() => {})}
            className="w-full flex items-center justify-between px-3 py-2 rounded-xl bg-amber-500 text-slate-950 text-xs font-bold hover:bg-amber-400 cursor-pointer"
          >
            <span>查价窗</span>
            <Kbd className="!bg-black/15 !border-0 !text-slate-950">{hudKey}</Kbd>
          </button>
          <button
            type="button"
            onClick={() => invoke("toggle_lite_float").catch(() => {})}
            className="w-full flex items-center justify-between px-3 py-2 rounded-xl bg-slate-800 text-slate-200 text-xs font-bold hover:bg-slate-700 cursor-pointer"
          >
            <span>经验窗</span>
            <Kbd>{floatKey}</Kbd>
          </button>
          <p className="text-[11px] text-slate-500 leading-relaxed">在游戏里按快捷键呼出，Esc 收起；边缘可拖动缩放。</p>
        </div>
      </Panel>
    </div>
  );
};
