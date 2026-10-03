import React, { useEffect, useRef, useState } from "react";
import { ArrowRight, Check, Copy, Info } from "lucide-react";
import type { MesoRate } from "../types";
import { Panel, SubTabs } from "./ui/kit";

/** 金币数的中文速读形式（万 / 亿） */
export function humanGold(gold: number): string {
  if (!Number.isFinite(gold)) return "--";
  if (gold >= 1e8) return `${trimNum(gold / 1e8, 2)} 亿`;
  if (gold >= 1e4) return `${trimNum(gold / 1e4, 2)} 万`;
  return String(Math.round(gold));
}

/** 去掉多余的小数尾零 */
function trimNum(value: number, digits: number): string {
  return value.toFixed(digits).replace(/\.?0+$/, "");
}

/** 大数带千位分隔，肉眼才能一眼读出量级 */
function group(value: number, digits = 0): string {
  if (!Number.isFinite(value)) return "--";
  return value.toLocaleString("zh-CN", { minimumFractionDigits: 0, maximumFractionDigits: digits });
}

type Mode = "yuan" | "gold";

const MODE_KEY = "mxdbox.converter.mode";

/**
 * 把输入框里的文字读成数字，支持中文习惯写法：
 * `100`、`1,000`、`5000万`、`1.5亿`、`2w`。读不出来返回 null。
 * 返回 `unit`：用了「万 / 亿」时给界面一句「= 50,000,000」的回显，
 * 免得用户不确定程序把「5000万」读成了多少。
 */
function parseAmount(raw: string): { value: number; unit: boolean } | null {
  const text = raw.replace(/[,，\s]/g, "");
  if (!text) return null;
  const match = /^(\d+(?:\.\d+)?|\.\d+)(亿|万|w|W)?$/.exec(text);
  if (!match) return null;
  const base = Number(match[1]);
  const mult = match[2] === "亿" ? 1e8 : match[2] ? 1e4 : 1;
  const value = base * mult;
  return Number.isFinite(value) ? { value, unit: !!match[2] } : null;
}

const PRESETS: Record<Mode, Array<{ label: string; raw: string }>> = {
  yuan: [
    { label: "10 元", raw: "10" },
    { label: "50 元", raw: "50" },
    { label: "100 元", raw: "100" },
    { label: "500 元", raw: "500" },
    { label: "1000 元", raw: "1000" },
  ],
  gold: [
    { label: "1000 万", raw: "1000万" },
    { label: "5000 万", raw: "5000万" },
    { label: "1 亿", raw: "1亿" },
    { label: "5 亿", raw: "5亿" },
    { label: "10 亿", raw: "10亿" },
  ],
};

/**
 * 元 ⇄ 金币 换算器（重做版）。
 *
 * 旧版是「左右两个都能输入的框」：读起来像单向换算器、又要解释「哪边在输入」，
 * 还只能输纯数字（输 1 亿要打 9 个 0）。现在改成最常见的计算器形式：
 *
 * * 上面一个**方向**切换（元 → 金币 / 金币 → 元），切换时保持「同一笔钱」；
 * * 一个大输入框，支持 `5000万` `1.5亿` 这类写法，下面回显读成了多少；
 * * 结果放大显示，一键复制；
 * * 下面同时列出**每个区服**的结果 —— 换算器最常见的用途其实是比价，不用一个个切区服；
 * * 预设金额一键填入。
 */
export const MesoConverter: React.FC<{
  rates: MesoRate[];
  /** 默认按哪个区服的金价（预设区服） */
  defaultServer?: string;
  className?: string;
}> = ({ rates, defaultServer, className }) => {
  const [mode, setMode] = useState<Mode>(() => {
    try {
      return window.localStorage.getItem(MODE_KEY) === "gold" ? "gold" : "yuan";
    } catch {
      return "yuan";
    }
  });
  const [raw, setRaw] = useState("100");
  /** null = 跟着预设区服走；用户手选过就用他选的 */
  const [picked, setPicked] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const copyTimer = useRef<number | undefined>(undefined);

  useEffect(() => () => window.clearTimeout(copyTimer.current), []);

  const serverName =
    (picked && rates.some((r) => r.server_name === picked) ? picked : null) ??
    (defaultServer && rates.some((r) => r.server_name === defaultServer) ? defaultServer : null) ??
    rates[0]?.server_name ??
    "";
  const rate = rates.find((r) => r.server_name === serverName);

  const goldPerYuanOf = (r: MesoRate | undefined) => {
    const wan = Number(r?.wan_rate ?? "");
    return Number.isFinite(wan) && wan > 0 ? wan * 10000 : 0;
  };
  const goldPerYuan = goldPerYuanOf(rate);
  const hasRate = goldPerYuan > 0;

  const parsed = parseAmount(raw);
  const amount = parsed?.value ?? null;

  const result = amount == null || !hasRate ? null : mode === "yuan" ? amount * goldPerYuan : amount / goldPerYuan;

  /** 结果的两种写法：大字（速读）+ 完整数字 */
  const resultBig =
    result == null ? "—" : mode === "yuan" ? `${humanGold(result)} 金币` : `${group(result, 2)} 元`;
  const resultFull =
    result == null ? "" : mode === "yuan" ? `${group(Math.round(result))} 金币` : `${trimNum(result, 2)} 元`;

  /** 切方向：把当前结果带过去，「同一笔钱」换个单位继续看（100 元 ⇄ 125 万金币）。 */
  const changeMode = (next: Mode) => {
    if (next === mode) return;
    if (result != null && result > 0) {
      setRaw(next === "yuan" ? String(Number(result.toFixed(2))) : String(Math.round(result)));
    }
    setMode(next);
    try {
      window.localStorage.setItem(MODE_KEY, next);
    } catch {
      /* 记不住方向也不影响使用 */
    }
  };

  const copy = async () => {
    if (result == null) return;
    const text = mode === "yuan" ? String(Math.round(result)) : trimNum(result, 2);
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      window.clearTimeout(copyTimer.current);
      copyTimer.current = window.setTimeout(() => setCopied(false), 1400);
    } catch {
      /* 剪贴板不可用就算了：结果就在屏幕上，手选也能复制 */
    }
  };

  // 各区服的结果（比价）
  const rows = rates.map((r) => {
    const g = goldPerYuanOf(r);
    const value = amount != null && g > 0 ? (mode === "yuan" ? amount * g : amount / g) : null;
    return { name: r.server_name, value };
  });
  const maxValue = Math.max(0, ...rows.map((row) => row.value ?? 0));

  return (
    <Panel
      className={className}
      title="换算器"
      meta={
        <SubTabs
          tabs={[
            { key: "yuan", label: "元 → 金币" },
            { key: "gold", label: "金币 → 元" },
          ]}
          value={mode}
          onChange={changeMode}
        />
      }
    >
      {!hasRate ? (
        <div className="p-3 rounded-xl bg-amber-500/10 border border-amber-500/30 text-amber-300 text-xs flex items-center gap-2">
          <Info className="w-4 h-4 shrink-0" />
          还没取到金价，暂时无法换算。
        </div>
      ) : (
        <div className="space-y-4">
          {/* 输入 + 结果 */}
          <div className="grid grid-cols-1 gap-1.5">
            <label className="block rounded-2xl border border-white/10 bg-slate-800/50 px-4 py-3 focus-within:border-amber-500/60 transition-colors">
              <span className="text-[11px] text-slate-500">{mode === "yuan" ? "我有（人民币）" : "我有（金币）"}</span>
              <span className="flex items-baseline gap-2">
                <input
                  type="text"
                  inputMode="decimal"
                  value={raw}
                  onChange={(e) => setRaw(e.target.value)}
                  placeholder={mode === "yuan" ? "如 100" : "如 5000万 / 1.5亿"}
                  aria-label={mode === "yuan" ? "人民币金额" : "金币数量"}
                  className="w-full min-w-0 bg-transparent text-[28px] font-extrabold tabular-nums text-amber-400 placeholder:text-slate-600 placeholder:text-base placeholder:font-normal focus:outline-none select-text"
                />
                <span className="text-sm text-slate-400 shrink-0">{mode === "yuan" ? "元" : "金币"}</span>
              </span>
              <span className="block text-[11px] h-4 mt-0.5 text-slate-500 tabular-nums">
                {raw.trim() === ""
                  ? "输入金额"
                  : parsed == null
                    ? <span className="text-red-400">读不懂这个数字（可写 5000万、1.5亿）</span>
                    : parsed.unit
                      ? `= ${group(parsed.value)}`
                      : ""}
              </span>
            </label>

            <div className="flex items-center justify-center text-slate-500">
              <ArrowRight className="w-5 h-5 rotate-90" />
            </div>

            <div className="rounded-2xl border border-amber-500/25 bg-amber-500/10 px-4 py-3 flex flex-col">
              <span className="text-[11px] text-amber-300/80">
                {mode === "yuan" ? "≈ 能换" : "≈ 值"}（按 {serverName}）
              </span>
              <span className="flex items-center gap-2 mt-0.5">
                <span className="text-[28px] font-extrabold tabular-nums text-slate-100 truncate" title={resultFull}>
                  {resultBig}
                </span>
                <button
                  type="button"
                  onClick={copy}
                  disabled={result == null}
                  title="复制结果数字"
                  aria-label="复制结果"
                  className="ml-auto shrink-0 p-1.5 rounded-lg bg-slate-800/80 hover:bg-slate-700 text-slate-300 hover:text-amber-300 disabled:opacity-40 cursor-pointer"
                >
                  {copied ? <Check className="w-4 h-4 text-emerald-400" /> : <Copy className="w-4 h-4" />}
                </button>
              </span>
              <span className="text-[11px] text-slate-500 h-4 mt-0.5 tabular-nums">
                {mode === "yuan" && result != null ? resultFull : `1 元 = ${rate?.wan_rate} 万金币`}
              </span>
            </div>
          </div>

          {/* 预设金额 */}
          <div className="flex flex-wrap items-center gap-1.5">
            <span className="text-[11px] text-slate-500 mr-1">快捷：</span>
            {PRESETS[mode].map((preset) => (
              <button
                key={preset.label}
                type="button"
                onClick={() => setRaw(preset.raw)}
                className={`px-2.5 py-1 rounded-lg border text-xs cursor-pointer ${
                  raw === preset.raw
                    ? "bg-amber-500/15 border-amber-500/40 text-amber-300"
                    : "bg-slate-800/70 border-white/10 text-slate-300 hover:text-amber-300"
                }`}
              >
                {preset.label}
              </button>
            ))}
          </div>

          {/* 各区服结果：点一行就按那个区服的金价算 */}
          <div>
            <div className="text-[11px] text-slate-500 mb-1.5">
              同样的{mode === "yuan" ? "钱" : "金币"}，各区服{mode === "yuan" ? "能换" : "值"}多少（点一行按该服计算）
            </div>
            <div className="space-y-1">
              {rows.map((row) => {
                const on = row.name === serverName;
                const width = row.value != null && maxValue > 0 ? (row.value / maxValue) * 100 : 0;
                return (
                  <button
                    key={row.name}
                    type="button"
                    onClick={() => setPicked(row.name)}
                    className={`relative w-full overflow-hidden flex items-center justify-between gap-3 rounded-lg px-3 py-1.5 text-xs cursor-pointer ${
                      on ? "ring-1 ring-amber-500/50" : "hover:bg-white/5"
                    }`}
                  >
                    <span
                      aria-hidden
                      className={`absolute inset-y-0 left-0 transition-[width] duration-500 ease-out ${on ? "bg-amber-500/20" : "bg-slate-700/40"}`}
                      style={{ width: `${width}%` }}
                    />
                    <span className={`relative ${on ? "text-amber-300 font-bold" : "text-slate-300"}`}>{row.name}</span>
                    <span className="relative tabular-nums text-slate-100 font-medium">
                      {row.value == null
                        ? "—"
                        : mode === "yuan"
                          ? `${humanGold(row.value)} 金币`
                          : `${group(row.value, 2)} 元`}
                    </span>
                  </button>
                );
              })}
            </div>
          </div>
        </div>
      )}
    </Panel>
  );
};
