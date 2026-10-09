import React, { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { COOLDOWN, guard } from "../lib/throttle";
import { 
  Search, 
  X, 
  Package, 
  AlertCircle, 
  Tag, 
  Filter,
  CheckCircle2,
  RefreshCw,
  Database,
  History,
  Info,
  Sword
} from "lucide-react";
import type { MarketItem, MarketQueryResult } from "../types";
import { usePresetServer } from "../lib/presetServer";
import { formatAge } from "../lib/format";
import { useItemDetail } from "../lib/useItemDetail";
import { useLatestRequest } from "../lib/latest";
import { useQueryHistory, rememberQuery } from "../lib/queryHistory";
import { ItemDetailPanel } from "./ItemDetailPanel";

/** 从别处（总览的「最近查价」）带一个词进来，进页就查。`nonce` 变了才算新的一次。 */
export interface MarketSeed {
  term: string;
  nonce: number;
}

export const MarketSearch: React.FC<{
  seed?: MarketSeed;
  /**
   * 点结果卡片的「谁掉」：把词交给查询页去切到掉落页签。
   * `itemId` 是更准的查询词（用道具 ID 查不掉错别的东西，见方案 5.1），
   * 展示仍用 `term`；没解析出 ID 时为 undefined。
   */
  onSearchDrops?: (term: string, itemId?: string) => void;
}> = ({ seed, onSearchDrops }) => {
  const [keyword, setKeyword] = useState("");
  // 全局预设区服：换服会写回数据库并广播，悬窗也跟着切（不再每开一个窗口都要选一次）
  const { serverId, servers, chooseServer } = usePresetServer();
  const [items, setItems] = useState<MarketItem[]>([]);
  const [loading, setLoading] = useState(false);
  const [errorMsg, setErrorMsg] = useState("");
  const [searchedKeyword, setSearchedKeyword] = useState("");
  // 这份结果是实时抓的、还是本地缓存？缓存又是多久前写进去的？
  const [source, setSource] = useState<"live" | "cache" | null>(null);
  const [cacheAge, setCacheAge] = useState<number | null>(null);

  // 每条结果右侧的「物品详情」（官方基准属性 + NPC 出售价）
  const itemDetail = useItemDetail();
  /** 只认最后一次查询的响应，慢的旧响应不许覆盖新结果 */
  const beginRequest = useLatestRequest();
  /** 查价关键词历史：两个入口（主窗 + 悬浮窗）共用同一份 localStorage */
  const history = useQueryHistory();

  const handleSearch = async (forceRefresh = false, override?: string) => {
    // 下拉选中时带着那个词来查：不能等 setKeyword 再读 state（异步刷新
    // 拿到的还是旧词），所以提交函数要能吃一个显式的词。
    const term = (override ?? keyword).trim();
    if (!term) return;
    // 查询中就别再发了：站点限流的风控是真实存在的，连按回车就是连着几个请求
    if (loading) return;
    const isCurrent = beginRequest();
    setLoading(true);
    setErrorMsg("");
    try {
      // 拍卖查询是真的去打小册子那个站点，别让连点变成连发
      if (!guard("query_market", "拍卖查询", COOLDOWN.query)) return;
      const result = await invoke<MarketQueryResult>("query_market", {
        keyword: term,
        serverId,
        forceRefresh,
      });
      // 换词重查时旧响应可能后到，写进去就成了「搜 A 显示 B」
      if (!isCurrent()) return;
      setItems(result.items);
      setSource(result.source);
      setCacheAge(result.cached_seconds_ago);
      setSearchedKeyword(term);
      // 历史只记「以后再点还能查到东西」的词：失败不记（打错的词），
      // **0 结果也不记** —— 拍卖行有没有货会变，但重跑一次还是 0，
      // 纯属污染下拉（和「失败不记」是同一个道理的下一条）。
      if (result.items.length > 0) rememberQuery(term);
    } catch (err: unknown) {
      if (!isCurrent()) return;
      setErrorMsg(typeof err === "string" ? err : "查询物价出错");
    } finally {
      setLoading(false);
    }
  };

  /**
   * 已经处理过的 seed nonce：同一个 nonce 只查一次。
   *
   * 开发模式的 StrictMode 会把**挂载时**的 effect 跑两遍：第二遍会被冷却 `guard`
   * 拦下（弹一条假的「太频繁」），还顺手作废第一遍的响应（`beginRequest` 已推进
   * 序号）—— 表现是「点了最近查价，输入框有词、结果空着」。用 ref 去重没这回事
   * （ref 在 StrictMode 的重跑之间保留）。生产构建 effect 只跑一遍，这里是保险。
   */
  const handledSeed = useRef(0);

  useEffect(() => {
    if (!seed?.term || handledSeed.current === seed.nonce) return;
    handledSeed.current = seed.nonce;
    setKeyword(seed.term);
    void handleSearch(false, seed.term);
    // 只在「新的一次带词进来」时触发；handleSearch 每次渲染都是新函数，不能放进依赖
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [seed?.nonce]);

  return (
    <div className="space-y-6">
      {/* 搜索区 */}
      <div className="p-4 rounded-2xl bg-slate-900/70 border border-white/10 space-y-3">
        <p className="text-xs text-slate-400">
          小册子拍卖行行情（5 分钟本地缓存），按区服查询，回车即可。
        </p>

        <div className="flex flex-col sm:flex-row items-center gap-3">
          {/* Server Selector */}
          <div className="relative w-full sm:w-44">
            <Filter className="w-4 h-4 absolute left-3 top-1/2 -translate-y-1/2 text-slate-400 pointer-events-none" />
            <select
              value={serverId}
              onChange={(e) => chooseServer(Number(e.target.value))}
              title="换服会存成全局预设"
              className="w-full bg-slate-800 border border-white/10 rounded-xl pl-9 pr-3 py-2.5 text-sm text-amber-300 font-medium focus:outline-none focus:border-amber-400 cursor-pointer"
            >
              {servers.map((s) => (
                <option key={s.id} value={s.id}>
                  {s.name}
                </option>
              ))}
            </select>
          </div>

          {/* Search Input */}
          <div className="relative flex-1 w-full">
            <Search className="w-4 h-4 absolute left-3 top-1/2 -translate-y-1/2 text-slate-400 pointer-events-none" />
            <input
              type="text"
              value={keyword}
              onChange={(e) => {
                setKeyword(e.target.value);
                // 继续输入时刷新下拉（按新前缀过滤）；focus 用 onFocus 那一次
                history.show(e.target.value);
              }}
              onFocus={(e) => history.show(e.currentTarget.value)}
              onBlur={history.close}
              onKeyDown={(e) => {
                // `isComposing`：中文输入法选词的回车不是「提交查询」，
                // 不挡的话每选一次词都会发一次请求
                if (e.nativeEvent.isComposing) return;
                // 下拉的键盘：先于提交处理，不然 ↑ 会把光标挪到行首。
                // **只在下拉确实可见时才拦** —— 没开的时候 ↑↓ 是输入框自己的
                // 光标移动，preventDefault 会把它弄坏（审查抓到的回归）。
                if (history.open && e.key === "ArrowDown") {
                  e.preventDefault();
                  history.move(1);
                  return;
                }
                if (history.open && e.key === "ArrowUp") {
                  e.preventDefault();
                  history.move(-1);
                  return;
                }
                if (e.key === "Escape") {
                  history.close();
                  return;
                }
                if (e.key === "Enter") {
                  // 高亮在下拉项上 → 回填并直接查（点条目和它走同一条路）
                  const picked = history.takeHighlighted();
                  if (picked) {
                    setKeyword(picked);
                    handleSearch(false, picked);
                  } else {
                    history.close();
                    handleSearch();
                  }
                }
              }}
              placeholder="输入物品名称后回车查询 (例如: 锅盖、桑拿服、枫叶盾、白日梦)..."
              className="w-full bg-slate-800 border border-white/10 rounded-xl pl-9 pr-8 py-2.5 text-sm text-slate-100 placeholder:text-slate-500 focus:outline-none focus:border-amber-400"
            />

            {/* 最近查询下拉：focus 且有匹配时出现；点条目 = 回填 + 立即查。
                onMouseDown preventDefault 挡住失焦，点击才能完整走到 onClick
                （不这样 blur 会先把下拉关掉，onClick 根本轮不到）。 */}
            {history.open && history.items.length > 0 && (
              <div className="absolute left-0 right-0 top-full mt-1 z-20 rounded-lg border border-white/10 bg-slate-900 shadow-xl overflow-hidden">
                <div className="px-3 pt-1.5 pb-1 text-[10px] text-slate-500">最近查询（↑↓ 选择，回车确认）</div>
                {history.items.map((term, index) => (
                  <button
                    key={term}
                    type="button"
                    onMouseDown={(e) => e.preventDefault()}
                    onClick={() => {
                      setKeyword(term);
                      history.close();
                      handleSearch(false, term);
                    }}
                    className={`w-full text-left px-3 py-1.5 text-xs flex items-center gap-2 transition-colors cursor-pointer ${
                      index === history.highlight
                        ? "bg-amber-500/15 text-amber-200"
                        : "text-slate-300 hover:bg-slate-800"
                    }`}
                  >
                    <History className="w-3 h-3 shrink-0 text-slate-500" />
                    <span className="truncate">{term}</span>
                  </button>
                ))}
              </div>
            )}
            {keyword && (
              <button
                // onMouseDown preventDefault：不让点击把焦点挪到按钮上 ——
                // 不然输入框先 blur（关掉下拉），这里再 show 反而把下拉挂在
                // 一个**没聚焦**的输入框旁边，点外面也关不掉（评审指出的挂起）。
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => {
                  setKeyword("");
                  // 清空后退回全量列表：焦点还在输入框里，blur 时会正常关掉
                  history.show("");
                }}
                className="absolute right-3 top-1/2 -translate-y-1/2 text-slate-400 hover:text-white"
              >
                <X className="w-4 h-4" />
              </button>
            )}
          </div>

          {/* Submit Button */}
          <button
            onClick={() => handleSearch()}
            disabled={loading}
            className="w-full sm:w-auto px-5 py-2.5 rounded-xl bg-amber-500 hover:bg-amber-400 text-slate-950 font-bold text-sm disabled:opacity-50 flex items-center justify-center gap-1.5 cursor-pointer"
          >
            <Search className="w-4 h-4" />
            {loading ? "检索中..." : "查询物价"}
          </button>
        </div>

        {/* 结果来源 + 强制刷新：本地缓存 5 分钟，脏了可以自己踩一脚 */}
        <div className="flex items-center justify-between gap-3 text-[11px] text-slate-400">
          <span className="flex items-center gap-1.5">
            {source === "live" && (
              <>
                <RefreshCw className="w-3 h-3 text-emerald-400" />
                本次为实时抓取
              </>
            )}
            {source === "cache" && (
              <>
                <Database className="w-3 h-3 text-sky-400" />
                本地缓存（{formatAge(cacheAge)}前抓取）
              </>
            )}
            {!source && <span className="text-slate-500">结果会标明：实时 / 缓存</span>}
          </span>
          <button
            type="button"
            onClick={() => handleSearch(true)}
            disabled={loading || !keyword.trim()}
            title="忽略本地缓存，重新向小册子抓取一次"
            className="flex shrink-0 items-center gap-1 text-slate-300 hover:text-amber-300 disabled:opacity-40 cursor-pointer"
          >
            <RefreshCw className="w-3 h-3" />
            强制刷新
          </button>
        </div>
      </div>

      {errorMsg && (
        <div className="p-4 rounded-xl bg-red-950/40 border border-red-500/30 text-red-300 text-sm flex items-start gap-2.5">
          <AlertCircle className="w-5 h-5 text-red-400 shrink-0 mt-0.5" />
          <div className="space-y-1">
            <p className="font-semibold">查询未完成</p>
            <p className="text-xs text-red-300/90">{errorMsg}</p>
          </div>
        </div>
      )}

      {/* Results Header */}
      {searchedKeyword && !loading && (
        <div className="flex items-center justify-between text-xs text-slate-400 px-1">
          <span>
            关于 <strong className="text-amber-400">"{searchedKeyword}"</strong> 的搜索结果：共找到 {items.length} 个行情
          </span>
          <span className="flex items-center gap-1 text-emerald-400">
            <CheckCircle2 className="w-3.5 h-3.5" />
            已接入小册子实时行情
          </span>
        </div>
      )}

      {/* Results Grid */}
      <div key={searchedKeyword} className="anim-stagger grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
        {items.map((item, idx) => (
          <div
            key={idx}
            className="lift p-4 rounded-2xl bg-slate-900/70 border border-white/10 hover:border-amber-500/40 flex flex-col justify-between"
          >
            <div className="flex items-start gap-3">
              {item.icon_url ? (
                <img
                  src={item.icon_url}
                  alt={item.name}
                  className="w-12 h-12 rounded-xl bg-slate-800 p-1 border border-white/10 object-contain shrink-0"
                />
              ) : (
                <div className="w-12 h-12 rounded-xl bg-amber-500/10 border border-amber-500/20 flex items-center justify-center text-amber-400 font-bold shrink-0">
                  <Tag className="w-5 h-5" />
                </div>
              )}
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-1.5 flex-wrap">
                  <span className="font-bold text-slate-100 text-sm truncate">{item.name}</span>
                  <span className="text-[10px] px-1.5 py-0.5 rounded bg-amber-500/15 text-amber-300 border border-amber-500/30">
                    {item.server_name}
                  </span>
                </div>
                <div className="mt-2 text-xs text-slate-400">
                  最低在售参考:
                  <div className="text-amber-400 font-extrabold text-base mt-0.5 tabular-nums">
                    {item.lowest_price}
                  </div>
                </div>
              </div>
            </div>

            <div className="mt-3 pt-2.5 border-t border-white/5 flex items-center justify-between gap-2">
              <span className="text-[10px] text-slate-500 truncate">
                {itemDetail.details[item.id]?.sell_price || item.category || ""}
              </span>
              <div className="shrink-0 flex items-center gap-1.5">
                <button
                  type="button"
                  onClick={() => onSearchDrops?.(item.name, item.id || undefined)}
                  disabled={!item.id || !onSearchDrops}
                  title="查哪只怪掉这件"
                  className="flex items-center gap-1 rounded-lg border border-white/10 bg-slate-800/80 px-2.5 py-1 text-xs text-slate-300 hover:border-amber-400/40 hover:text-amber-300 disabled:opacity-50 transition-colors cursor-pointer"
                >
                  <Sword className="w-3 h-3" />
                  谁掉
                </button>
                <button
                  type="button"
                  onClick={() => itemDetail.toggle(item.id)}
                  disabled={!item.id || itemDetail.loadingId === item.id}
                  title="小册子道具图鉴：基准属性与 NPC 售价"
                  className="shrink-0 flex items-center gap-1 rounded-lg border border-white/10 bg-slate-800/80 px-2.5 py-1 text-xs text-slate-300 hover:border-amber-400/40 hover:text-amber-300 disabled:opacity-50 transition-colors cursor-pointer"
                >
                  <Info className="w-3 h-3" />
                  {itemDetail.loadingId === item.id
                    ? "读取中..."
                    : itemDetail.openId === item.id
                      ? "收起详情"
                      : "物品详情"}
                </button>
              </div>
            </div>

            {itemDetail.openId === item.id && (
              <div className="mt-2.5">
                <ItemDetailPanel
                  detail={itemDetail.details[item.id]}
                  loading={itemDetail.loadingId === item.id}
                  error={itemDetail.error}
                />
              </div>
            )}
          </div>
        ))}
      </div>

      {items.length === 0 && !loading && !errorMsg && (
        <div className="p-12 text-center rounded-xl bg-slate-900/40 border border-white/5 space-y-3">
          <Package className="w-12 h-12 text-slate-600 mx-auto" />
          <p className="text-slate-400 text-sm">暂无查询数据</p>
          <p className="text-slate-500 text-xs">输入道具名称，回车查看该服在售参考价。</p>
        </div>
      )}
    </div>
  );
};
