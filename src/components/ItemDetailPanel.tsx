import React from "react";
import { AlertCircle, Coins, Loader2 } from "lucide-react";
import type { ItemDetail } from "../types";

interface Props {
  detail?: ItemDetail;
  loading: boolean;
  error: string;
}

/**
 * 物品详情面板：小册子道具图鉴里的官方资料。
 *
 * 装备就给基准属性（攻击力 / 防御力 / 需求等级 / 卷轴次数），
 * 一律带上**出售给 NPC 的价格**（图鉴页脚那句「出售价格：2,000 金币」）。
 */
export const ItemDetailPanel: React.FC<Props> = ({ detail, loading, error }) => {
  if (loading) {
    return (
      <div className="rounded-xl border border-white/10 bg-slate-950/60 px-3 py-2.5 text-xs text-slate-400 flex items-center gap-2">
        <Loader2 className="w-3.5 h-3.5 animate-spin" />
        正在读取小册子物品图鉴...
      </div>
    );
  }

  if (error) {
    return (
      <div className="rounded-xl border border-red-500/30 bg-red-950/40 px-3 py-2.5 text-xs text-red-300 flex items-start gap-2">
        <AlertCircle className="w-3.5 h-3.5 shrink-0 mt-0.5" />
        <span>{error}</span>
      </div>
    );
  }

  if (!detail) return null;

  return (
    <div className="rounded-xl border border-white/10 bg-slate-950/60 p-3 space-y-2.5 text-xs">
      <div className="flex items-center gap-2.5">
        {detail.icon_url && (
          <img
            src={detail.icon_url}
            alt={detail.name}
            className="w-9 h-9 shrink-0 rounded-lg bg-slate-900/80 p-0.5 border border-white/10 object-contain"
          />
        )}
        <div className="min-w-0">
          <div className="font-semibold text-slate-100 truncate">
            {detail.name}
            <span className="ml-2 text-[10px] font-normal text-slate-500">ID {detail.id}</span>
          </div>
          <div className="text-[10px] text-slate-500">
            {detail.equipment ? "装备" : "道具"}
            {detail.facts.length > 0 && <> · {detail.facts.map((fact) => fact.value).join(" · ")}</>}
          </div>
        </div>
      </div>

      {detail.reqs.length > 0 && (
        <div className="flex flex-wrap gap-x-3 gap-y-1">
          {detail.reqs.map((fact) => (
            <span key={fact.label} className="text-slate-500">
              {fact.label}
              <b className="ml-0.5 text-amber-300">{fact.value}</b>
            </span>
          ))}
        </div>
      )}

      {detail.props.length > 0 && (
        <div className="space-y-1">
          <div className="text-[10px] tracking-wide text-slate-500">官方基准属性</div>
          {detail.props.map((prop) => (
            <div
              key={prop.label}
              className="flex items-center justify-between gap-3 rounded bg-slate-900/70 px-2 py-1"
            >
              <span className="text-slate-300">{prop.label}</span>
              <span className="tabular-nums font-semibold text-emerald-300">
                {prop.value}
                {prop.range && (
                  <span className="ml-1 text-[10px] font-normal text-slate-500">{prop.range}</span>
                )}
              </span>
            </div>
          ))}
        </div>
      )}

      {detail.jobs.length > 0 && (
        <div className="text-[10px] text-slate-500">可装备职业：{detail.jobs.join(" / ")}</div>
      )}

      <div className="flex items-center justify-between gap-2 border-t border-white/5 pt-2">
        <span className="text-[10px] text-slate-500">
          {detail.upgrade || (detail.equipment ? "不可强化" : "非装备")}
        </span>
        <span className="flex items-center gap-1 font-medium text-amber-300">
          <Coins className="w-3.5 h-3.5" />
          {detail.sell_price || "售价未知"}
        </span>
      </div>

      <div className="text-[10px] text-slate-600">资料来自小册子道具图鉴（官方基准值）</div>
    </div>
  );
};
