import React, { useMemo, useState } from "react";
import { RefreshCw, Activity, ShieldCheck, Info } from "lucide-react";
import {
  ResponsiveContainer,
  XAxis,
  YAxis,
  Tooltip,
  CartesianGrid,
  Line,
  LineChart,
} from "recharts";
import type { MesoRate } from "../types";
import { useTheme } from "../lib/theme";
import { useMesoReport } from "../lib/mesoStore";
import { usePresetServer } from "../lib/presetServer";
import { useTween } from "../lib/useTween";
import { MesoConverter } from "./MesoConverter";
import { Panel, SubTabs } from "./ui/kit";

/** 与站点区服表一一对应，保证折线颜色稳定 */
const SERVER_COLORS = ["#f59e0b", "#10b981", "#3b82f6", "#ec4899", "#8b5cf6"];

/** 金价数字：变化时滚过去，首次从 0 滚起。 */
const RateNumber: React.FC<{ value: string; digits?: number }> = ({ value, digits = 2 }) => {
  const n = Number(value);
  const tween = useTween(Number.isFinite(n) ? n : null, { fromZero: true, duration: 700 });
  return <>{tween != null && Number.isFinite(n) ? tween.toFixed(digits) : value}</>;
};

/** 折线（铺满宽度）。`fill` 时下面铺一层渐变。 */
const Spark: React.FC<{
  values: number[];
  color: string;
  height?: number;
  fill?: boolean;
  id?: string;
  /** 高度随容器伸缩（首屏当前区服卡里用，填满卡片中间的空白） */
  fluid?: boolean;
}> = ({ values, color, height = 32, fill = false, id = "spark", fluid = false }) => {
  if (values.length < 2) return <div style={{ height }} />;
  const min = Math.min(...values);
  const span = Math.max(...values) - min || 1;
  const w = 240;
  const pts = values.map(
    (v, i) =>
      `${i ? "L" : "M"}${((i / (values.length - 1)) * w).toFixed(1)},${(height - 3 - ((v - min) / span) * (height - 8)).toFixed(1)}`,
  );
  const line = pts.join(" ");
  return (
    <svg viewBox={`0 0 ${w} ${height}`} width="100%" height={fluid ? "100%" : height} preserveAspectRatio="none" className="block">
      {fill && (
        <>
          <defs>
            <linearGradient id={id} x1="0" y1="0" x2="0" y2="1">
              <stop offset="0" stopColor={color} stopOpacity="0.3" />
              <stop offset="1" stopColor={color} stopOpacity="0" />
            </linearGradient>
          </defs>
          <path d={`${line} L${w},${height} L0,${height}Z`} fill={`url(#${id})`} />
        </>
      )}
      <path d={line} fill="none" stroke={color} strokeWidth="2" strokeLinejoin="round" vectorEffect="non-scaling-stroke" />
    </svg>
  );
};

/** 涨跌小标：▲ 0.24（近 24 个采样点）。变化小于 0.005 不显示（横盘画箭头就是噪音）。 */
const Delta: React.FC<{ delta: number | null }> = ({ delta }) => {
  if (delta == null || Math.abs(delta) < 0.005) return <span className="text-slate-600">—</span>;
  return (
    <span className={`font-bold tabular-nums ${delta > 0 ? "text-emerald-400" : "text-red-400"}`}>
      {delta > 0 ? "▲" : "▼"} {Math.abs(delta).toFixed(2)}
    </span>
  );
};

export const MesoTrends: React.FC = () => {
  const { report, loading, error, updatedAt, refresh } = useMesoReport();
  const { serverId, servers } = usePresetServer();
  const [granularity, setGranularity] = useState<"hour" | "day">("hour");
  /** 走势图里被点灭的区服（默认全显示） */
  const [hidden, setHidden] = useState<Set<string>>(new Set());
  const [theme] = useTheme();

  const rates: MesoRate[] = useMemo(() => report?.latest ?? [], [report]);
  const series = useMemo(() => report?.history.series ?? [], [report]);
  const points = granularity === "hour" ? (report?.history.hour ?? []) : (report?.history.day ?? []);
  const currentName = servers.find((server) => server.id === serverId)?.name ?? rates[0]?.server_name ?? "";
  const current = rates.find((rate) => rate.server_name === currentName);
  const colorOf = (name: string) => SERVER_COLORS[Math.max(0, series.indexOf(name)) % SERVER_COLORS.length];

  /** 每个区服最近 24 个小时采样点（表格 / 卡片的迷你走势 + 涨跌都从它算） */
  const recent = useMemo(() => {
    const hour = report?.history.hour ?? [];
    const map: Record<string, number[]> = {};
    for (const name of series) {
      map[name] = hour
        .map((point) => point.values[name])
        .filter((v): v is number => typeof v === "number")
        .slice(-24);
    }
    return map;
  }, [report, series]);
  const deltaOf = (name: string) => {
    const list = recent[name] ?? [];
    return list.length > 1 ? list[list.length - 1] - list[0] : null;
  };

  // 表格按「1 元能换多少万金」从多到少排：一眼看出哪个服的金最便宜
  const ranked = useMemo(() => [...rates].sort((a, b) => Number(b.wan_rate) - Number(a.wan_rate)), [rates]);
  const rateMax = Math.max(0, ...rates.map((rate) => Number(rate.wan_rate) || 0));

  // 走势图数据（每个时间点摊平成一行，区服名作为列）
  const trendData: Array<Record<string, string | number>> = points.map((p) => ({ at: p.at, ...p.values }));
  const shownSeries = series.filter((name) => !hidden.has(name));
  const trendValues = trendData
    .flatMap((row) => shownSeries.map((name) => row[name]))
    .filter((v): v is number => typeof v === "number");
  const trendMin = trendValues.length ? Math.min(...trendValues) : 0;
  const trendMax = trendValues.length ? Math.max(...trendValues) : 1;
  const pad = Math.max((trendMax - trendMin) * 0.15, 0.2);
  const yDomain: [number, number] = [Number((trendMin - pad).toFixed(3)), Number((trendMax + pad).toFixed(3))];

  const toggleSeries = (name: string) =>
    setHidden((prev) => {
      const next = new Set(prev);
      if (next.has(name)) next.delete(name);
      else if (next.size < series.length - 1) next.add(name); // 至少留一条线
      return next;
    });

  // recharts 的 SVG 属性拿不到 CSS 变量，所以按主题取具体颜色
  const chart =
    theme === "light"
      ? { grid: "#dccfb3", tick: "#5c4c39", tooltipBg: "#fffaf0", tooltipBorder: "#dccfb3", tooltipText: "#1f1810" }
      : { grid: "#3b3229", tick: "#b0a492", tooltipBg: "#1c1713", tooltipBorder: "#3b3229", tooltipText: "#fbf3e6" };

  return (
    <div className="space-y-4 min-w-0">
      {error && !report && (
        <div className="anim-fade p-3 rounded-xl bg-red-500/10 border border-red-500/30 text-red-300 text-xs flex items-center gap-2">
          <Info className="w-4 h-4 text-red-400 shrink-0" />
          <span>{error}</span>
        </div>
      )}

      {/* 第一行：当前区服 + 换算器 */}
      <div className="anim-stagger grid grid-cols-12 gap-4">
        <Panel
          className="col-span-12 lg:col-span-5 flex flex-col"
          title={
            <>
              {currentName || "金价"}
              <span className="text-[10px] px-2 py-0.5 rounded-full bg-amber-500/20 text-amber-300 font-normal">当前区服</span>
            </>
          }
          meta={
            <>
              <span className="tabular-nums">
                {updatedAt
                  ? new Date(updatedAt).toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit", hour12: false })
                  : loading
                    ? "读取中…"
                    : "—"}
              </span>
              <button
                type="button"
                title="立即刷新（后端 10 分钟内用缓存）"
                onClick={refresh}
                disabled={loading}
                className="p-1.5 rounded-lg bg-slate-800 hover:bg-slate-700 text-slate-300 hover:text-amber-300 disabled:opacity-40 cursor-pointer"
              >
                <RefreshCw className={`w-3.5 h-3.5 ${loading ? "animate-spin" : ""}`} />
              </button>
            </>
          }
        >
          {current ? (
            <div className="flex flex-col gap-3 flex-1">
              <div className="flex items-baseline gap-2">
                <span className="text-[56px] font-black leading-none text-amber-400 tabular-nums">
                  <RateNumber value={current.wan_rate} />
                </span>
                <span className="text-sm text-slate-400">万金 / 元</span>
                <span className="ml-auto text-sm">
                  <Delta delta={deltaOf(current.server_name)} />
                </span>
              </div>
              <div className="flex-1 min-h-[64px] max-h-[120px] my-auto">
                <Spark
                  values={recent[current.server_name] ?? []}
                  color={colorOf(current.server_name)}
                  height={64}
                  fill
                  fluid
                  id="hero-spark"
                />
              </div>
              <div className="grid grid-cols-2 gap-3 text-xs mt-auto">
                <div className="rounded-xl bg-slate-800/60 border border-white/5 px-3 py-2">
                  <div className="text-slate-500">1 万金 ≈</div>
                  <div className="text-base font-bold text-slate-100 tabular-nums">{current.yuan_rate} 元</div>
                </div>
                <div className="rounded-xl bg-slate-800/60 border border-white/5 px-3 py-2">
                  <div className="text-slate-500">在售库存</div>
                  <div className="text-base font-bold text-slate-100 tabular-nums">{current.stock || "充足"}</div>
                </div>
              </div>
            </div>
          ) : (
            <div className="py-10 text-center text-xs text-slate-500">{loading ? "读取中…" : "还没有金价数据"}</div>
          )}
        </Panel>

        <MesoConverter className="col-span-12 lg:col-span-7" rates={rates} defaultServer={currentName} />
      </div>

      {/* 各区服对比：一张表，按「1 元换的万金」从多到少排。只看不点 ——
          换预设区服统一在主窗口右上角，这里再放一个入口就重复了。 */}
      <Panel title="各区服对比" className="px-2 pb-2">
        <div className="hidden md:grid grid-cols-[1.1fr_1.4fr_0.9fr_0.9fr_1.2fr_0.8fr] gap-3 px-2 pb-2 text-[11px] text-slate-500">
          <span>区服</span>
          <span>1 元换（万金）</span>
          <span>1 万金≈（元）</span>
          <span>近 24 点</span>
          <span>走势</span>
          <span className="text-right">库存</span>
        </div>
        <div>
          {ranked.map((rate) => {
            const isCurrent = rate.server_name === currentName;
            const share = rateMax > 0 ? ((Number(rate.wan_rate) || 0) / rateMax) * 100 : 0;
            return (
              <div
                key={rate.server_name}
                aria-current={isCurrent}
                className={`w-full grid grid-cols-[1fr_auto] md:grid-cols-[1.1fr_1.4fr_0.9fr_0.9fr_1.2fr_0.8fr] gap-3 items-center rounded-xl px-2 py-2.5 text-left text-sm ${
                  isCurrent ? "bg-amber-500/10 ring-1 ring-amber-500/35" : ""
                }`}
              >
                <span className="flex items-center gap-2 font-semibold min-w-0">
                  <span className="w-2.5 h-2.5 rounded-full shrink-0" style={{ background: colorOf(rate.server_name) }} />
                  <span className="truncate">{rate.server_name}</span>
                  {isCurrent && (
                    <span className="text-[10px] px-1.5 py-px rounded-full bg-amber-500/20 text-amber-300 font-normal shrink-0">预设</span>
                  )}
                </span>
                <span className="flex items-center gap-2 min-w-0">
                  <span className="font-extrabold tabular-nums text-amber-400 w-12 shrink-0">{rate.wan_rate}</span>
                  <span className="hidden md:block flex-1 h-1.5 rounded-full bg-slate-800 overflow-hidden">
                    <span
                      className="block h-full rounded-full transition-[width] duration-700 ease-out"
                      style={{ width: `${share}%`, background: colorOf(rate.server_name) }}
                    />
                  </span>
                </span>
                <span className="hidden md:block tabular-nums text-slate-300">{rate.yuan_rate}</span>
                <span className="hidden md:block text-xs">
                  <Delta delta={deltaOf(rate.server_name)} />
                </span>
                <span className="hidden md:block">
                  <Spark values={recent[rate.server_name] ?? []} color={colorOf(rate.server_name)} height={26} />
                </span>
                <span className="hidden md:block text-right text-xs tabular-nums text-slate-400">{rate.stock || "充足"}</span>
              </div>
            );
          })}
        </div>
      </Panel>

      {/* 走势图 */}
      <Panel
        className="space-y-3"
        title={
          <>
            <Activity className="w-4 h-4 text-amber-400 shrink-0" />
            金价走势
            <span className="text-xs font-normal text-slate-500">1 元可兑换万金</span>
          </>
        }
        meta={
          <>
            <span className="text-xs text-slate-500">{points.length ? `${points.length} 个采样点` : "暂无快照"}</span>
            <SubTabs
              tabs={[
                { key: "hour", label: "按小时" },
                { key: "day", label: "按天" },
              ]}
              value={granularity}
              onChange={setGranularity}
            />
          </>
        }
      >
        {/* 区服开关：点一下显示 / 隐藏那条线（当前区服的线更粗） */}
        <div className="flex flex-wrap gap-1.5">
          {series.map((name) => {
            const off = hidden.has(name);
            return (
              <button
                key={name}
                type="button"
                onClick={() => toggleSeries(name)}
                aria-pressed={!off}
                className={`inline-flex items-center gap-1.5 px-2.5 py-1 rounded-full border text-xs cursor-pointer ${
                  off ? "border-white/10 text-slate-500" : "border-white/20 text-slate-100 bg-slate-800/70"
                }`}
              >
                <span
                  className="w-2 h-2 rounded-full"
                  style={{ background: off ? "transparent" : colorOf(name), border: `1.5px solid ${colorOf(name)}` }}
                />
                {name}
              </button>
            );
          })}
        </div>

        {trendData.length === 0 ? (
          <p className="py-10 text-center text-xs text-slate-500">站点暂未提供历史快照，稍等片刻会自动刷新。</p>
        ) : (
          <div className="h-72 w-full min-w-0">
            <ResponsiveContainer width="100%" height="100%">
              <LineChart data={trendData} margin={{ top: 8, right: 16, left: -18, bottom: 0 }}>
                <CartesianGrid stroke={chart.grid} vertical={false} />
                <XAxis dataKey="at" tick={{ fill: chart.tick, fontSize: 11 }} stroke={chart.grid} tickLine={false} minTickGap={28} />
                <YAxis tick={{ fill: chart.tick, fontSize: 11 }} stroke={chart.grid} tickLine={false} domain={yDomain} width={56} />
                <Tooltip
                  contentStyle={{
                    backgroundColor: chart.tooltipBg,
                    border: `1px solid ${chart.tooltipBorder}`,
                    borderRadius: "10px",
                    fontSize: "12px",
                  }}
                  labelStyle={{ color: chart.tooltipText }}
                  itemStyle={{ color: chart.tooltipText }}
                  formatter={(val: unknown, name: unknown) => [`${val} 万金`, String(name)]}
                />
                {shownSeries.map((name) => (
                  <Line
                    key={name}
                    type="monotone"
                    dataKey={name}
                    name={name}
                    stroke={colorOf(name)}
                    strokeWidth={name === currentName ? 3 : 1.8}
                    strokeOpacity={name === currentName ? 1 : 0.75}
                    dot={false}
                    connectNulls
                  />
                ))}
              </LineChart>
            </ResponsiveContainer>
          </div>
        )}
      </Panel>

      {/* 防骗提示 */}
      <div className="p-4 rounded-2xl bg-amber-500/5 border border-amber-500/20 text-xs text-amber-200/90 leading-relaxed flex items-start gap-2.5">
        <ShieldCheck className="w-4 h-4 text-amber-400 shrink-0 mt-0.5" />
        <div>
          <span className="font-semibold text-amber-300">防骗提示：</span>
          线下交易有盗号 / 黑金封禁风险，请走官方担保，并用「查询 › 查人」核对交易对象。
        </div>
      </div>
    </div>
  );
};
