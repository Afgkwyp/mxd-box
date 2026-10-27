import React, { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Plus, Timer, Trash2, Volume2 } from "lucide-react";
import { Switch } from "./ui/kit";
import { BuffCropDialog } from "./BuffCropDialog";
import { BuffSkillPicker } from "./BuffSkillPicker";
import { useBuffStatus } from "../lib/buffStatus";
import { cn } from "../lib/utils";
import type { BuffConfig, BuffItem, BuffItemStatus } from "../types";

/** 一个 BUFF 现在的样子，写成一句短话。 */
function stateText(status: BuffItemStatus | undefined, running: boolean): string {
  if (!running || !status) return "—";
  if (status.presence === "missing") return "没在身上";
  if (status.presence === "unknown") return "找图标中";
  if (status.minutes != null) return `剩 ${status.minutes} 分多`;
  if (status.seconds == null) return "在身上";
  return `剩 ${status.seconds} 秒${status.established ? "" : "（核对中）"}`;
}

/**
 * BUFF 到期提醒：盯着游戏画面顶部的 BUFF 图标，快结束时念一声「XX 即将结束」。
 *
 * 规则在后端（`src-tauri/src/buff/`，照网页版「枫记」移植）：认图标、读左下角的秒数、
 * 多读几次把结束时刻钉准了才提醒，一次放技能只提醒一次。
 * 和这一页别的开关一样**改完立刻生效**。
 */
export const BuffAlertCard: React.FC = () => {
  const [config, setConfig] = useState<BuffConfig | null>(null);
  const [error, setError] = useState("");
  /** 正在加：从技能图标里挑 / 在截图上框 */
  const [adding, setAdding] = useState<"skill" | "crop" | null>(null);
  const status = useBuffStatus();

  useEffect(() => {
    invoke<BuffConfig>("get_buff_config")
      .then(setConfig)
      .catch((err: unknown) => setError(`读取 BUFF 提醒设置失败：${err}`));
  }, []);

  const save = (next: BuffConfig) => {
    setError("");
    setConfig(next);
    invoke<BuffConfig>("save_buff_config", { config: next })
      .then(setConfig)
      .catch((err: unknown) => setError(typeof err === "string" ? err : "没保存上，再试一次"));
  };

  const updateItem = (id: string, patch: Partial<BuffItem>) => {
    if (!config) return;
    save({ ...config, items: config.items.map((item) => (item.id === id ? { ...item, ...patch } : item)) });
  };

  const remove = (item: BuffItem) => {
    if (!window.confirm(`删掉「${item.name}」？`)) return;
    invoke<BuffConfig>("delete_buff", { id: item.id })
      .then(setConfig)
      .catch((err: unknown) => setError(String(err)));
  };

  const testSpeech = () => {
    if (!config) return;
    const name = config.items[0]?.name ?? "圣盾";
    invoke("test_buff_speech", { text: `${name}即将结束`, volume: config.volume, rate: config.rate });
  };

  const input =
    "w-14 bg-slate-950/60 border border-white/10 rounded-lg px-2 py-1 text-xs text-slate-100 focus:border-amber-500/40 outline-none tabular-nums";

  return (
    <div className="p-5 rounded-2xl bg-slate-900/70 border border-white/10 space-y-4">
      <div className="flex items-center justify-between gap-3">
        <div className="flex items-center gap-2 text-sm font-bold text-slate-200">
          <Timer className="w-4 h-4 text-amber-400" />
          BUFF 到期提醒
        </div>
        <label className="flex items-center gap-2">
          <span className="text-[11px] text-slate-400">
            {config == null ? "读取中" : config.enabled ? (status?.note ?? "启动中") : "已关闭"}
          </span>
          <Switch
            checked={config?.enabled ?? false}
            disabled={config == null}
            onChange={(enabled) => config && save({ ...config, enabled })}
            label="BUFF 到期提醒"
          />
        </label>
      </div>

      <p className="text-xs text-slate-400 leading-relaxed">
        认出游戏画面顶部的 BUFF 图标，读它左下角的倒计时，剩到你设的秒数时念一声「XX 即将结束」。
        点「添加 BUFF」挑技能；技能图鉴里没有的（药水之类），可以在游戏截图上框住它的图标。
      </p>

      {config && config.items.length > 0 && (
        <div className="space-y-2">
          {config.items.map((item) => {
            const live = status?.items.find((s) => s.id === item.id);
            return (
              <div
                key={item.id}
                className={cn(
                  "flex flex-wrap items-center gap-3 rounded-xl bg-slate-800/60 border px-3 py-2 text-xs transition-colors",
                  live?.nearing ? "border-amber-500/60" : "border-white/5",
                  !item.enabled && "opacity-60",
                )}
              >
                <img
                  src={`data:image/png;base64,${item.icon_png}`}
                  alt=""
                  className="w-8 h-8 rounded border border-white/10"
                  style={{ imageRendering: "pixelated" }}
                />
                <input
                  defaultValue={item.name}
                  maxLength={20}
                  onBlur={(event) => {
                    const name = event.target.value.trim();
                    if (name && name !== item.name) updateItem(item.id, { name });
                    else event.target.value = item.name;
                  }}
                  className="w-24 bg-transparent border-b border-transparent hover:border-white/10 focus:border-amber-500/40 outline-none text-slate-100 font-semibold"
                  aria-label="BUFF 名字"
                />
                <span
                  className={cn(
                    "w-28 tabular-nums",
                    live?.nearing ? "text-amber-400 font-bold" : "text-slate-400",
                  )}
                >
                  {stateText(live, (status?.running ?? false) && item.enabled)}
                </span>
                <label className="flex items-center gap-1.5 text-slate-400">
                  剩
                  <input
                    type="number"
                    min={1}
                    max={59}
                    defaultValue={item.threshold}
                    onBlur={(event) => {
                      const value = Math.round(Number(event.target.value));
                      if (value >= 1 && value <= 59 && value !== item.threshold) {
                        updateItem(item.id, { threshold: value });
                      } else {
                        event.target.value = String(item.threshold);
                      }
                    }}
                    className={input}
                  />
                  秒提醒
                </label>
                <label className="flex items-center gap-1.5 text-slate-400" title="关掉就只响提示音">
                  念出来
                  <Switch
                    checked={item.speech}
                    onChange={(speech) => updateItem(item.id, { speech })}
                    label="念出来"
                  />
                </label>
                <label className="flex items-center gap-1.5 text-slate-400" title="图标消失时再提醒一次「已结束」">
                  结束也提醒
                  <Switch
                    checked={item.expire_alert}
                    onChange={(expire_alert) => updateItem(item.id, { expire_alert })}
                    label="结束也提醒"
                  />
                </label>
                <div className="ml-auto flex items-center gap-2">
                  <Switch
                    checked={item.enabled}
                    onChange={(enabled) => updateItem(item.id, { enabled })}
                    label={`启用 ${item.name}`}
                  />
                  <button
                    type="button"
                    onClick={() => remove(item)}
                    title="删除"
                    aria-label={`删除 ${item.name}`}
                    className="p-1 rounded-lg text-slate-500 hover:text-red-300 hover:bg-slate-800 cursor-pointer"
                  >
                    <Trash2 className="w-3.5 h-3.5" />
                  </button>
                </div>
              </div>
            );
          })}
        </div>
      )}

      <div className="flex flex-wrap items-center gap-3 text-xs">
        <button
          type="button"
          onClick={() => setAdding("skill")}
          disabled={config == null}
          className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-amber-500 text-slate-950 font-bold cursor-pointer disabled:opacity-40"
        >
          <Plus className="w-3.5 h-3.5" />
          添加 BUFF
        </button>
        {config && (
          <>
            <label className="flex items-center gap-2 text-slate-400 ml-2">
              音量
              <input
                type="range"
                min={0}
                max={100}
                step={5}
                value={config.volume}
                onChange={(event) => setConfig({ ...config, volume: Number(event.target.value) })}
                onPointerUp={() => save(config)}
                onKeyUp={() => save(config)}
                className="w-24 accent-amber-500"
              />
            </label>
            <label className="flex items-center gap-2 text-slate-400">
              语速
              <input
                type="range"
                min={-5}
                max={8}
                value={config.rate}
                onChange={(event) => setConfig({ ...config, rate: Number(event.target.value) })}
                onPointerUp={() => save(config)}
                onKeyUp={() => save(config)}
                className="w-24 accent-amber-500"
              />
            </label>
            <button
              type="button"
              onClick={testSpeech}
              className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-slate-800 border border-white/10 text-slate-200 hover:border-amber-500/40 cursor-pointer"
            >
              <Volume2 className="w-3.5 h-3.5" />
              试听
            </button>
          </>
        )}
      </div>

      {error && <div className="text-[11px] text-red-300">{error}</div>}

      <p className="text-[11px] text-slate-500 leading-relaxed">
        只看游戏画面上面 30%（BUFF 栏的位置），游戏最小化时暂停。秒数要连着读对几次、把结束时刻核对准了才会提醒，
        一次放技能只提醒一次；重新放技能会自动算新的一轮。念用的是 Windows 自带的语音，系统里没有中文语音时改响提示音。
        识别做法参考了网页版「枫记」（fj.need.run）的 BUFF 提醒；技能图标来自「冒险岛小册子」的技能图鉴。
        换分辨率不用重新添加：会自动量出游戏把界面放大了几倍（经验统计认到状态栏时最快）。
      </p>

      {adding === "skill" && (
        <BuffSkillPicker
          added={(config?.items ?? []).flatMap((item) => (item.skill_id ? [item.skill_id] : []))}
          onClose={() => setAdding(null)}
          onAdded={setConfig}
          onCrop={() => setAdding("crop")}
        />
      )}
      {adding === "crop" && <BuffCropDialog onClose={() => setAdding(null)} onAdded={setConfig} />}
    </div>
  );
};
