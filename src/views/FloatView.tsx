import React, { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { ChevronsDownUp, ChevronsUpDown, Frame, Pause, Play, Square, X } from "lucide-react";
import { formatEtaCompact, formatNumber, formatStopwatch, joinBig, splitBig } from "../lib/format";
import { useExpSession } from "../lib/useExpSession";
import { useMemorySample } from "../lib/memory";
import { ChannelBadge } from "../components/ChannelBadge";
import { ResizeHandles, useEnterAnimation, useWindowDrag, useWindowSize } from "../components/WindowFrame";
import { useTween } from "../lib/useTween";
import { gameAway, gameAwayText } from "../lib/expRead";

/**
 * **经验窗**（`float` 窗口，全局快捷键默认 F10，切换式显示 / 收起）。
 *
 * 以前叫「精简悬浮窗」，里面是「金价 + 内存 + 经验」三张卡片。重设计后它只做一件事：
 * **打怪时瞄一眼经验**——所以金价拿掉了（要看金价去主窗口的「行情」页或总览），
 * 只留经验，外加一条很淡的内存线（这个工具本来就是来报内存的）。
 *
 * ## 大小与自适应
 *
 * 窗口可以拖成任意大小，内容按窗口大小重新排版（容器查询，见 styles/overlay.css）：
 * 越矮显示的东西越少，最矮变成一条单行条；越宽主数字越大。不再做整体缩放。
 *
 * ## 按钮必须「立刻有反应」
 *
 * 几秒才采一次样，所以**不能等 `exp-update` 事件回来再更新界面** ——
 * 那样点一下要等一两秒才看到变化。做法是：
 *
 * 1. 点下去**先按预期改本地状态**（乐观更新），界面当帧就变；
 * 2. 同时把命令发出去，用**命令的返回值**（后端直接把新状态回给我们）覆盖掉；
 * 3. 事件仍然在监听，只是它现在是「兜底」而不是唯一来源。
 *
 * 开始 / 暂停 / 结束**不吃冷却**：它们是本地状态切换，不发任何网络请求，
 * 套冷却会让「暂停之后马上点开始」被自己拦掉。只用 `busy` 防重入。
 */

/** 头部和单行条都要完整放下「品牌 + 区服｜线 + 上次的线」；和 tauri.conf.json 里的 minWidth 一致 */
const MIN_W = 470;
const MIN_H = 56;
/** 单行条的窗口高度 = 面板 44 + 上下留白 12 */
const STRIP_H = 56;
const DEFAULT_H = 282;
const STRIP_BELOW = 92;

export const FloatView: React.FC = () => {
  const memory = useMemorySample();
  const { exp, busy, actionError, toggle: toggleSession, end, resolve: resolveSession } = useExpSession();
  const [tiny, setTiny] = useState(() => window.innerHeight < STRIP_BELOW);
  /** 收起成单行条之前的窗口高度，展开时回到它 */
  const expandedHeight = useRef(DEFAULT_H);

  const resizeTo = useWindowSize("mxdbox.window.float", MIN_W, MIN_H);
  useEnterAnimation();
  const drag = useWindowDrag();

  useEffect(() => {
    const onResize = () => {
      setTiny(window.innerHeight < STRIP_BELOW);
      if (window.innerHeight >= STRIP_BELOW) expandedHeight.current = window.innerHeight;
    };
    onResize();
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") invoke("hide_lite_float");
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, []);

  const hide = () => invoke("hide_lite_float");

  const toggleStrip = () => {
    const width = window.innerWidth;
    if (tiny) resizeTo(width, Math.max(expandedHeight.current, 160));
    else {
      expandedHeight.current = window.innerHeight;
      resizeTo(width, STRIP_H);
    }
  };

  /** 双击标题栏收起 / 展开；双击在按钮上（比如连点暂停）不算。 */
  const onHeadDoubleClick = (e: React.MouseEvent) => {
    if ((e.target as HTMLElement).closest("button")) return;
    toggleStrip();
  };

  /**
   * 「校准位置」：优先开**游戏画面蒙版**（正好盖在游戏客户区上、直接在真实画面上拖框，
   * 主窗口会先让开）。开不出来（游戏没开 / 独占全屏）再退回主窗口的截图浮层。
   * 最后收起自己：悬浮窗是置顶的，留着会盖住蒙版或主窗口。
   */
  const openCalibration = async () => {
    try {
      await invoke("open_calibration_overlay");
    } catch {
      void emit("open-exp-calibration").catch(() => {});
      void invoke("focus_main_window").catch(() => {});
    }
    hide();
  };

  const phase = exp?.phase ?? "idle";
  const running = phase === "running";
  const paused = phase === "paused";
  const active = running || paused;
  const unreadable = exp != null && exp.read_state !== "ok";
  /** 游戏没开 / 最小化：只说一句，不劝人去校准 */
  const away = gameAway(exp?.read_state);
  const blind = !!exp?.stopped || unreadable;
  const session = exp?.session ?? null;
  const level = session?.current_level ?? exp?.level ?? null;
  const percent = session?.current_percent ?? exp?.percent ?? null;
  // 大数字滚动：读数变化时滚过去而不是直接跳（读不到时保持 null，不拿 0 顶替）
  const perHourTween = useTween(blind ? null : (exp?.per_hour ?? null));
  const perHour = splitBig(perHourTween);
  const pending = exp?.pending ?? null;
  const goal = exp?.goal ?? null;
  const gained = session != null ? joinBig(session.gained_exp, true) : "—";
  const perMinute =
    blind || exp?.per_minute == null ? "—" : `${formatNumber(Math.round(exp.per_minute))}`;
  // 没开始统计就没有速率可言：和旁边两格一样留「—」，「测量中」只在统计开始后才成立
  const eta = blind || !active
    ? "—"
    : session?.eta_secs != null
      ? formatEtaCompact(session.eta_secs)
      : exp?.per_hour === 0
        ? "未涨经验"
        : "测量中";
  const lvText = level != null ? `Lv.${level}` : "Lv.—";
  const pctText = percent != null ? `${percent.toFixed(2)}%` : "";
  const timer = formatStopwatch(session?.active_secs ?? 0);

  const playIcon = running ? <Pause size={14} /> : <Play size={14} />;
  const playTitle = running ? "暂停统计" : paused ? "继续统计" : "开始统计";
  const canEnd = !busy && exp != null && phase !== "idle" && exp.pending == null;

  return (
    <div className="ov-root">
      <ResizeHandles />
      <div className="xf-panel">
        {/* 头部：拖动区。窄了先收品牌名 / 频道，再矮了整块让位给单行条 */}
        <div className="xf-head" {...drag} onDoubleClick={onHeadDoubleClick} title="双击标题栏：收起成单行条 / 展开">
          <span className="xf-brand">
            枫之助
          </span>
          <span className="xf-channel">
            <ChannelBadge compact align="left" />
          </span>
          {/* 没开始就什么都不写（下面的「开始统计」按钮已经说明了）。
              窗口窄时只留 圆点 / 暂停符号 + 计时，把地方让给频道胶囊 */}
          {active && (
            <span className={`xf-status ${running ? "on" : ""}`} title={running ? "记录中" : "已暂停"}>
              {running ? <span className="xf-dot" /> : <Pause size={11} />}
              <span className="xf-word">{running ? "记录中" : "已暂停"}</span>
              <span className="xf-clock">{timer}</span>
            </span>
          )}
          <span className="xf-spacer" />
          <span className="xf-headact">
            <button
              type="button"
              className={`xf-ib ${running ? "" : "good"}`}
              title={playTitle}
              disabled={busy}
              onClick={toggleSession}
            >
              {playIcon}
            </button>
            <button
              type="button"
              className="xf-ib"
              title="结束这一段"
              disabled={!canEnd}
              onClick={end}
            >
              <Square size={13} />
            </button>
          </span>
          <button
            type="button"
            className="xf-ib"
            title="收起成单行条（双击标题栏也可以）"
            aria-label="收起成单行条"
            onClick={toggleStrip}
          >
            <ChevronsDownUp size={14} />
          </button>
          <button type="button" className="xf-ib" title="收起 (Esc)" onClick={hide}>
            <X size={14} />
          </button>
        </div>

        <div className="xf-body">
          <div className="xf-hero">
            <div style={{ minWidth: 0 }}>
              <div className="xf-lbl">经验 / 小时</div>
              <div className={`xf-num ${blind || perHour.n === "—" ? "blind" : ""}`}>
                {perHour.n}
                {perHour.unit && <small>{perHour.unit}</small>}
              </div>
            </div>
            <div className="xf-lv">
              <b>{lvText}</b>
              <span>{pctText}</span>
            </div>
          </div>

          <div className="xf-bar" title={pctText}>
            <i style={{ width: `${Math.min(100, Math.max(0, percent ?? 0))}%` }} />
          </div>

          {/* 中矮：一行小字 */}
          <div className="xf-inline">
            <span>
              净 <b className="good">{gained}</b>
            </span>
            <span>
              每分 <b>{perMinute}</b>
            </span>
            <span>
              升级 <b>{eta}</b>
            </span>
          </div>

          {/* 中高：三格统计 */}
          <div className="xf-stats">
            <div className="xf-stat">
              <div className="xf-lbl">净经验</div>
              <b className="good">{gained}</b>
            </div>
            <div className="xf-stat opt">
              <div className="xf-lbl">每分钟</div>
              <b>{perMinute}</b>
            </div>
            <div className="xf-stat">
              <div className="xf-lbl">距升级</div>
              <b>
                {eta}
                {session?.remaining_exp != null && !blind && (
                  <span className="xf-rem"> · {joinBig(session.remaining_exp)}</span>
                )}
              </b>
            </div>
          </div>

          {/* 有话要说才占一行：待确认 / 后端拒绝 / 读不到 / 还没开始 */}
          {pending != null ? (
            <div className="xf-note">
              <span>
                这一段 {formatStopwatch(pending.active_secs)} · +{joinBig(pending.gained_exp)}
              </span>
              <span style={{ display: "flex", gap: 6 }}>
                <button
                  type="button"
                  className="xf-btn ok"
                  disabled={busy}
                  onClick={() => resolveSession(true)}
                >
                  计入历史
                </button>
                <button
                  type="button"
                  className="xf-btn"
                  disabled={busy}
                  onClick={() => resolveSession(false)}
                >
                  丢弃
                </button>
              </span>
            </div>
          ) : actionError ? (
            <div className="xf-note bad">
              <span title={actionError}>{actionError}</span>
            </div>
          ) : away ? (
            <div className="xf-note">
              <span>{gameAwayText(exp?.read_state)}，进游戏后会自动开始读</span>
            </div>
          ) : unreadable ? (
            <div className="xf-note">
              <span>未识别到经验条</span>
              <button type="button" className="xf-btn go" onClick={openCalibration}>
                <Frame size={12} />
                校准位置
              </button>
            </div>
          ) : null}

          <div className="xf-actions">
            <button
              type="button"
              className={`xf-btn ${running ? "" : "go"}`}
              disabled={busy}
              onClick={toggleSession}
            >
              {playIcon}
              {running ? "暂停" : paused ? "继续" : "开始统计"}
            </button>
            <button
              type="button"
              className="xf-btn"
              disabled={!canEnd}
              onClick={end}
            >
              <Square size={13} />
              结束
            </button>
          </div>

          <div className="xf-foot">
            <span title={exp?.map_name ?? ""}>{exp?.map_name ? exp.map_name : "地图未识别"}</span>
            {/* 练级目标（在主窗口「练级」页设）：统计中显示还要多久，否则显示还差多少 */}
            {goal && !goal.reached && goal.remaining_exp != null && (
              <span title={`练级目标 Lv.${goal.target_level}：还差 ${joinBig(goal.remaining_exp)} 经验`}>
                目标 Lv.{goal.target_level} ·{" "}
                {goal.eta_secs != null && active && !blind
                  ? formatEtaCompact(goal.eta_secs)
                  : `差 ${joinBig(goal.remaining_exp)}`}
              </span>
            )}
            {exp?.quality != null && exp.quality.grade < 2 && (
              <span
                title={exp.quality.reason}
                style={{ color: exp.quality.grade === 0 ? "var(--ui-bad)" : "var(--ui-ac)" }}
              >
                画面 {Math.round(exp.quality.coverage * 100)}%
              </span>
            )}
            <span
              className={`xf-mem ${memory?.is_warning ? "warn" : ""}`}
              title={
                memory
                  ? `已用 ${memory.used_mb}MB / 共 ${memory.total_mb}MB，警戒线 ${memory.threshold}%`
                  : "正在读取内存占用…"
              }
            >
              内存 {memory ? `${memory.percent.toFixed(0)}%` : "…"}
              <i>
                <em style={{ width: `${Math.min(100, memory?.percent ?? 0)}%` }} />
              </i>
            </span>
          </div>
        </div>

        {/* 单行条：面板很矮时替换上面两块 */}
        <div className="xf-strip" {...drag} onDoubleClick={onHeadDoubleClick} title="双击：展开">
          <span className="xf-brand">枫之助</span>
          <span className={`xf-status ${running ? "on" : ""}`}>
            <span className="xf-dot" />
          </span>
          <span className={`xf-num ${blind || perHour.n === "—" ? "blind" : ""}`}>
            {perHour.n}
            {perHour.unit && <small>{perHour.unit}/时</small>}
          </span>
          {/* 单行条里只看、不点：窗口只有一行高，浮层开不出来（展开后再点） */}
          <span className="xf-channel">
            <ChannelBadge compact align="left" />
          </span>
          <span className="xf-lvline">
            {lvText} {pctText}
          </span>
          <div className="xf-bar">
            <i style={{ width: `${Math.min(100, Math.max(0, percent ?? 0))}%` }} />
          </div>
          <button
            type="button"
            className={`xf-ib ${running ? "" : "good"}`}
            title={playTitle}
            disabled={busy}
            onClick={toggleSession}
          >
            {playIcon}
          </button>
          <button type="button" className="xf-ib" title="展开成完整窗口" aria-label="展开" onClick={toggleStrip}>
            <ChevronsUpDown size={14} />
          </button>
          <button type="button" className="xf-ib" title="收起 (Esc)" onClick={hide}>
            <X size={14} />
          </button>
        </div>
      </div>
    </div>
  );
};
