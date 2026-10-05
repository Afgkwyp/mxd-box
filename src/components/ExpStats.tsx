import React, { useCallback, useEffect, useMemo, useState } from "react";
import { Panel } from "./ui/kit";
import { useTween } from "../lib/useTween";
import { useTheme } from "../lib/theme";
import { READ_SHORT, gameAway } from "../lib/expRead";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  AlertTriangle,
  Frame,
  Keyboard,
  MapPin,
  Pause,
  Play,
  Share2,
  ShieldAlert,
  ShieldCheck,
  Square,
  Target,
  Trash2,
  TrendingUp,
} from "lucide-react";
import {
  CartesianGrid,
  Line,
  LineChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import { HotkeyInput } from "./HotkeyInput";
import { ExpGoalRow } from "./ExpGoalRow";
import { ExpCharacters } from "./ExpCharacters";
import { ConfirmDialog, InlineNotice, type Notice } from "./Notice";
import {
  formatClock,
  formatDuration,
  formatNumber,
  formatShortDate,
  formatStopwatch,
  splitBig,
} from "../lib/format";
import { ShareCardDialog } from "./ShareCardDialog";
import type { ExpCardData } from "../lib/shareCard";
import type {
  AppSettings,
  ExpCharacter,
  ExpCurvePoint,
  ExpHistoryPayload,
  ExpHistoryRow,
  ExpSessionQuality,
  ExpSessionStatus,
  ExpStatus,
  ExpTotals,
  HotkeyStatus,
} from "../types";

/**
 * 「经验统计」—— 从游戏画面上读经验，算出每分钟 / 每小时收益。
 *
 * 三条这个页面必须守住的规矩：
 *
 * 1. **读不到就说读不到**：`read_state` 不是 `ok` 时，速率区域显示原因而不是数字
 *    （拿上一个值凑合会让一条平坦的曲线看起来像「你在挂机」）；
 * 2. **没开始就不统计**：程序启动不会自己开始，也不会自己把数据写进历史
 *    （用户点「开始」才算一段，点「结束」才问要不要留档）；
 * 3. **数字要带上它测了多久**：不满一小时就用实测区间折算，并在旁边写「实测 12 分钟」；
 * 4. **要告诉用户这段数据能不能信**：读数质量（能读到画面的时间占比、被校验拦下的帧）
 *    单独一块，而且**不和「挂机多不多」混在一起说** —— 前者是数据可不可信，
 *    后者是玩法。混起来说会让「挂机 70%」看起来像程序出了问题。
 */
export const ExpStats: React.FC<{ onOpenCalibration: () => void }> = ({ onOpenCalibration }) => {
  const [status, setStatus] = useState<ExpStatus | null>(null);
  const [history, setHistory] = useState<ExpHistoryRow[]>([]);
  const [totals, setTotals] = useState<ExpTotals | null>(null);
  const [curve, setCurve] = useState<ExpCurvePoint[]>([]);
  const [notice, setNotice] = useState<Notice | null>(null);
  /** 正在忙（按钮转圈，避免连点两次「结束」） */
  const [busy, setBusy] = useState(false);


  /** 本功能自己的全局快捷键 */
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [hotkeyStatuses, setHotkeyStatuses] = useState<HotkeyStatus[]>([]);
  const [capturing, setCapturing] = useState(false);

  const [pendingClear, setPendingClear] = useState(false);
  /** 等用户确认要不要删的那一行历史（null = 没在问） */
  const [pendingDelete, setPendingDelete] = useState<ExpHistoryRow | null>(null);
  /**
   * 正在看哪一段的小结卡片（null = 没开）。`rowId` 是历史里那一行的 id：
   * 评稀有度时要把它自己从历史里剔掉，不然永远是「跟自己打平」。
   */
  const [sharing, setSharing] = useState<{ data: ExpCardData; rowId: number | null } | null>(
    null,
  );
  /**
   * 这一段在哪个图练的。**用户自己填** —— 程序不去认地图（地图名在小地图那一角，
   * 认错了比不认更糟）；填一次就会跟着这一行进历史、进小结卡片。
   */
  /**
   * 用户手填的地图名。
ull = 他没动过，这时用程序认出来的那个
   * （见下面 mapName）—— 在渲染时推导，而不是在 effect 里回填 state：
   * 后者会多渲染一轮，而且「认出来的」和「手填的」谁是准的会变得含糊。
   */
  const [manualMap, setMapName] = useState<string | null>(null);

  const refreshStatus = useCallback(() => {
    invoke<ExpStatus>("get_exp_status")
      .then(setStatus)
      .catch(() => {});
  }, []);

  /** 已经认过的地图名（库里存的，见 exp::map）：输入框旁边的快捷按钮用 */
  const [knownMaps, setKnownMaps] = useState<string[]>([]);

  /** 角色列表，和「历史只看谁的」（null = 全部角色） */
  const [characters, setCharacters] = useState<ExpCharacter[]>([]);
  const [viewCharacter, setViewCharacter] = useState<number | null>(null);
  const [pendingCharacter, setPendingCharacter] = useState<ExpCharacter | null>(null);

  const refreshCharacters = useCallback(() => {
    invoke<ExpCharacter[]>("list_exp_characters")
      .then(setCharacters)
      .catch(() => {});
  }, []);

  const refreshHistory = useCallback(() => {
    invoke<ExpHistoryPayload>("get_exp_history", { limit: 30, character: viewCharacter })
      .then((payload) => {
        setHistory(payload.rows);
        setTotals(payload.totals);
      })
      .catch(() => {});
    invoke<ExpCurvePoint[]>("get_exp_curve", { character: viewCharacter })
      .then(setCurve)
      .catch(() => {});
    invoke<string[]>("known_exp_maps")
      .then(setKnownMaps)
      .catch(() => {});
    refreshCharacters();
  }, [viewCharacter, refreshCharacters]);

  // 角色的等级和进度是后台边读边记的：登录的角色变了立刻刷一次，平时半分钟刷一次
  const currentCharacter = status?.character?.id ?? null;
  useEffect(() => {
    refreshCharacters();
    const timer = window.setInterval(refreshCharacters, 30_000);
    return () => window.clearInterval(timer);
  }, [currentCharacter, refreshCharacters]);

  useEffect(() => {
    refreshStatus();
    refreshHistory();
    invoke<AppSettings>("get_settings")
      .then(setSettings)
      .catch(() => {});
    invoke<HotkeyStatus[]>("get_hotkey_status")
      .then(setHotkeyStatuses)
      .catch(() => {});

    // 采样循环每秒 / 每两秒广播一次；事件到不了时还有下面那条定时兜底
    const unlisten = listen<ExpStatus>("exp-update", (event) => setStatus(event.payload));
    const timer = window.setInterval(refreshStatus, 2000);

    return () => {
      unlisten.then((off) => off());
      window.clearInterval(timer);
    };
  }, [refreshStatus, refreshHistory]);

  /**
   * 改键捕获期间必须先注销全局快捷键。
   *
   * 全局键是 OS 级的 `RegisterHotKey`：键被注册时按键会直接投给系统、webview
   * 收不到 keydown，而用户想改的往往正是**当前那个键**（按下去只会触发原功能）。
   * 用 effect 的 cleanup 保证一定会挂回去（捕获成功 / Esc / 点到别处都走这条路）。
   */
  useEffect(() => {
    if (!capturing) return;
    invoke("begin_hotkey_capture").catch(() => {});
    return () => {
      invoke<HotkeyStatus[]>("end_hotkey_capture")
        .then(setHotkeyStatuses)
        .catch(() => {});
    };
  }, [capturing]);

  const run = async (task: () => Promise<unknown>) => {
    setBusy(true);
    try {
      await task();
    } catch (err: unknown) {
      setNotice({ kind: "error", message: typeof err === "string" ? err : String(err) });
    } finally {
      setBusy(false);
      refreshStatus();
    }
  };

  /** 输入框里该显示什么：手填优先，没填过就用程序认出来的 */
  const mapName = manualMap ?? status?.map_name ?? '';

  /** 历史里出现过的地图，做成快捷按钮（每次手打地名很烦） */
  const recentMaps = useMemo(
    () =>
      Array.from(
        new Set([
          ...knownMaps,
          ...history.map((row) => row.map_name).filter((name) => name.length > 0),
        ]),
      ).slice(0, 6),
    [history, knownMaps],
  );

  /**
   * 小结卡片的数据。两个来源（进行中的这一段 / 历史里的一段）走同一个结构 ——
   * 卡片只有一套画法，不能让「刚打的那段」和「以前那段」长得不一样。
   */
  const cardData = (
    item: ExpSessionStatus,
    itemQuality: ExpSessionQuality,
    map: string,
  ): ExpCardData => ({
    mapName: map,
    startedAt: item.started_unix,
    endedAt: item.ended_unix,
    activeSecs: item.active_secs,
    gainedExp: item.gained_exp,
    perHour: item.per_hour,
    startLevel: item.start_level,
    startPercent: item.start_percent,
    endLevel: item.current_level,
    endPercent: item.current_percent,
    levelUps: item.level_ups,
    stages: item.stages.map((stage) => ({
      level: stage.level,
      secs: stage.secs,
      perHour: stage.per_hour,
      ongoing: stage.ongoing,
    })),
    qualityGrade: itemQuality.grade,
    qualityLabel: itemQuality.label,
    coverage: itemQuality.coverage,
    activeRatio: itemQuality.active_ratio,
  });

  /** 历史里的一段：那时还没记分段速率，所以分段那一行不画，其余照旧。 */
  const historyCardData = (row: ExpHistoryRow): ExpCardData => ({
    mapName: row.map_name,
    startedAt: row.started_unix,
    endedAt: row.ended_unix,
    activeSecs: row.active_secs,
    gainedExp: row.gained_exp,
    perHour: row.active_secs > 60 ? row.gained_exp / (row.active_secs / 3600) : null,
    startLevel: row.start_level,
    startPercent: row.start_percent,
    endLevel: row.end_level,
    endPercent: row.end_percent,
    levelUps:
      row.start_level != null && row.end_level != null
        ? Math.max(0, row.end_level - row.start_level)
        : 0,
    stages: [],
    qualityGrade: row.quality,
    qualityLabel: qualityLabel(row.quality),
    coverage: row.coverage,
    activeRatio: 1 - row.idle_ratio,
  });

  /** 打开分享弹窗：先看卡，复制还是存图在弹窗里选 */
  const shareCard = (data: ExpCardData, rowId: number | null = null) =>
    setSharing({ data, rowId });

  /**
   * 评稀有度用的「别的记录的时速」：太短的、数据不完整的不算数，正在看的那一行剔掉。
   * 只有最近 30 段（历史列表取的就是这么多）—— 卡片上的说法也是「最近 N 段」。
   */
  const pastRates = useMemo(
    () =>
      history
        .filter((row) => row.id !== sharing?.rowId && row.active_secs >= 300 && row.quality >= 1)
        .map((row) => row.gained_exp / (row.active_secs / 3600)),
    [history, sharing?.rowId],
  );
  /** 卡片编号：历史里的用它那一行的 id，还没存的这一段排在最后一行后面 */
  const shareSerial =
    sharing?.rowId ?? (history.length > 0 ? Math.max(...history.map((row) => row.id)) + 1 : 1);

  /** 「计不计入历史」的回答：地图名跟着这一行一起写进去 */
  const resolvePending = (save: boolean) =>
    run(async () => {
      const message = await invoke<string>("resolve_exp_session", { save, mapName });
      if (save) refreshHistory();
      setNotice({ kind: "success", message });
    });

  const handleHotkeyChange = useCallback(async (hotkey: string) => {
    setSettings((prev) => (prev ? { ...prev, exp_hotkey: hotkey } : prev));
    try {
      const statuses = await invoke<HotkeyStatus[]>("apply_hotkey", {
        action: "exp_toggle",
        hotkey,
      });
      if (statuses.length) setHotkeyStatuses(statuses);
    } catch (err: unknown) {
      setNotice({ kind: "error", message: `注册快捷键失败：${err}` });
    }
  }, []);

  const expHotkeyStatus = hotkeyStatuses.find((item) => item.action === "exp_toggle");
  const viewedCharacter = characters.find((item) => item.id === viewCharacter) ?? null;
  const phase = status?.phase ?? "idle";
  const reading = status?.read_state === "ok";
  const session = status?.session ?? null;
  const quality = status?.quality ?? null;
  const pending = status?.pending ?? null;
  // recharts 的 SVG 属性不接受 var()，图表颜色按主题在 JS 里取
  const [theme] = useTheme();
  const chart =
    theme === "light"
      ? { grid: "#dccfb3", axis: "#6b5b47", tipBg: "#fffaf0", tipBorder: "#dccfb3", line: "#d9770a" }
      : { grid: "#3b3229", axis: "#a89c8a", tipBg: "#1c1713", tipBorder: "#3b3229", line: "#f5a04a" };
  // 大数字滚动：读数变化时滚过去；读不到 / 已停止时保持 null，不拿 0 顶替
  const hourIdle = !reading || (status?.stopped ?? false) || status?.per_hour == null;
  const hourTween = useTween(hourIdle ? null : (status?.per_hour ?? null), { fromZero: true });
  const hourBig = splitBig(hourTween);

  return (
    <>
      {notice && (
        <div className="mb-4">
          <InlineNotice notice={notice} onClose={() => setNotice(null)} />
        </div>
      )}
    <div className="anim-stagger grid grid-cols-12 gap-4">
      {/* 实时读数 + 这一段的控制 */}
      <Panel
        className="col-span-12 lg:col-span-7 flex flex-col gap-4"
        title={
          <>
            练级
            <PhasePill phase={phase} secs={session?.active_secs ?? 0} />
          </>
        }
        meta={
          <span className="truncate max-w-[240px]" title={status?.window_title ?? ""}>
            {status?.window_title ?? "未找到游戏窗口"}
          </span>
        }
      >
        {/* 大数字：每小时经验 + 等级进度 */}
        <div className="flex items-end gap-6 flex-wrap">
          <div className="flex-1 min-w-[220px]">
            <div className="text-[11px] text-slate-500">每小时经验</div>
            <div
              className={`text-[52px] font-extrabold leading-tight tabular-nums ${
                hourIdle ? "text-slate-600" : "text-amber-400"
              }`}
            >
              {hourBig.n}
              {hourBig.unit && (
                <span className="text-lg font-semibold text-slate-400 ml-1.5">{hourBig.unit}</span>
              )}
            </div>
            <div className="text-[11px] text-slate-500 tabular-nums">
              {status?.per_hour != null && !hourIdle ? `${formatNumber(Math.round(status.per_hour))} · ` : ""}
              {rateCaption(status?.hour_span_secs ?? 0, "一小时", status?.stopped ?? false, reading, status?.idle_secs ?? 0)}
            </div>
          </div>
          <div className="w-60 shrink-0">
            <div className="flex items-baseline justify-between mb-1.5">
              <span className="text-2xl font-extrabold tabular-nums">
                {status?.level != null ? `Lv.${status.level}` : "Lv.—"}
              </span>
              <span className="text-xs text-slate-400 tabular-nums">
                {status?.percent != null ? `${status.percent.toFixed(2)}%` : ""}
              </span>
            </div>
            <div className="h-2.5 rounded-full bg-slate-800 overflow-hidden">
              <div
                className="h-full rounded-full bg-gradient-to-r from-orange-600 to-amber-500 transition-all duration-700"
                style={{ width: `${Math.min(100, Math.max(0, status?.percent ?? 0))}%` }}
              />
            </div>
            <div className="text-[11px] text-slate-500 mt-1 tabular-nums">
              {status?.exp != null ? `${formatNumber(status.exp)} 经验` : "—"}
            </div>
          </div>
        </div>

        <div className="grid grid-cols-2 sm:grid-cols-3 gap-3">
          <RateBox
            label="每分钟"
            value={status?.per_minute ?? null}
            spanSecs={status?.minute_span_secs ?? 0}
            spanLabel="一分钟"
            stopped={status?.stopped ?? false}
            readOk={reading}
            idleSecs={status?.idle_secs ?? 0}
          />
          <div className="min-w-0 rounded-xl bg-slate-800/60 border border-white/5 px-3 py-2.5 space-y-1">
            <div className="text-[11px] text-slate-500">累计经验</div>
            <div className="text-lg font-extrabold leading-tight tabular-nums truncate">
              {status?.cumulative != null ? formatNumber(status.cumulative) : "—"}
            </div>
            <div className="text-[11px] text-slate-500">从 1 级算起</div>
          </div>
          <div className="rounded-xl bg-slate-800/60 border border-white/5 px-3 py-2.5 space-y-1 col-span-2 sm:col-span-1">
            <div className="text-[11px] text-slate-500">状态</div>
            <div className="text-xl font-extrabold leading-tight">{status?.phase_label || "—"}</div>
            <div className="text-[11px] text-slate-500 truncate">{status?.map_name || "地图未识别"}</div>
          </div>
        </div>

        {/* 控制 */}
        <div className="flex items-center gap-2 flex-wrap">
          {phase !== "running" ? (
            <button
              type="button"
              disabled={busy}
              onClick={() =>
                run(() => invoke(phase === "paused" ? "resume_exp_session" : "start_exp_session"))
              }
              className="px-4 py-2 rounded-xl bg-amber-500 hover:bg-amber-400 disabled:opacity-50 text-sm font-bold text-slate-950 flex items-center gap-1.5 cursor-pointer"
            >
              <Play className="w-4 h-4" />
              {phase === "paused" ? "继续" : "开始统计"}
            </button>
          ) : (
            <button
              type="button"
              disabled={busy}
              onClick={() => run(() => invoke("pause_exp_session"))}
              className="px-4 py-2 rounded-xl bg-slate-800 hover:bg-slate-700 disabled:opacity-50 text-sm font-bold text-slate-100 flex items-center gap-1.5 cursor-pointer"
            >
              <Pause className="w-4 h-4" />
              暂停
            </button>
          )}

          {phase !== "idle" && (
            <button
              type="button"
              disabled={busy}
              onClick={() => run(() => invoke("end_exp_session"))}
              className="px-4 py-2 rounded-xl bg-slate-800 hover:bg-slate-700 disabled:opacity-50 border border-white/10 text-sm text-slate-200 flex items-center gap-1.5 cursor-pointer"
            >
              <Square className="w-4 h-4" />
              结束本段
            </button>
          )}

          {/* 手动校准：不只在读不到时才有 —— 想提前框准、换过分辨率后微调，都从这进 */}
          <button
            type="button"
            onClick={onOpenCalibration}
            title="自动识别或手动框选经验行的位置；校准按比例记住，换分辨率不用重框"
            className="ml-auto px-3.5 py-2 rounded-xl border border-amber-500/40 text-amber-300 hover:bg-amber-500/10 text-sm font-semibold flex items-center gap-1.5 cursor-pointer"
          >
            <Frame className="w-4 h-4" />
            手动校准
          </button>
        </div>

        {!reading && status && (
          <div className="flex items-start justify-between gap-3 text-xs text-amber-200/90 bg-amber-500/10 border border-amber-500/30 rounded-lg px-3 py-2">
            <div className="flex items-start gap-2 min-w-0">
              <AlertTriangle className="w-3.5 h-3.5 mt-0.5 shrink-0" />
              <span>
                {READ_SHORT[status.read_state] ?? "没识别到经验条"}
                {status.region_lost && !gameAway(status.read_state) && (
                  <span className="block mt-0.5 text-amber-300/80">
                    手动校准可能失效，点「校准位置」重新框一次。
                  </span>
                )}
              </span>
            </div>
            {/* 失败时的出口：手动框选。正常读数时整条横幅不露头，
                所以这个入口天然不打扰正常用户（项目惯例）。
                游戏没开 / 最小化时不摆：没有画面可框，校准解决不了。 */}
            {!gameAway(status.read_state) && (
              <div className="shrink-0 flex items-center gap-2">
                <button
                  type="button"
                  onClick={onOpenCalibration}
                  title="自动识别经验行位置，或抓一张当前画面手动拖框"
                  className="px-2 py-0.5 rounded bg-amber-500/20 hover:bg-amber-500/30 border border-amber-500/40 text-[11px] font-medium text-amber-200 flex items-center gap-1 cursor-pointer transition-colors"
                >
                  <Frame className="w-3 h-3" />
                  校准位置
                </button>
              </div>
            )}
          </div>
        )}

        {status?.notice && (
          <div className="text-xs text-sky-200 bg-sky-500/10 border border-sky-500/30 rounded-lg px-3 py-2">
            {status.notice}
          </div>
        )}

        {/* 恢复后效率：主速率被「看不到画面」的那段时间弄脏了，这个才是恢复后的真实水平 */}
        {status?.recovery_per_hour != null && (
          <div className="text-[11px] text-sky-200/90 bg-sky-500/10 border border-sky-500/20 rounded-lg px-3 py-1.5">
            恢复后效率（恢复 {formatDuration(status.recovery_span_secs)}起算）：{" "}
            <span className="tabular-nums font-bold">
              {formatNumber(Math.round(status.recovery_per_hour))}
            </span>{" "}
            / 小时
          </div>
        )}

      </Panel>

      {/* 右列：本段 + 快捷键 */}
      <div className="col-span-12 lg:col-span-5 space-y-4">
        <Panel title="本段" meta={session ? formatStopwatch(session.active_secs) : undefined}>
          <div className="space-y-3">
        {/* 练级目标：没在统计也显示还差多少（进游戏就在读），时间要等开始统计才有 */}
        <ExpGoalRow
          goal={status?.goal ?? null}
          level={status?.level ?? null}
          rateStale={!reading || (status?.stopped ?? false)}
          onStatus={setStatus}
        />
        {session && (
          <div className="text-xs text-slate-300 space-y-1.5">
            <div className="flex items-center gap-2 flex-wrap">
              <Target className="w-3.5 h-3.5 text-slate-500" />
              <span>
                本段 {formatStopwatch(session.active_secs)} · 获得{" "}
                <span className="tabular-nums text-emerald-300">
                  +{formatNumber(session.gained_exp)}
                </span>
              </span>
              <span className="text-slate-500">
                {formatLevelPercent(session.start_level, session.start_percent)} →{" "}
                {formatLevelPercent(session.current_level, session.current_percent)}
              </span>
              {session.level_ups > 0 && (
                <span className="text-amber-300">升级 {session.level_ups} 次</span>
              )}
            </div>
            {session.per_hour != null && (
              <div className="text-slate-500 tabular-nums">
                本段平均 {formatNumber(Math.round(session.per_hour))} / 小时（按有效时间算）
              </div>
            )}
            {/* 预计升级时间（枫记的「本机共需多久升级」） */}
            {session.remaining_exp != null && (
              <div className="text-slate-500 tabular-nums">
                距升级 {formatNumber(session.remaining_exp)} 经验
                {session.eta_secs != null &&
                  `（按当前时速约 ${formatEta(session.eta_secs)}）`}
              </div>
            )}
            {/* 阶段效率：每一级的真实时速（升级时分段，枫记的「阶段效率」） */}
            {session.stages.filter((stage) => stage.per_hour != null).length > 1 && (
              <div className="flex items-center gap-1.5 flex-wrap pt-0.5">
                <span className="text-slate-500">分段：</span>
                {session.stages
                  .filter((stage) => stage.per_hour != null)
                  .slice(-4)
                  .map((stage, index, list) => (
                    <span
                      key={`${stage.level}-${stage.started_at_secs}`}
                      className="tabular-nums text-[11px] text-slate-300 bg-slate-900/80 border border-white/10 rounded px-1.5 py-0.5"
                    >
                      Lv.{stage.level} {formatDuration(stage.secs)} ·{" "}
                      {formatNumber(Math.round(stage.per_hour!))}/时
                      {stage.ongoing && index === list.length - 1 ? "（进行中）" : ""}
                    </span>
                  ))}
              </div>
            )}

            {/* 地图：程序认小地图的**像素指纹**（不认字，理由见 exp::map），
                每张新地图填一次名字，之后每次来都自动填好。 */}
            <div className="flex items-center gap-2 flex-wrap pt-0.5">
              <MapPin className="w-3.5 h-3.5 text-slate-500 shrink-0" />
              <input
                value={mapName}
                onChange={(event) => setMapName(event.target.value)}
                placeholder="练级地图（填了会写进历史和小结卡片）"
                maxLength={24}
                className="flex-1 min-w-40 bg-slate-950/60 border border-white/10 rounded px-2 py-1 text-[11px] text-slate-200 placeholder:text-slate-600 focus:border-amber-500/40 outline-none"
              />
              {recentMaps.map((name) => (
                <button
                  key={name}
                  type="button"
                  onClick={() => setMapName(name)}
                  title={`填成「${name}」`}
                  className="text-[10px] text-slate-400 bg-slate-900/80 border border-white/10 rounded px-1.5 py-0.5 hover:text-amber-300 cursor-pointer"
                >
                  {name}
                </button>
              ))}
            </div>

            {/* 小结卡片：把这一段的结论画成一张能直接发出去的卡 */}
            <div className="flex items-center gap-2 flex-wrap">
              <button
                type="button"
                disabled={busy || !quality}
                onClick={() => {
                  if (session && quality) shareCard(cardData(session, quality, mapName));
                }}
                className="px-2.5 py-1 rounded-lg bg-amber-500/20 hover:bg-amber-500/30 border border-amber-500/40 disabled:opacity-50 text-[11px] font-bold text-amber-200 flex items-center gap-1 cursor-pointer"
              >
                <Share2 className="w-3 h-3" />
                小结卡片
              </button>
              <span className="text-[11px] text-slate-500">
                先看卡，再选复制（发微信 / QQ）还是存成图片
              </span>
            </div>
          </div>
        )}

        {/* 这段数据可不可信 —— 和「挂机多不多」分开说 */}
        {status?.quality && <QualityPanel quality={status.quality} />}

        {/* 结束之后先问一句，再决定这段留不留 */}
        {pending && (
          <div className="text-xs bg-emerald-500/10 border border-emerald-500/30 rounded-lg px-3 py-2 space-y-2">
            <div className="text-emerald-200">
              刚刚结束的这一段：{formatDuration(pending.active_secs)} · 获得{" "}
              <span className="tabular-nums">+{formatNumber(pending.gained_exp)}</span> ·{" "}
              {pending.quality.label}，要计入历史吗？
            </div>
            <p className="text-[11px] text-emerald-200/70">{pending.quality.reason}</p>
            {/* 结束瞬间冻结的结论：等级区间、平均时速、升级次数。
                会话一结束 session 就没了，没有这份快照这里就只剩两个数。 */}
            <div className="text-[11px] text-emerald-100/80 flex items-center gap-2 flex-wrap tabular-nums">
              <span>
                {formatLevelPercent(pending.summary.start_level, pending.summary.start_percent)}
                {" → "}
                {formatLevelPercent(pending.summary.current_level, pending.summary.current_percent)}
              </span>
              {pending.summary.per_hour != null && (
                <span>平均 {formatNumber(Math.round(pending.summary.per_hour))} / 小时</span>
              )}
              {pending.summary.level_ups > 0 && <span>升级 {pending.summary.level_ups} 次</span>}
            </div>
            {/* 地图就在这儿填：填完再点「计入历史」，它才存得进去 */}
            <div className="flex items-center gap-2">
              <MapPin className="w-3.5 h-3.5 text-emerald-200/70 shrink-0" />
              <input
                value={mapName}
                onChange={(event) => setMapName(event.target.value)}
                placeholder="这段在哪个图练的？（可空）"
                maxLength={24}
                className="flex-1 min-w-0 bg-slate-950/60 border border-white/10 rounded px-2 py-1 text-[11px] text-emerald-100 placeholder:text-slate-600 focus:border-emerald-400/50 outline-none"
              />
            </div>
            <div className="flex gap-2 flex-wrap">
              <button
                type="button"
                disabled={busy}
                onClick={() => resolvePending(true)}
                className="px-3 py-1.5 rounded-lg bg-emerald-500 hover:bg-emerald-400 disabled:opacity-50 font-bold text-slate-950 cursor-pointer"
              >
                计入历史
              </button>
              <button
                type="button"
                disabled={busy}
                onClick={() => resolvePending(false)}
                className="px-3 py-1.5 rounded-lg bg-slate-800 hover:bg-slate-700 disabled:opacity-50 border border-white/10 text-slate-200 cursor-pointer"
              >
                丢弃
              </button>
              <button
                type="button"
                disabled={busy}
                onClick={() => shareCard(cardData(pending.summary, pending.quality, mapName))}
                className="px-3 py-1.5 rounded-lg bg-amber-500/20 hover:bg-amber-500/30 disabled:opacity-50 border border-amber-500/40 text-amber-200 font-bold flex items-center gap-1 cursor-pointer"
              >
                <Share2 className="w-3 h-3" />
                分享
              </button>
            </div>
          </div>
        )}
            {!session && !pending && (
              <p className="text-xs text-slate-500 leading-relaxed">
                还没开始统计。点「开始统计」（或按快捷键）后，这里会显示这一段的收益、分段效率和数据可信度。
              </p>
            )}
          </div>
        </Panel>

      {/* 快捷键（本功能自己那一个） */}
      <Panel className="space-y-3">
        <h3 className="text-sm font-bold text-slate-100 flex items-center gap-2">
          <Keyboard className="w-4 h-4 text-amber-400" />
          开始 / 暂停快捷键
        </h3>
        <div className="flex items-start gap-3 flex-wrap">
          <div className="w-64 max-w-full shrink-0">
            <HotkeyInput
              value={settings?.exp_hotkey ?? ""}
              defaultValue={expHotkeyStatus?.default_hotkey ?? "Ctrl+Alt+P"}
              onChange={handleHotkeyChange}
              onCaptureChange={setCapturing}
            />
          </div>
          <HotkeyState status={expHotkeyStatus} />
        </div>
      </Panel>

      </div>

      {/* 角色：谁在线、各自练到哪了；点一个，下面的历史只看它的 */}
      <ExpCharacters
        characters={characters}
        currentId={currentCharacter}
        selectedId={viewCharacter}
        busy={busy}
        onSelect={setViewCharacter}
        onRename={(id, name) =>
          run(async () => {
            await invoke("rename_exp_character", { id, name });
            refreshHistory();
          })
        }
        onDelete={setPendingCharacter}
      />

      {/* 历史 */}
      <Panel className="col-span-12 space-y-4">
        <div className="flex items-center justify-between">
          <h3 className="text-sm font-bold text-slate-100 flex items-center gap-2 min-w-0">
            <TrendingUp className="w-4 h-4 text-amber-400" />
            历史统计
            {viewedCharacter && (
              <button
                type="button"
                onClick={() => setViewCharacter(null)}
                title="回到全部角色的历史"
                className="text-[11px] font-normal text-amber-200 bg-amber-500/10 border border-amber-500/30 rounded px-1.5 py-0.5 truncate cursor-pointer hover:bg-amber-500/20"
              >
                只看 {viewedCharacter.name} ×
              </button>
            )}
          </h3>
          <button
            type="button"
            onClick={() => setPendingClear(true)}
            className="text-xs text-red-300/80 hover:text-red-200 flex items-center gap-1 cursor-pointer"
          >
            <Trash2 className="w-3.5 h-3.5" />
            清空
          </button>
        </div>

        {totals != null && totals.sessions > 0 && (
          <p className="text-[11px] text-slate-500 tabular-nums">
            共 {totals.sessions} 段 · 累计 {formatDuration(totals.active_secs)} · 一共{" "}
            {formatNumber(totals.gained_exp)} 经验
          </p>
        )}

        {curve.length > 1 ? (
          <div className="h-56">
            <ResponsiveContainer width="100%" height="100%">
              <LineChart
                data={curve.map((point) => ({
                  at: formatClock(point.captured_unix),
                  exp: point.exp,
                }))}
              >
                <CartesianGrid stroke={chart.grid} strokeDasharray="3 3" />
                <XAxis dataKey="at" stroke={chart.axis} fontSize={11} minTickGap={40} />
                <YAxis
                  stroke={chart.axis}
                  fontSize={11}
                  width={70}
                  tickFormatter={(value: number) => formatNumber(value)}
                />
                <Tooltip
                  contentStyle={{
                    background: chart.tipBg,
                    border: `1px solid ${chart.tipBorder}`,
                    borderRadius: 8,
                    fontSize: 11,
                  }}
                  formatter={(value) => [formatNumber(Number(value)), "经验"]}
                />
                <Line
                  type="monotone"
                  dataKey="exp"
                  stroke={chart.line}
                  strokeWidth={2.5}
                  dot={false}
                />
              </LineChart>
            </ResponsiveContainer>
          </div>
        ) : (
          <p className="text-xs text-slate-500">
            还没有采样点
          </p>
        )}

        {history.length > 0 ? (
          <ul className="space-y-1.5">
            {history.map((row) => (
              <li
                key={row.id}
                className="text-xs text-slate-300 bg-slate-950/40 border border-white/10 rounded-lg px-3 py-2 flex items-center gap-2 flex-wrap"
              >
                <span className="tabular-nums text-slate-400">
                  {formatShortDate(row.started_unix)}
                </span>
                <span>{formatDuration(row.active_secs)}</span>
                {/* 谁练的：看全部角色时才标（只看一个角色时每行都一样，不用重复） */}
                {viewCharacter == null && row.character_name && (
                  <span
                    title={`这一段是 ${row.character_name} 练的`}
                    className="text-[10px] text-amber-200/90 bg-amber-500/10 border border-amber-500/20 rounded px-1.5 py-0.5"
                  >
                    {row.character_name}
                  </span>
                )}
                {/* 在哪练的：填过的才有，没填就不占位置 */}
                {row.map_name && (
                  <span
                    title={`这一段的练级地图：${row.map_name}`}
                    className="text-[10px] text-sky-200/90 bg-sky-500/10 border border-sky-500/20 rounded px-1.5 py-0.5"
                  >
                    {row.map_name}
                  </span>
                )}
                <span className="text-slate-500">
                  {formatLevelPercent(row.start_level, row.start_percent)} →{" "}
                  {formatLevelPercent(row.end_level, row.end_percent)}
                </span>
                <span className="tabular-nums text-emerald-300">
                  +{formatNumber(row.gained_exp)}
                </span>
                {row.active_secs > 60 && (
                  <span className="text-slate-500 tabular-nums">
                    约 {formatNumber(Math.round(row.gained_exp / (row.active_secs / 3600)))}/小时
                  </span>
                )}
                <span className="text-[10px] text-slate-500 tabular-nums">
                  画面 {Math.round(row.coverage * 100)}% · 有效打怪{" "}
                  {Math.round((1 - row.idle_ratio) * 100)}%
                </span>
                {/* 以前的一段也想发出去时：用同一套卡片重画一张 */}
                <button
                  type="button"
                  disabled={busy}
                  onClick={() => shareCard(historyCardData(row), row.id)}
                  title="看这一段的小结卡片（可以复制或存成图片）"
                  className="ml-auto p-1 rounded text-slate-400 hover:text-amber-300 hover:bg-white/10 disabled:opacity-40 cursor-pointer"
                >
                  <Share2 className="w-3 h-3" />
                </button>
                {/* 哪几段别拿去比：结论挂在这里，理由放在悬停提示里 */}
                <span
                  title={row.quality_reason}
                  className={`px-1.5 py-0.5 rounded text-[10px] border ${qualityTone(row.quality)}`}
                >
                  {qualityLabel(row.quality)}
                </span>
                {/* 误点了开始、挂着忘了关：这一段不想留在历史里 */}
                <button
                  type="button"
                  disabled={busy}
                  onClick={() => setPendingDelete(row)}
                  title="删掉这一段"
                  className="p-1 rounded text-slate-500 hover:text-red-300 hover:bg-white/10 disabled:opacity-40 cursor-pointer"
                >
                  <Trash2 className="w-3 h-3" />
                </button>
              </li>
            ))}
          </ul>
        ) : (
          <p className="text-xs text-slate-500">
            {viewedCharacter ? `${viewedCharacter.name} 还没有计入历史的记录` : "还没有历史记录"}
          </p>
        )}
      </Panel>

    </div>

      {sharing && (
        <ShareCardDialog
          data={sharing.data}
          pastRates={pastRates}
          serial={shareSerial}
          onClose={() => setSharing(null)}
        />
      )}

      <ConfirmDialog
        open={pendingDelete != null}
        title="删掉这一段历史？"
        body={
          pendingDelete && (
            <>
              {formatShortDate(pendingDelete.started_unix)} ·{" "}
              {formatDuration(pendingDelete.active_secs)}
              {pendingDelete.map_name ? ` · ${pendingDelete.map_name}` : ""} · +
              {formatNumber(pendingDelete.gained_exp)} 经验。
              <br />
              只删这一条记录和它的曲线采样点，别的不动；删了找不回来。
            </>
          )
        }
        confirmLabel="删除"
        onConfirm={() => {
          const row = pendingDelete;
          setPendingDelete(null);
          if (!row) return;
          run(async () => {
            await invoke("delete_exp_session", { id: row.id });
            refreshHistory();
            setNotice({ kind: "success", message: "已删掉这一段" });
          });
        }}
        onCancel={() => setPendingDelete(null)}
      />

      <ConfirmDialog
        open={pendingCharacter != null}
        title="从列表里拿掉这个角色？"
        body={
          pendingCharacter && (
            <>
              {pendingCharacter.name}
              {pendingCharacter.job ? `（${pendingCharacter.job}）` : ""}。
              <br />
              它名下的 {pendingCharacter.sessions} 段历史会留着，只是不再标是谁练的；
              下次登录这个角色，程序会重新把它记进列表。
            </>
          )
        }
        confirmLabel="拿掉"
        onConfirm={() => {
          const character = pendingCharacter;
          setPendingCharacter(null);
          if (!character) return;
          run(async () => {
            await invoke("delete_exp_character", { id: character.id });
            if (viewCharacter === character.id) setViewCharacter(null);
            refreshHistory();
          });
        }}
        onCancel={() => setPendingCharacter(null)}
      />

      <ConfirmDialog
        open={pendingClear}
        title="清空全部经验历史？"
        body="会删掉所有会话记录和曲线采样点，只影响这台机器上的数据，无法恢复。"
        confirmLabel="清空"
        onConfirm={() => {
          setPendingClear(false);
          run(async () => {
            const removed = await invoke<number>("clear_exp_history");
            refreshHistory();
            setNotice({ kind: "success", message: `已清空 ${removed} 段记录` });
          });
        }}
        onCancel={() => setPendingClear(false)}
      />
    </>
  );
};

/**
 * 这一段数据的可信度。
 *
 * 两个数字分开列：**画面覆盖率**是「我们能不能看见」（数据可不可信），
 * **有效打怪占比**是「你是不是一直在打」（玩法）—— 后者低不代表数据有问题。
 */
const QualityPanel: React.FC<{ quality: ExpSessionQuality }> = ({ quality }) => (
  <div
    className={`rounded-lg border px-3 py-2 space-y-1 text-xs ${qualityTone(quality.grade)}`}
  >
    <div className="flex items-center gap-2 flex-wrap">
      {quality.grade >= 2 ? (
        <ShieldCheck className="w-3.5 h-3.5" />
      ) : (
        <ShieldAlert className="w-3.5 h-3.5" />
      )}
      <span className="font-bold">本段数据 · {quality.label}</span>
      <span className="tabular-nums opacity-80">
        画面覆盖 {Math.round(quality.coverage * 100)}% · 有效打怪{" "}
        {Math.round(quality.active_ratio * 100)}%
      </span>
      {quality.rejected_frames > 0 && (
        <span className="tabular-nums opacity-80">拒绝帧 {quality.rejected_frames}</span>
      )}
      {quality.bar_conflicts > 0 && (
        <span className="tabular-nums opacity-80">条对不上 {quality.bar_conflicts}</span>
      )}
      {quality.gated_gains > 0 && (
        <span className="tabular-nums opacity-80">丢弃增量 {quality.gated_gains}</span>
      )}
    </div>
    <p className="text-[11px] opacity-80">{quality.reason}</p>
  </div>
);

/** 质量结论的颜色（2 绿 / 1 黄 / 0 红）。 */
function qualityTone(grade: number): string {
  if (grade >= 2) return "border-emerald-500/30 bg-emerald-500/10 text-emerald-200";
  if (grade === 1) return "border-amber-500/30 bg-amber-500/10 text-amber-200";
  return "border-red-500/30 bg-red-500/10 text-red-200";
}

/**
 * 历史列表里的标签。
 *
 * 措辞**和后端 `SessionQuality::label` 保持一致**（「数据完整 / 可参考 / 不建议用于比较」）：
 * 同一件事在两处叫两个名字，用户会以为是两个不同的指标。
 */
function qualityLabel(grade: number): string {
  if (grade >= 2) return "数据完整";
  if (grade === 1) return "可参考";
  return "不建议用于比较";
}

/**
 * 速率下面那行说明：读不到 / 已停止 / 折算 / 近一段时间。
 * 还没测满一个整区间时，数字是按实测区间折算的 —— 必须写出来。
 */
function rateCaption(
  spanSecs: number,
  spanLabel: string,
  stopped: boolean,
  readOk: boolean,
  idleSecs: number,
): string {
  const partial = spanSecs > 5 && spanSecs < (spanLabel === "一分钟" ? 60 : 3600) * 0.98;
  if (!readOk) return "读不到游戏画面";
  if (stopped) return `已停止 · ${formatDuration(idleSecs)}没有新收益`;
  if (partial) return `实测 ${formatDuration(spanSecs)}折算`;
  return `近${spanLabel}`;
}

/** 一个速率格子：数字 + 它实际测了多久 / 停手状态。 */
const RateBox: React.FC<{
  label: string;
  value: number | null;
  spanSecs: number;
  spanLabel: string;
  stopped: boolean;
  readOk: boolean;
  idleSecs: number;
}> = ({ label, value, spanSecs, spanLabel, stopped, readOk, idleSecs }) => {
  const idle = !readOk || stopped || value == null;
  return (
    <div className="rounded-xl bg-slate-800/60 border border-white/5 px-3 py-2.5 space-y-1">
      <div className="text-[11px] text-slate-500">{label}经验</div>
      <div
        className={`text-xl font-extrabold leading-tight tabular-nums ${
          idle ? "text-slate-600" : "text-slate-100"
        }`}
      >
        {value == null ? "—" : formatNumber(Math.round(value))}
      </div>
      <div className="text-[11px] text-slate-500">
        {rateCaption(spanSecs, spanLabel, stopped, readOk, idleSecs)}
      </div>
    </div>
  );
};

/** 记录状态胶囊：● 记录中 03:12 / 已暂停 / 未开始。 */
const PhasePill: React.FC<{ phase: string; secs: number }> = ({ phase, secs }) => {
  const running = phase === "running";
  const active = running || phase === "paused";
  return (
    <span
      className={`inline-flex items-center gap-1.5 px-2.5 py-0.5 rounded-full text-[11px] font-normal ${
        running ? "bg-emerald-500/15 text-emerald-300" : "bg-slate-800 text-slate-400"
      }`}
    >
      {running && <span className="w-1.5 h-1.5 rounded-full bg-emerald-400 animate-pulse" />}
      {active ? (running ? "记录中" : "已暂停") : "未开始"}
      {active && <span className="tabular-nums">{formatStopwatch(secs)}</span>}
    </span>
  );
};

const HotkeyState: React.FC<{ status: HotkeyStatus | undefined }> = ({ status }) => {
  if (!status) return null;
  if (status.registered) {
    return (
      <span className="text-[11px] text-emerald-300 mt-2">
        已生效（{status.hotkey}）
      </span>
    );
  }
  return (
    <span className="text-[11px] text-red-300 mt-2">
      {status.error ?? "没注册上"}（想用的键可能被别的程序占着，换一个）
    </span>
  );
};

/** 预计升级时间：一小时以内报分钟，再长报小时。 */
function formatEta(secs: number): string {
  if (secs < 60) return "不足 1 分钟";
  if (secs < 3600) return `${Math.round(secs / 60)} 分钟`;
  const hours = Math.floor(secs / 3600);
  const minutes = Math.round((secs - hours * 3600) / 60);
  return minutes > 0 ? `${hours} 小时 ${minutes} 分` : `${hours} 小时`;
}

function formatLevelPercent(level: number | null, percent: number | null): string {
  if (level == null && percent == null) return "—";
  const levelText = level != null ? `Lv.${level}` : "Lv.—";
  return percent != null ? `${levelText} ${percent.toFixed(2)}%` : levelText;
}
