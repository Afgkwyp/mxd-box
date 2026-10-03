import React, { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { History, Signal } from "lucide-react";
import type { ChannelState, ChannelVisit } from "../types";
import { useChannelSample } from "../lib/channel";

/** 1–60，按十位分组 —— 和游戏里那页格子一个排法。 */
const CHANNEL_ROWS: number[][] = Array.from({ length: 6 }, (_, row) =>
  Array.from({ length: 10 }, (_, col) => row * 10 + col + 1),
);

/** 「绿水灵 · 47线」；不知道区服就只有线号。 */
function lineText(server: string | null, channel: number): string {
  return server ? `${server} · ${channel}线` : `${channel}线`;
}

/** 今天的只写「11:05」，隔天的带上日期「9-30 11:05」；没有时间记录就是空串。 */
function formatSeen(unix: number): string {
  if (!unix) return "";
  const at = new Date(unix * 1000);
  const time = `${String(at.getHours()).padStart(2, "0")}:${String(at.getMinutes()).padStart(2, "0")}`;
  return at.toDateString() === new Date().toDateString()
    ? time
    : `${at.getMonth() + 1}-${at.getDate()} ${time}`;
}

/** 「11:05 断开」/「11:20 换线」 */
function visitNote(visit: ChannelVisit): string {
  return `${formatSeen(visit.last_seen_unix)} ${visit.dropped ? "断开" : "换线"}`.trim();
}

/**
 * 「当前在哪条线」胶囊（区服 + 线号）+「上次在哪条线」胶囊 + 校准格。
 *
 * 自动值来自游戏连接的端口（`src-tauri/src/channel.rs`：不截屏、不碰游戏内存，
 * 只是查系统 TCP 表）。常规部署有固定规律，直接算；认不出形状的部署退回
 * 「点一次、按节点记住」。
 *
 * 「上次」是给掉线准备的：掉线重登往往进的不是原来那条线，这时旁边那颗胶囊
 * 就是掉线前的线（琥珀色 = 连接断开后离开的；灰色 = 自己换线走的）。
 * 游戏没开时只剩这一颗。点开能看到最近待过的几条线。
 *
 * `align`：浮层朝哪边展开；`side`：浮层往上还是往下开（贴着窗口底边的用 `top`）。
 * `wrap`：位置窄时让「上次」胶囊折到下一行，而不是把一行撑破。
 */
export const ChannelBadge: React.FC<{
  compact?: boolean;
  align?: "left" | "right";
  side?: "top" | "bottom";
  wrap?: boolean;
}> = ({ compact, align = "right", side = "bottom", wrap }) => {
  const state = useChannelSample();
  const [open, setOpen] = useState(false);
  const [fixing, setFixing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const boxRef = useRef<HTMLDivElement>(null);

  // 点浮层外面关掉（和 HotkeyInput 同一套做法）
  useEffect(() => {
    if (!open) return;
    const onDown = (event: MouseEvent) => {
      if (boxRef.current && !boxRef.current.contains(event.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [open]);

  const pick = (channel: number) => {
    if (busy) return;
    setBusy(true);
    setError("");
    invoke<ChannelState>("set_channel_manual", { channel })
      .then(() => setOpen(false))
      .catch((err: unknown) => {
        setError(typeof err === "string" ? err : "没定上，再试一次");
      })
      .finally(() => setBusy(false));
  };

  const channel = state?.channel ?? null;
  const server = state?.server ?? null;
  const source = state?.source ?? "stale";
  const stale = source === "stale";
  /** 现在确实连着、也认得出线号 */
  const live = channel != null && !stale;
  const uncalibrated = state != null && state.host != null && channel == null;

  // 待过的线，新的在前。连接没了的时候，「断线前那条」就是状态里留着的线号。
  const recent = state?.recent ?? [];
  const history: ChannelVisit[] =
    stale && channel != null
      ? [{ server, channel, last_seen_unix: state?.updated_unix ?? 0, dropped: true }, ...recent]
      : recent;
  const last = history[0] ?? null;

  const where = state?.host != null ? `${state.host}:${state.port ?? "?"}` : "";
  const toggle = () => {
    setOpen((value) => !value);
    setFixing(false);
  };

  const mainTooltip = live
    ? `${source === "manual" ? "手动定的" : "自动读到"} ${lineText(server, channel)}（${where}）；点一下可以改`
    : uncalibrated
      ? `还没认出线号（连着 ${where}）。在登录 / 选角色界面是正常的，进角色后会自动显示`
      : "没读到当前线（游戏没开或还没连上）—— 点一下看看";

  const mainTone = live
    ? source === "manual"
      ? "border-amber-500/40 bg-amber-500/10 text-amber-300"
      : "border-sky-500/30 bg-slate-800/80 text-sky-400"
    : uncalibrated
      ? "border-amber-500/40 bg-amber-500/10 text-amber-300"
      : "border-white/10 bg-slate-800/80 text-slate-500";

  const pill = `inline-flex items-stretch overflow-hidden rounded-full border whitespace-nowrap cursor-pointer transition-colors hover:border-sky-400/60 ${
    compact ? "text-[10px] leading-4" : "text-xs leading-5"
  }`;
  const icon = compact ? "w-2.5 h-2.5 shrink-0" : "w-3.5 h-3.5 shrink-0";
  const segment = `flex items-center gap-1 ${compact ? "px-1.5 py-0.5" : "px-2.5 py-0.5"}`;

  const mainText = live ? `${channel}线` : uncalibrated ? "线号未知" : "未连接";
  // 游戏没开但记得上次的线：只留「上次」那一颗，不再多挂一颗「未连接」
  const showMain = live || uncalibrated || last == null;

  // 「上次」和现在是同一个区服就不重复写区服名
  const lastText =
    last == null
      ? ""
      : lineText(live && last.server === server ? null : last.server, last.channel);
  const lastTooltip =
    last == null
      ? ""
      : `上次在 ${lineText(last.server, last.channel)}${
          last.last_seen_unix ? `，${formatSeen(last.last_seen_unix)} ` : "，"
        }${last.dropped ? "连接断开（掉线 / 下线）" : "换线离开"}；点一下看最近待过的线`;

  const hint =
    state?.host == null
      ? "游戏没开或还没连上；进角色后这里会自动显示所在的线"
      : channel != null
        ? `${lineText(server, channel)}，来自游戏连接 ${where}${state.base != null ? `（手动基准 ${state.base}）` : ""}`
        : `连着 ${where}，还没认出线号。在登录 / 选角色界面是正常的，进角色后会自动显示。`;

  return (
    <div ref={boxRef} className="relative min-w-0">
      {/* 类名 ch-* 是给悬浮窗样式用的钩子（overlay.css：窗口窄时决定谁让位） */}
      <div className={`ch-pills flex items-center gap-1 ${wrap ? "flex-wrap justify-end" : ""}`}>
        {showMain && (
          <button
            type="button"
            title={mainTooltip}
            onClick={toggle}
            className={`ch-main ${pill} ${mainTone}`}
          >
            {server && (live || uncalibrated) ? (
              <>
                <span className={`ch-server ${segment} bg-white/5 border-r border-current/20`}>
                  <Signal className={icon} />
                  {server}
                </span>
                <span className={`${segment} font-bold tabular-nums`}>{mainText}</span>
              </>
            ) : (
              <span className={`${segment} font-bold tabular-nums`}>
                <Signal className={icon} />
                {mainText}
              </span>
            )}
          </button>
        )}

        {last && (
          <button
            type="button"
            title={lastTooltip}
            onClick={toggle}
            className={`ch-last ${pill} border-dashed ${
              last.dropped
                ? "ch-dropped border-amber-500/40 text-amber-300"
                : "border-white/15 text-slate-400"
            }`}
          >
            <span className={segment}>
              <History className={icon} />
              <span className="opacity-80">上次</span>
              <span className="font-bold tabular-nums">{lastText}</span>
            </span>
          </button>
        )}
      </div>

      {open && (
        <div
          className={`ch-pop absolute z-50 w-[268px] rounded-xl border border-white/15 bg-slate-900 p-2 shadow-2xl ${
            side === "top" ? "bottom-full mb-1.5" : "top-full mt-1.5"
          } ${align === "left" ? "left-0" : "right-0"}`}
        >
          <div className="px-1 pb-1.5 text-[10px] leading-snug text-slate-400">{hint}</div>

          {history.length > 0 && (
            <div className="mb-2 rounded-lg bg-slate-800/60 px-2 py-1.5">
              <div className="pb-1 text-[10px] text-slate-500">最近待过的线（掉线后回来找这里）</div>
              {history.map((visit, index) => (
                <div
                  key={`${visit.server ?? ""}-${visit.channel}`}
                  className="flex items-center justify-between gap-2 py-0.5 text-[11px]"
                >
                  <span
                    className={`font-bold tabular-nums ${index === 0 ? "text-slate-100" : "text-slate-300"}`}
                  >
                    {lineText(visit.server, visit.channel)}
                  </span>
                  <span className={visit.dropped ? "text-amber-300" : "text-slate-500"}>
                    {visitNote(visit)}
                  </span>
                </div>
              ))}
            </div>
          )}

          {/* 校准格平时用不上，收在一行字后面。认不出线号时也不直接摊开：
              登录 / 选角色界面连的是登录服，本来就没有线号，这时点了格子会给登录服记一个错的基准 */}
          {state?.host != null && !fixing && (
            <button
              type="button"
              onClick={() => setFixing(true)}
              className="px-1 text-[10px] text-slate-500 underline underline-offset-2 cursor-pointer hover:text-slate-300"
            >
              {live ? "线号不对？点这里纠正" : "已经进了角色还是未知？点这里手动选线号"}
            </button>
          )}
          {state?.host != null && fixing && (
            <>
              <div className="px-1 pb-1 text-[10px] text-slate-500">
                点一下你真实所在的线（这台服务器只需选一次）
              </div>
              <div className="grid grid-cols-10 gap-1">
                {CHANNEL_ROWS.flat().map((value) => (
                  <button
                    key={value}
                    type="button"
                    disabled={busy}
                    onClick={() => pick(value)}
                    className={`h-6 rounded text-[11px] font-mono cursor-pointer transition-colors disabled:opacity-50 ${
                      live && value === channel
                        ? "bg-amber-500 text-slate-950 font-bold"
                        : "bg-slate-800 text-slate-300 hover:bg-slate-700"
                    }`}
                  >
                    {value}
                  </button>
                ))}
              </div>
            </>
          )}
          {error && <div className="px-1 pt-1 text-[10px] text-red-300">{error}</div>}
        </div>
      )}
    </div>
  );
};
