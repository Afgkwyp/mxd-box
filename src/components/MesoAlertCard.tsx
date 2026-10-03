import React, { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Coins } from "lucide-react";
import { Switch } from "./ui/kit";
import { usePresetServer } from "../lib/presetServer";
import { useMesoReport } from "../lib/mesoStore";
import type { MesoAlertConfig } from "../types";

/** 输入框里的字 → 数；空着 = 不盯这个方向；填得不像数返回 undefined（不保存）。 */
function parseRate(text: string): number | null | undefined {
  const trimmed = text.trim();
  if (trimmed === "") return null;
  const value = Number(trimmed);
  return Number.isFinite(value) && value > 0 ? value : undefined;
}

const rateText = (value: number | null) => (value == null ? "" : String(value));

/**
 * 金价到价提醒：预设区服的「1 元能换多少万金」涨到 / 跌到你填的数时提醒一次。
 *
 * 规则在后端（`src-tauri/src/meso_alert.rs`）：开着才每 10 分钟看一眼报价；只在价格
 * **跨过去的那一下**响，退回去超过 1% 才重新上膛；改完设置立刻查一次。
 *
 * 和这一页别的开关一样**改完立刻生效**：开关点了就存，数字在离开输入框 / 按回车时存。
 */
export const MesoAlertCard: React.FC = () => {
  const [config, setConfig] = useState<MesoAlertConfig | null>(null);
  const [aboveText, setAboveText] = useState("");
  const [belowText, setBelowText] = useState("");
  const [error, setError] = useState("");
  const { serverId, servers } = usePresetServer();
  const { report } = useMesoReport();

  const serverName = servers.find((server) => server.id === serverId)?.name ?? "预设区服";
  const current = report?.latest.find((item) => item.server_name === serverName)?.wan_rate ?? null;

  useEffect(() => {
    invoke<MesoAlertConfig>("get_meso_alert")
      .then((loaded) => {
        setConfig(loaded);
        setAboveText(rateText(loaded.above));
        setBelowText(rateText(loaded.below));
      })
      .catch((err: unknown) => setError(`读取金价提醒设置失败：${err}`));
  }, []);

  const save = (next: MesoAlertConfig) => {
    setError("");
    invoke<MesoAlertConfig>("set_meso_alert", { config: next })
      .then(setConfig)
      .catch((err: unknown) => setError(typeof err === "string" ? err : "没保存上，再试一次"));
  };

  /** 把两个输入框里现在的字存下去；有一个填得不像数就先不存，并说明白。 */
  const commit = (enabled: boolean) => {
    if (!config) return;
    const above = parseRate(aboveText);
    const below = parseRate(belowText);
    if (above === undefined || below === undefined) {
      setError("要填一个大于 0 的数，比如 8.5；不想盯这个方向就留空");
      return;
    }
    if (above === config.above && below === config.below && enabled === config.enabled) return;
    save({ enabled, above, below });
  };

  const onKeyDown = (event: React.KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Enter" && !event.nativeEvent.isComposing) event.currentTarget.blur();
  };

  const input =
    "w-24 bg-slate-950/60 border border-white/10 rounded-lg px-2.5 py-1.5 text-xs text-slate-100 placeholder:text-slate-600 focus:border-amber-500/40 outline-none tabular-nums disabled:opacity-50";

  return (
    <div className="p-5 rounded-2xl bg-slate-900/70 border border-white/10 space-y-4">
      <div className="flex items-center justify-between gap-3">
        <div className="flex items-center gap-2 text-sm font-bold text-slate-200">
          <Coins className="w-4 h-4 text-amber-400" />
          金价到价提醒
        </div>
        <label className="flex items-center gap-2">
          <span className="text-[11px] text-slate-400">
            {config == null ? "读取中" : config.enabled ? "每 10 分钟看一次报价" : "已关闭"}
          </span>
          <Switch
            checked={config?.enabled ?? false}
            disabled={config == null}
            onChange={(value) => commit(value)}
            label="金价到价提醒"
          />
        </label>
      </div>

      <p className="text-xs text-slate-400 leading-relaxed">
        盯的是<b className="text-slate-200">{serverName}</b>（跟着右上角的预设区服走）的「1 元能换多少万金」
        {current != null && (
          <>
            ，现在是 <b className="text-amber-400 tabular-nums">{current}</b>
          </>
        )}
        。这个数越大，金币越便宜。
      </p>

      <div className="grid grid-cols-1 sm:grid-cols-2 gap-3 text-xs">
        <label className="flex items-center gap-2 rounded-xl bg-slate-800/60 border border-white/5 px-3 py-2.5">
          <span className="text-slate-300 whitespace-nowrap">涨到</span>
          <input
            inputMode="decimal"
            value={aboveText}
            disabled={config == null}
            onChange={(event) => setAboveText(event.target.value)}
            onBlur={() => commit(config?.enabled ?? false)}
            onKeyDown={onKeyDown}
            placeholder="不盯"
            className={input}
          />
          <span className="text-slate-500">万金或以上时提醒（适合等着买金）</span>
        </label>
        <label className="flex items-center gap-2 rounded-xl bg-slate-800/60 border border-white/5 px-3 py-2.5">
          <span className="text-slate-300 whitespace-nowrap">跌到</span>
          <input
            inputMode="decimal"
            value={belowText}
            disabled={config == null}
            onChange={(event) => setBelowText(event.target.value)}
            onBlur={() => commit(config?.enabled ?? false)}
            onKeyDown={onKeyDown}
            placeholder="不盯"
            className={input}
          />
          <span className="text-slate-500">万金或以下时提醒（适合等着卖金）</span>
        </label>
      </div>

      {error && <div className="text-[11px] text-red-300">{error}</div>}

      <p className="text-[11px] text-slate-500 leading-relaxed">
        只在价格到价的那一下提醒一次；价格退回去超过 1% 之后再次到价才会再提醒。两个数都留空就不会提醒。
        已经满足的条件，开关一打开就会响。
      </p>
    </div>
  );
};
