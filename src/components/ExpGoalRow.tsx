import React, { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Flag, X } from "lucide-react";
import { formatEtaCompact, joinBig } from "../lib/format";
import type { ExpGoal, ExpStatus } from "../types";

const MIN_LEVEL = 2;
const MAX_LEVEL = 120;

/**
 * 练级目标：填一个目标等级，告诉你「还差多少经验、按本段时速还要多久」。
 *
 * * 没在统计时也显示「还差多少」（进游戏就在读等级和经验），只是没有时间 ——
 *   时间要有时速才算得出来，时速只在统计开始后才有；
 * * `rateStale`（读不到 / 已停手）时不显示时间：那时候的时速已经不代表「现在」了，
 *   和旁边「距升级」同一个口径；
 * * 目标存在后端设置里，重启还在；改完立刻用命令的返回值更新界面，不等下一次推送。
 */
export const ExpGoalRow: React.FC<{
  goal: ExpGoal | null;
  /** 当前等级（读不到时为 null）；用来给输入框一个合理的默认值 */
  level: number | null;
  rateStale: boolean;
  onStatus: (status: ExpStatus) => void;
}> = ({ goal, level, rateStale, onStatus }) => {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  const save = (target: number | null) => {
    if (busy) return;
    setBusy(true);
    setError("");
    invoke<ExpStatus>("set_exp_goal", { level: target })
      .then((status) => {
        onStatus(status);
        setEditing(false);
      })
      .catch((err: unknown) => setError(typeof err === "string" ? err : "没保存上，再试一次"))
      .finally(() => setBusy(false));
  };

  const submit = () => {
    const value = Number(draft);
    if (!Number.isInteger(value) || value < MIN_LEVEL || value > MAX_LEVEL) {
      setError(`目标等级要填 ${MIN_LEVEL}–${MAX_LEVEL} 之间的整数`);
      return;
    }
    if (level != null && value <= level) {
      setError(`你已经 Lv.${level} 了，目标要比它高`);
      return;
    }
    save(value);
  };

  const startEditing = () => {
    setDraft(String(goal?.target_level ?? (level != null ? Math.min(MAX_LEVEL, level + 1) : "")));
    setError("");
    setEditing(true);
  };

  if (editing || goal == null) {
    return (
      <div className="text-xs text-slate-300 space-y-1">
        <div className="flex items-center gap-2 flex-wrap">
          <Flag className="w-3.5 h-3.5 text-slate-500 shrink-0" />
          <span className="text-slate-400">练级目标</span>
          {editing ? (
            <>
              <span className="text-slate-500">Lv.</span>
              <input
                autoFocus
                inputMode="numeric"
                value={draft}
                onChange={(event) => setDraft(event.target.value.replace(/[^\d]/g, "").slice(0, 3))}
                onKeyDown={(event) => {
                  if (event.nativeEvent.isComposing) return;
                  if (event.key === "Enter") submit();
                  if (event.key === "Escape") setEditing(false);
                }}
                placeholder="目标等级"
                className="w-20 bg-slate-950/60 border border-white/10 rounded px-2 py-1 text-[11px] text-slate-200 placeholder:text-slate-600 focus:border-amber-500/40 outline-none tabular-nums"
              />
              <button
                type="button"
                disabled={busy}
                onClick={submit}
                className="px-2.5 py-1 rounded-lg bg-amber-500/20 hover:bg-amber-500/30 border border-amber-500/40 disabled:opacity-50 text-[11px] font-bold text-amber-200 cursor-pointer"
              >
                定下来
              </button>
              <button
                type="button"
                onClick={() => setEditing(false)}
                className="text-[11px] text-slate-500 hover:text-slate-300 cursor-pointer"
              >
                取消
              </button>
            </>
          ) : (
            <button
              type="button"
              onClick={startEditing}
              className="text-[11px] text-slate-400 underline underline-offset-2 hover:text-amber-300 cursor-pointer"
            >
              设一个目标等级，看还要练多久
            </button>
          )}
        </div>
        {error && <div className="text-[11px] text-red-300">{error}</div>}
      </div>
    );
  }

  return (
    <div className="text-xs text-slate-300 space-y-1">
      <div className="flex items-center gap-2 flex-wrap">
        <Flag className={`w-3.5 h-3.5 shrink-0 ${goal.reached ? "text-emerald-400" : "text-amber-400"}`} />
        <span>
          目标 <b className="text-slate-100 tabular-nums">Lv.{goal.target_level}</b>
        </span>
        {goal.reached ? (
          <span className="text-emerald-300">已经到了</span>
        ) : goal.remaining_exp == null ? (
          <span className="text-slate-500">读到等级后显示还差多少</span>
        ) : (
          <span className="text-slate-400 tabular-nums">
            还差 <span className="text-slate-200">{joinBig(goal.remaining_exp)}</span> 经验
            {goal.eta_secs != null && !rateStale ? (
              <>
                {" · "}按本段时速约{" "}
                <span className="text-amber-300 font-bold">{formatEtaCompact(goal.eta_secs)}</span>
              </>
            ) : (
              <span className="text-slate-500"> · 开始统计后显示还要多久</span>
            )}
          </span>
        )}
        <button
          type="button"
          onClick={startEditing}
          className="text-[11px] text-slate-500 underline underline-offset-2 hover:text-slate-300 cursor-pointer"
        >
          改
        </button>
        <button
          type="button"
          disabled={busy}
          onClick={() => save(null)}
          title="不要这个目标了"
          aria-label="清除练级目标"
          className="p-0.5 rounded text-slate-500 hover:text-slate-200 hover:bg-white/10 disabled:opacity-50 cursor-pointer"
        >
          <X className="w-3 h-3" />
        </button>
      </div>
      {error && <div className="text-[11px] text-red-300">{error}</div>}
    </div>
  );
};
