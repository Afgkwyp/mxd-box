import React, { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { COOLDOWN, guard } from "../lib/throttle";
import { useLatestRequest } from "../lib/latest";
import {
  AlertTriangle,
  Boxes,
  ChevronDown,
  ChevronUp,
  Coins,
  ExternalLink,
  Loader2,
  MapPin,
  Package,
  Search,
  Skull,
} from "lucide-react";
import type { DropEntry, DropMobResult, DropSearchResult } from "../types";

/** 一屏里默认只展开这么多掉落行；剩下的点「展开全部」。 */
const DROPS_PREVIEW = 6;
/** 地图同理：一排地图名很容易把卡片撑得很长。 */
const MAPS_PREVIEW = 6;

/**
 * 从物价页「谁掉」带一个词进来：`term` 是输入框里显示的（道具名），
 * `query` 是实际发给站点的词（用道具 ID 更准，见方案 5.1）；缺省两者相同。
 * `nonce` 变了才算新的一次。
 */
export interface DropSeed {
  term: string;
  query?: string;
  nonce: number;
}

/** 关键词示例：一个人不知道「能查什么」的时候，一行例子比一段说明管用。 */
const EXAMPLES = ["锅盖", "绿蘑菇", "海盗冒险家表彰状", "1110100"];

/**
 * 怪物掉落速查。
 *
 * 数据来自小册子的**公开**接口（`/api/drop-search.php`，不需要登录）——
 * 这一点对这个工具很关键：朋友拿到 exe、没登录小册子账号也能直接查。
 *
 * 支持的四种输入（站点自己的说法）：怪物名 / 道具名 / 怪物ID / 道具ID。
 * 所以「我想刷某个装备，找哪只怪掉」和「这只怪掉什么」是同一个框。
 *
 * 两个刻意的取舍：
 *
 * * **概率直接显示站点算好的那个串**（`chanceText`，例如 `0.007%`），
 *   不自己拿 `chance` 换算 —— 两个工具显示不同的概率是最没必要的困惑；
 * * **命中的那几条置顶并高亮**：一次查询往往返回几十条掉落（绿蘑菇有 34 条），
 *   用户想看的只是他查的那件东西。
 */
export const MobDrops: React.FC<{
  seed?: DropSeed;
  /** 点某条掉落的「查价」：把道具名交给查询页去切页签查价 */
  onSearchPrice?: (name: string) => void;
}> = ({ seed, onSearchPrice }) => {
  const [keyword, setKeyword] = useState("");
  const [result, setResult] = useState<DropSearchResult | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [page, setPage] = useState(1);
  /** 每只怪物分别控制「掉落全部 / 地图全部」的展开状态 */
  const [openDrops, setOpenDrops] = useState<Record<number, boolean>>({});
  const [openMaps, setOpenMaps] = useState<Record<number, boolean>>({});
  /** 只认最后一次查询的响应，慢的旧响应不许覆盖新结果 */
  const beginRequest = useLatestRequest();
  /**
   * 联动进来的查询是「输入框显示道具名、实际用道具 ID 查」：结果区文案和翻页
   * 都沿用这个名字（不露裸 ID）。普通搜索为 null，行为和以前一致。
   */
  const [displayTerm, setDisplayTerm] = useState<string | null>(null);

  const search = async (raw: string, nextPage = 1, display?: string, fromSeed = false) => {
    const target = raw.trim();
    if (!target) {
      setError("请输入怪物名、道具名或 ID");
      return;
    }
    // 查询中就别再发了：站点限流的风控是真实存在的，回车能绕过按钮的 disabled。
    // 联动进来的 seed 是用户明确点的一次查询，不能因为上一单还在跑就被 loading
    // 悄悄丢掉（那会停在「输入框是 B、结果是 A」，方案 5.4）—— 让它作废旧单、照发。
    if (loading && !fromSeed) return;
    const isCurrent = beginRequest();
    setLoading(true);
    setError("");
    try {
      // 掉落速查要打站点
      if (!guard("search_drops", "掉落速查", COOLDOWN.query)) return;
      const data = await invoke<DropSearchResult>("search_drops", {
        keyword: target,
        page: nextPage,
      });
      // 连着改词、或翻页与新搜索交叠时，慢的旧响应会晚于新响应到达，
      // 不属于最后一次请求就丢掉 —— 否则「搜了 A 显示 B」，翻页还会把
      // 旧词的第 2 页拼到新词结果后面。
      if (!isCurrent()) return;
      // 翻页时把新一页接在后面，而不是把前面的结果冲掉
      setResult((prev) =>
        nextPage > 1 && prev
          ? { ...data, results: [...prev.results, ...data.results] }
          : data
      );
      setKeyword(display ?? target);
      setDisplayTerm(display ?? null);
      setPage(nextPage);
      // 只在新搜索时清空展开状态：翻页（「加载更多」）时用户刚点开的那几只
      // 不该被收回去 —— 查价悬浮窗里就是保留的，两处行为要一致。
      if (nextPage === 1) {
        setOpenDrops({});
        setOpenMaps({});
      }
    } catch (err: unknown) {
      if (!isCurrent()) return;
      setError(typeof err === "string" ? err : "掉落查询失败");
      setResult(null);
    } finally {
      setLoading(false);
    }
  };

  /** 同一次联动 seed 只处理一次（同 MarketSearch：StrictMode 会把挂载 effect 跑两遍，第二遍会被 guard 拦下并作废第一遍） */
  const handledSeed = useRef(0);

  useEffect(() => {
    if (!seed?.term || handledSeed.current === seed.nonce) return;
    handledSeed.current = seed.nonce;
    setKeyword(seed.term);
    void search(seed.query ?? seed.term, 1, seed.term, true);
    // 只在「新的一次带词进来」时触发；search 每次渲染都是新函数，不能放进依赖
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [seed?.nonce]);

  const openPage = (url: string) => {
    if (!url) return;
    invoke("open_external_url", { url }).catch(() => {});
  };

  /** 命中的排前面：一次查询返回几十条掉落时，用户只想看他在找的那件。 */
  const sortedDrops = (drops: DropEntry[]): DropEntry[] =>
    [...drops].sort((a, b) => Number(b.matched) - Number(a.matched));

  const hasHits = (data: DropSearchResult) =>
    data.results.length > 0 || data.matches.items.length > 0 || data.matches.mobs.length > 0;

  /** 结果区文案里的词：联动进来查的是道具名（不露裸 ID），普通查询就是查询词本身 */
  const shownKeyword = displayTerm ?? result?.meta.keyword ?? "";

  return (
    <div className="space-y-5">
      {/* 搜索区 */}
      <div className="p-5 rounded-2xl bg-slate-900/70 border border-white/10 space-y-4">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <p className="text-xs text-slate-400 leading-relaxed">
            怪物名、道具名、怪物 ID、道具 ID 都能查，结果含掉落概率与出没地图。
          </p>
          <span className="text-[10px] px-2 py-0.5 rounded-full bg-emerald-500/15 text-emerald-300 border border-emerald-500/30">
            公开数据 · 不需要登录
          </span>
        </div>

        <div className="flex flex-col sm:flex-row gap-2">
          <div className="relative flex-1">
            <Search className="w-4 h-4 absolute left-3 top-1/2 -translate-y-1/2 text-slate-400 pointer-events-none" />
            <input
              type="text"
              value={keyword}
              onChange={(e) => setKeyword(e.target.value)}
              onKeyDown={(e) => {
                // `isComposing`：中文输入法选词的回车不是「提交查询」，
                // 不挡的话每选一次词都会发一次请求（主窗口查价页踩过）。
                // loading 由 search() 自己挡，回车在这里不用重复判断。
                if (e.key === "Enter" && !e.nativeEvent.isComposing) search(keyword);
              }}
              placeholder="输入怪物名 / 道具名 / ID，回车查询（例如：锅盖、绿蘑菇）"
              className="w-full bg-slate-800/80 border border-white/15 rounded-xl pl-9 pr-3 py-2.5 text-sm text-slate-100 placeholder:text-slate-500 focus:outline-none focus:border-amber-400/80"
            />
          </div>
          <button
            onClick={() => search(keyword)}
            disabled={loading}
            className="px-5 py-2.5 rounded-xl bg-amber-500 hover:bg-amber-400 text-slate-950 text-sm font-bold cursor-pointer disabled:opacity-50 flex items-center justify-center gap-2"
          >
            {loading ? <Loader2 className="w-4 h-4 animate-spin" /> : <Search className="w-4 h-4" />}
            {loading ? "查询中…" : "查掉落"}
          </button>
        </div>

        <div className="flex flex-wrap items-center gap-1.5">
          <span className="text-[11px] text-slate-500 mr-1">试试：</span>
          {EXAMPLES.map((example) => (
            <button
              key={example}
              type="button"
              onClick={() => search(example)}
              className="px-2 py-0.5 rounded-md bg-slate-800 hover:bg-slate-700 border border-white/10 text-[11px] text-slate-300 hover:text-amber-300 cursor-pointer"
            >
              {example}
            </button>
          ))}
        </div>
      </div>

      {error && (
        <div className="p-3 rounded-xl bg-red-950/40 border border-red-500/30 text-red-300 text-xs flex items-start gap-2">
          <AlertTriangle className="w-4 h-4 shrink-0 mt-0.5" />
          <span>{error}</span>
        </div>
      )}

      {result && !hasHits(result) && (
        <div className="p-10 text-center text-slate-500 space-y-2 rounded-xl bg-slate-900/50 border border-white/10">
          <Boxes className="w-10 h-10 mx-auto text-slate-600" />
          <p className="text-sm">没查到「{shownKeyword}」的掉落资料</p>
          <p className="text-xs text-slate-600">换个说法试试：全名或直接填 ID。</p>
        </div>
      )}

      {result && hasHits(result) && (
        <>
          {/* 命中列表：让用户能精确点进他真正想要的那个 */}
          {(result.matches.mobs.length > 0 || result.matches.items.length > 0) && (
            <div className="p-4 rounded-2xl bg-slate-900/70 border border-white/10 space-y-3">
              {result.matches.mobs.length > 0 && (
                <div className="space-y-1.5">
                  <div className="text-[11px] text-slate-400 flex items-center gap-1.5">
                    <Skull className="w-3.5 h-3.5 text-red-400" />
                    名字里带「{shownKeyword}」的怪物（{result.matches.mobTotal} 只）· 点一下查它的掉落
                  </div>
                  <div className="flex flex-wrap gap-1.5">
                    {result.matches.mobs.slice(0, 12).map((mob) => (
                      <button
                        key={`${mob.mobId}-${mob.name}`}
                        type="button"
                        onClick={() => search(String(mob.mobId))}
                        title={`怪物 ID ${mob.mobId}`}
                        className="flex items-center gap-1.5 pl-1 pr-2 py-0.5 rounded-lg bg-slate-800 hover:bg-slate-700 border border-white/10 text-[11px] text-slate-200 hover:text-amber-200 transition-colors cursor-pointer"
                      >
                        {mob.icon ? (
                          <img
                            src={mob.icon}
                            alt=""
                            className="w-5 h-5 object-contain"
                            loading="lazy"
                          />
                        ) : (
                          <Skull className="w-3.5 h-3.5 text-slate-500" />
                        )}
                        {mob.name}
                        {mob.level > 0 && (
                          <span className="text-slate-500 tabular-nums">Lv{mob.level}</span>
                        )}
                      </button>
                    ))}
                  </div>
                </div>
              )}

              {result.matches.items.length > 0 && (
                <div className="space-y-1.5">
                  <div className="text-[11px] text-slate-400 flex items-center gap-1.5">
                    <Package className="w-3.5 h-3.5 text-amber-400" />
                    名字里带「{shownKeyword}」的道具（{result.matches.itemTotal} 件）· 点一下查它从哪掉
                  </div>
                  <div className="flex flex-wrap gap-1.5">
                    {result.matches.items.slice(0, 12).map((item) => (
                      <button
                        key={item.itemId}
                        type="button"
                        onClick={() => search(String(item.itemId))}
                        title={`道具 ID ${item.itemId}`}
                        className="flex items-center gap-1.5 pl-1 pr-2 py-0.5 rounded-lg bg-slate-800 hover:bg-slate-700 border border-white/10 text-[11px] text-slate-200 hover:text-amber-200 transition-colors cursor-pointer"
                      >
                        {item.icon ? (
                          <img
                            src={item.icon}
                            alt=""
                            className="w-5 h-5 object-contain"
                            loading="lazy"
                          />
                        ) : (
                          <Package className="w-3.5 h-3.5 text-slate-500" />
                        )}
                        {item.name}
                      </button>
                    ))}
                  </div>
                </div>
              )}
            </div>
          )}

          {/* 掉落结果 */}
          <div className="space-y-4">
            {result.results.map((hit) => (
              <MobCard
                key={`${hit.mob.mobId}-${hit.mob.name}`}
                hit={hit}
                openDrops={!!openDrops[hit.mob.mobId]}
                openMaps={!!openMaps[hit.mob.mobId]}
                onToggleDrops={() =>
                  setOpenDrops((prev) => ({ ...prev, [hit.mob.mobId]: !prev[hit.mob.mobId] }))
                }
                onToggleMaps={() =>
                  setOpenMaps((prev) => ({ ...prev, [hit.mob.mobId]: !prev[hit.mob.mobId] }))
                }
                sortedDrops={sortedDrops}
                onOpenPage={openPage}
                onSearchPrice={onSearchPrice}
              />
            ))}
          </div>

          {/* 翻页：站点一页最多十几只怪物 */}
          <div className="flex items-center justify-between text-[11px] text-slate-500">
            <span>
              第 {result.meta.page} / {Math.max(result.meta.totalPages, 1)} 页 · 已显示{" "}
              {result.results.length} 只怪物
            </span>
            {result.meta.page < result.meta.totalPages && (
              <button
                type="button"
                onClick={() => search(result.meta.keyword, page + 1, displayTerm ?? undefined)}
                disabled={loading}
                className="px-3 py-1.5 rounded-lg bg-slate-800 hover:bg-slate-700 border border-white/10 text-slate-200 font-semibold cursor-pointer disabled:opacity-50 flex items-center gap-1.5"
              >
                {loading && <Loader2 className="w-3.5 h-3.5 animate-spin" />}
                加载更多
              </button>
            )}
          </div>

          <p className="text-[11px] text-slate-500 leading-relaxed">
            数据来源：小册子掉落速查。概率是站点原始数值，掉落本身是随机的。
          </p>
        </>
      )}
    </div>
  );
};

/** 一只怪物的卡片：它掉什么、在哪刷。 */
const MobCard: React.FC<{
  hit: DropMobResult;
  openDrops: boolean;
  openMaps: boolean;
  onToggleDrops: () => void;
  onToggleMaps: () => void;
  sortedDrops: (drops: DropEntry[]) => DropEntry[];
  onOpenPage: (url: string) => void;
  onSearchPrice?: (name: string) => void;
}> = ({
  hit,
  openDrops,
  openMaps,
  onToggleDrops,
  onToggleMaps,
  sortedDrops,
  onOpenPage,
  onSearchPrice,
}) => {
  const drops = sortedDrops(hit.drops);
  const visibleDrops = openDrops ? drops : drops.slice(0, DROPS_PREVIEW);
  const visibleMaps = openMaps ? hit.maps : hit.maps.slice(0, MAPS_PREVIEW);

  return (
    <div className="rounded-2xl bg-slate-900/70 border border-white/10 overflow-hidden">
      {/* 怪物抬头 */}
      <div className="p-4 flex flex-wrap items-center gap-3 border-b border-white/5 bg-slate-900/40">
        {hit.mob.icon ? (
          <img
            src={hit.mob.icon}
            alt=""
            className="w-12 h-12 rounded-lg bg-slate-950/60 border border-white/10 p-0.5 object-contain"
            loading="lazy"
          />
        ) : (
          <div className="w-12 h-12 rounded-lg bg-slate-800 border border-white/10 flex items-center justify-center">
            <Skull className="w-5 h-5 text-slate-500" />
          </div>
        )}

        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <span className="text-base font-bold text-slate-100">{hit.mob.name}</span>
            {hit.mob.boss && (
              <span className="text-[10px] px-1.5 py-0.5 rounded bg-red-500/20 text-red-300 border border-red-500/40 font-bold">
                BOSS
              </span>
            )}
            {hit.mob.categoryLabel && (
              <span className="text-[10px] px-1.5 py-0.5 rounded bg-slate-800 text-slate-400 border border-white/10">
                {hit.mob.categoryLabel}
              </span>
            )}
            <button
              type="button"
              onClick={() => onOpenPage(hit.mob.pageUrl)}
              className="text-[10px] text-amber-400/90 hover:text-amber-300 flex items-center gap-0.5 cursor-pointer"
              title="在小册子打开这只怪物的图鉴"
            >
              图鉴
              <ExternalLink className="w-2.5 h-2.5" />
            </button>
          </div>

          <div className="flex flex-wrap gap-x-4 gap-y-0.5 text-[11px] text-slate-400 mt-1 tabular-nums">
            <span>Lv.{hit.mob.level || "-"}</span>
            <span>HP {hit.mob.hp || "-"}</span>
            <span>EXP {hit.mob.exp || "-"}</span>
            <span className="font-sans text-slate-500">ID {hit.mob.mobId}</span>
          </div>
        </div>

        <div className="flex items-center gap-3 text-[11px] text-slate-400">
          <span className="flex items-center gap-1">
            <Boxes className="w-3.5 h-3.5 text-amber-400" />
            掉落 {hit.drops.length} 条
          </span>
          <span className="flex items-center gap-1">
            <MapPin className="w-3.5 h-3.5 text-sky-400" />
            地图 {hit.maps.length} 个
          </span>
        </div>
      </div>

      <div className="grid grid-cols-1 lg:grid-cols-2 gap-0 lg:divide-x divide-white/5">
        {/* 掉落 */}
        <div className="p-3 space-y-1">
          <div className="text-[11px] text-slate-400 flex items-center gap-1.5 mb-1.5">
            <Coins className="w-3.5 h-3.5 text-amber-400" />
            掉落道具
            {hit.drops.length > DROPS_PREVIEW && (
              <button
                type="button"
                onClick={onToggleDrops}
                className="ml-auto text-amber-300/90 hover:text-amber-200 flex items-center gap-0.5 cursor-pointer"
              >
                {openDrops ? "收起" : `展开全部 ${hit.drops.length} 条`}
                {openDrops ? (
                  <ChevronUp className="w-3 h-3" />
                ) : (
                  <ChevronDown className="w-3 h-3" />
                )}
              </button>
            )}
          </div>

          {hit.drops.length === 0 && (
            <p className="text-[11px] text-slate-500 py-2">站点没有记录这只怪物的掉落。</p>
          )}

          {visibleDrops.map((drop, index) => (
            <div
              key={`${drop.item.itemId}-${index}`}
              className={`w-full flex items-center gap-2 px-2 py-1.5 rounded-lg border transition-colors ${
                drop.matched
                  ? "bg-emerald-500/10 border-emerald-500/30 hover:bg-emerald-500/15"
                  : "bg-slate-950/40 border-white/5 hover:bg-slate-800/60"
              }`}
            >
              {/* 名称区保持原行为（占满剩余宽度，点了开图鉴）——整行不能再是按钮，按钮里不能套按钮 */}
              <button
                type="button"
                onClick={() => onOpenPage(drop.item.pageUrl)}
                title="在小册子打开这件道具的图鉴"
                className="min-w-0 flex-1 flex items-center gap-2 text-left cursor-pointer"
              >
                {drop.item.icon ? (
                  <img
                    src={drop.item.icon}
                    alt=""
                    className="w-6 h-6 object-contain shrink-0"
                    loading="lazy"
                  />
                ) : (
                  <Package className="w-4 h-4 text-slate-600 shrink-0" />
                )}

                <span className="min-w-0 flex-1">
                  <span className="block text-xs text-slate-200 truncate">
                    {drop.item.name}
                    {drop.item.reqLevel > 0 && (
                      <span className="text-[10px] text-slate-500 ml-1 tabular-nums">
                        Lv{drop.item.reqLevel}
                      </span>
                    )}
                  </span>
                  <span className="block text-[10px] text-slate-500 tabular-nums">
                    ×{drop.min}-{drop.max}
                    {drop.questid > 0 && " · 任务道具"}
                  </span>
                </span>
              </button>

              {/* 任务道具不显示（查拍卖没意义），道具名为空的也不行 */}
              {onSearchPrice && drop.questid <= 0 && drop.item.name.trim() !== "" && (
                <button
                  type="button"
                  onClick={() => onSearchPrice(drop.item.name)}
                  title="查这件的拍卖价"
                  className="shrink-0 flex items-center gap-1 px-2 py-0.5 rounded-lg border border-white/10 bg-slate-800/80 text-[11px] text-slate-300 hover:border-amber-400/40 hover:text-amber-300 transition-colors cursor-pointer"
                >
                  <Coins className="w-3 h-3" />
                  查价
                </button>
              )}

              <span
                className={`shrink-0 text-xs tabular-nums font-bold ${
                  drop.matched ? "text-emerald-300" : "text-amber-300"
                }`}
              >
                {drop.chanceText}
              </span>
            </div>
          ))}
        </div>

        {/* 出没地图 */}
        <div className="p-3 space-y-1">
          <div className="text-[11px] text-slate-400 flex items-center gap-1.5 mb-1.5">
            <MapPin className="w-3.5 h-3.5 text-sky-400" />
            出没地图
            {hit.maps.length > MAPS_PREVIEW && (
              <button
                type="button"
                onClick={onToggleMaps}
                className="ml-auto text-amber-300/90 hover:text-amber-200 flex items-center gap-0.5 cursor-pointer"
              >
                {openMaps ? "收起" : `展开全部 ${hit.maps.length} 个`}
                {openMaps ? <ChevronUp className="w-3 h-3" /> : <ChevronDown className="w-3 h-3" />}
              </button>
            )}
          </div>

          {hit.maps.length === 0 && (
            <p className="text-[11px] text-slate-500 py-2">站点没有记录它出没的地图。</p>
          )}

          <div className="flex flex-wrap gap-1.5">
            {visibleMaps.map((map) => (
              <button
                key={map.mapId}
                type="button"
                onClick={() => onOpenPage(map.pageUrl)}
                title={`地图 ID ${map.mapId} · 点开在小册子查看`}
                className="text-left px-2 py-1 rounded-lg bg-slate-950/40 hover:bg-slate-800/70 border border-white/5 text-[11px] text-slate-300 hover:text-sky-200 transition-colors cursor-pointer max-w-full"
              >
                <span className="block truncate">{map.name}</span>
                {map.street && (
                  <span className="block text-[10px] text-slate-500">{map.street}</span>
                )}
              </button>
            ))}
          </div>
        </div>
      </div>
    </div>
  );
};
