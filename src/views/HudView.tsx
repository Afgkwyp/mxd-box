import React, { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { COOLDOWN, guard, peekGuard } from "../lib/throttle";
import { useQueryHistory, rememberQuery } from "../lib/queryHistory";
import {
  Search,
  X,
  RefreshCw,
  ShieldAlert,
  ShieldCheck,
  ShieldQuestion,
  Sword,
  UserSearch,
  Package,
  MapPin,
  ChevronDown,
  ChevronRight,
  Moon,
  Sun,
  History as HistoryIcon,
  Loader2,
} from "lucide-react";
import type {
  BlacklistVerdict,
  BuildInfo,
  DropEntry,
  DropSearchResult,
  ItemDetail,
  MarketItem,
  MarketQueryResult,
} from "../types";
import { formatAge } from "../lib/format";
import { usePresetServer } from "../lib/presetServer";
import { useItemDetail } from "../lib/useItemDetail";
import { useMemorySample } from "../lib/memory";
import { useLatestRequest } from "../lib/latest";
import { useHotkey } from "../lib/hotkeys";
import { useTheme } from "../lib/theme";
import { ChannelBadge } from "../components/ChannelBadge";
import { ResizeHandles, useEnterAnimation, useWindowDrag, useWindowSize } from "../components/WindowFrame";

/**
 * **查价窗**（`hud` 窗口，全局快捷键默认 Alt+F，切换式显示 / 收起）。
 *
 * 「在游戏里随手查点东西」的那个窗，一根输入框分三个模式：
 *
 * * **查价**（默认）：拍卖行搜索 + 物品详情；
 * * **掉落**：怪物名 / 道具名 / ID —— 搜「锅盖」看哪几只怪掉它，
 *   搜「无魂猴」看这只怪掉什么（公开接口，不需要登录）；
 * * **查人**：黑名单即时比对。
 *
 * 都放在这里而不是只放主窗口：打游戏时你正开着游戏，这个窗是唯一已经在屏幕上的。
 *
 * ## 版式（方向 B「终端」）
 *
 * 查价结果是**左列表 + 右详情**：↑/↓ 换选中项，右边的官方基准属性 / NPC 出售价跟着变，
 * 不用再一条条点「详情」。窗口被拖窄（面板 < 560px）时右栏自己收进选中行的下方；
 * 再矮就先收页脚、表头。这些都是 CSS 容器查询（styles/overlay.css）在做，
 * 这里只负责把两种摆法都渲染出来。
 *
 * 另一个完全不同的窗口是**经验窗**（`float` 窗口，默认 F10）——见 `FloatView.tsx`。
 */

const PLAYER_VERDICT_LABEL: Record<BlacklistVerdict["status"], string> = {
  hit: "确定命中：同名 + 同服",
  other_server: "注意：只在别的服被记过",
  similar: "疑似：有名字很接近的记录",
  clear: "本地库没有记录",
};

const PLAYER_VERDICT_HINT: Record<BlacklistVerdict["status"], string> = {
  hit: "交易请走担保，或直接不做这笔。",
  other_server: "同名很常见，不能就此认定是同一人。",
  similar: "可能是小号式改名，对照一下区服和原因。",
  clear: "只说明本地库没有，不代表他干净。",
};

/** 悬浮窗里地图最多平铺这么多，剩下的只报数量。 */
const HUD_MAPS_PREVIEW = 8;
const HUD_MIN_W = 580;
const HUD_MIN_H = 360;

type Mode = "price" | "drop" | "player";

const MODES: Array<{ key: Mode; label: string }> = [
  { key: "price", label: "查价" },
  { key: "drop", label: "掉落" },
  { key: "player", label: "查人" },
];

/** 右栏 / 行内展开共用的物品详情。 */
const HudItemDetail: React.FC<{
  item: MarketItem;
  detail?: ItemDetail;
  loading: boolean;
  error: string;
  onLoad: () => void;
  /** 「谁掉」：切到掉落模式查这件（见方案 3.2） */
  onDrops?: () => void;
}> = ({ item, detail, loading, error, onLoad, onDrops }) => (
  <div className="tm-mono">
    <div style={{ display: "flex", alignItems: "baseline", justifyContent: "space-between", gap: 8 }}>
      <b style={{ fontSize: 14, fontFamily: "inherit" }}>{item.name}</b>
      <span style={{ fontSize: 10, color: "var(--tm-dm)" }}>{item.server_name}</span>
    </div>
    <div className="tm-big">{item.lowest_price}</div>
    {onDrops && (
      <button type="button" className="tm-link" style={{ marginBottom: 8 }} onClick={onDrops} title="查哪只怪掉这件">
        谁掉 ›
      </button>
    )}

    {loading && (
      <div style={{ color: "var(--tm-mu)", display: "flex", alignItems: "center", gap: 6, fontSize: 11 }}>
        <Loader2 size={12} className="animate-spin" />
        读取小册子图鉴…
      </div>
    )}
    {!loading && error && <div style={{ color: "var(--tm-bd)", fontSize: 11 }}>{error}</div>}
    {!loading && !error && !detail && (
      <button type="button" className="tm-link" onClick={onLoad}>
        加载图鉴详情 ›
      </button>
    )}
    {!loading && !error && detail && (
      <>
        {detail.reqs.map((fact) => (
          <div key={fact.label} className="tm-kv">
            <span>{fact.label}</span>
            <span>{fact.value}</span>
          </div>
        ))}
        {detail.props.map((prop) => (
          <div key={prop.label} className="tm-kv">
            <span>{prop.label}</span>
            <span>
              {prop.value}
              {prop.range && <span style={{ color: "var(--tm-dm)" }}> {prop.range}</span>}
            </span>
          </div>
        ))}
        {detail.jobs.length > 0 && (
          <div className="tm-kv">
            <span>职业</span>
            <span style={{ textAlign: "right" }}>{detail.jobs.join(" / ")}</span>
          </div>
        )}
        <div className="tm-kv">
          <span>{detail.upgrade || (detail.equipment ? "不可强化" : "非装备")}</span>
          <span>NPC {detail.sell_price || "—"}</span>
        </div>
      </>
    )}
  </div>
);

/** 掉落行（紧凑）。「价」按钮存在时才显示（任务道具、道具名为空的行没有）。 */
const HudDropRow: React.FC<{
  drop: DropEntry;
  onOpen: (url: string) => void;
  onPrice?: (name: string) => void;
}> = ({ drop, onOpen, onPrice }) => (
  <div className={`tm-drop-row ${drop.matched ? "hit" : ""}`}>
    {/* 名称区保持原行为（占满剩余宽度，点了开图鉴）——整行不再是按钮，按钮里不能套按钮 */}
    <button
      type="button"
      className="tm-dr-main"
      onClick={() => onOpen(drop.item.pageUrl)}
      title="在小册子打开这件道具的图鉴"
    >
      {drop.item.icon ? (
        <img src={drop.item.icon} alt="" width={18} height={18} style={{ objectFit: "contain" }} loading="lazy" />
      ) : (
        <Package size={14} style={{ color: "var(--tm-dm)" }} />
      )}
      <span style={{ flex: 1, minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
        {drop.item.name}
        <span className="tm-mono" style={{ color: "var(--tm-dm)", marginLeft: 6, fontSize: 10 }}>
          ×{drop.min}-{drop.max}
          {drop.questid > 0 && " · 任务"}
        </span>
      </span>
    </button>
    {onPrice && drop.questid <= 0 && drop.item.name.trim() !== "" && (
      <button
        type="button"
        className="tm-dr-price"
        onClick={() => onPrice(drop.item.name)}
        title="查这件的拍卖价"
      >
        价
      </button>
    )}
    <span
      className="tm-mono"
      style={{ fontWeight: 700, color: drop.matched ? "var(--tm-good)" : "var(--tm-wn)" }}
    >
      {drop.chanceText}
    </span>
  </div>
);

export const HudView: React.FC = () => {
  const [keyword, setKeyword] = useState("");
  /** 同一根输入框、三种用途：查物价 / 查掉落 / 查人（黑名单） */
  const [mode, setMode] = useState<Mode>("price");
  const [verdict, setVerdict] = useState<BlacklistVerdict | null>(null);
  const [checking, setChecking] = useState(false);
  const [drops, setDrops] = useState<DropSearchResult | null>(null);
  const [dropsLoading, setDropsLoading] = useState(false);
  /** 掉落结果的页码（站点一页 12 只怪，搜「蘑菇」有 34 只 → 3 页） */
  const [dropsPage, setDropsPage] = useState(1);
  /** 掉落结果区文案用的词：联动「谁掉」实际用道具 ID 查，展示仍是道具名（不露裸 ID） */
  const [dropsDisplay, setDropsDisplay] = useState("");
  const [expandedMobs, setExpandedMobs] = useState<Record<number, boolean>>({});
  const [items, setItems] = useState<MarketItem[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [errorMsg, setErrorMsg] = useState("");
  const [source, setSource] = useState<"live" | "cache" | null>(null);
  const [cacheAge, setCacheAge] = useState<number | null>(null);

  const { serverId, servers, chooseServer } = usePresetServer();
  const itemDetail = useItemDetail();
  const [build, setBuild] = useState<BuildInfo | null>(null);
  /** 提示里写**实际在用**的快捷键（用户可能改过），见 lib/hotkeys.ts */
  const hotkeyHint = useHotkey("price_hud", "Alt+F");
  const memory = useMemorySample();
  const [theme, toggleTheme] = useTheme();
  /** 只认最后一次查询的响应，慢的旧响应不许覆盖新结果 */
  const beginRequest = useLatestRequest();
  /** 查价关键词历史：和主窗口的查价页共用同一份 localStorage，只在查价模式下弹 */
  const history = useQueryHistory();

  // 和 tauri.conf.json 的 minWidth / minHeight 一致：这个大小下头部、详情栏、页脚都放得下
  useWindowSize("mxdbox.window.hud", HUD_MIN_W, HUD_MIN_H);
  useEnterAnimation();
  const drag = useWindowDrag();
  /** 每次拿到新结果 +1：列表以它为 key 重新挂载，行的入场动画才会重播 */
  const [runId, setRunId] = useState(0);

  const inputRef = useRef<HTMLInputElement>(null);
  /**
   * Esc 处理函数挂在 window 上只挂一次，用 ref 读当前关键词就行（否则每打一个字
   * 都要解绑重挂）。在 effect 里同步：渲染期写 ref 会让 React 无法可靠地跳过重渲染。
   */
  const keywordRef = useRef(keyword);
  useEffect(() => {
    keywordRef.current = keyword;
  }, [keyword]);

  useEffect(() => {
    inputRef.current?.focus();

    invoke<BuildInfo>("get_build_info")
      .then(setBuild)
      .catch(() => {});

    // Esc：**输入框里有内容就只清空，空了再收起窗口**。
    // 中文输入法组字期间按 Esc 也会冒泡到 window，那种情况一律不管。
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.isComposing) return;
      if (keywordRef.current) {
        setKeyword("");
        return;
      }
      invoke("hide_hud");
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, []);

  /** 切模式后把焦点还给输入框（Tab 也走这里）。 */
  const switchMode = (next: Mode) => {
    setMode(next);
    setErrorMsg("");
    history.close();
    inputRef.current?.focus();
  };

  const selectItem = (item: MarketItem) => {
    setSelectedId(item.id);
    // 详情按需取：已经缓存的直接展示，没有的才去站点
    if (item.id && itemDetail.openId !== item.id) itemDetail.toggle(item.id);
  };

  /**
   * 「掉落」：怪物名 / 道具名 / 怪物ID / 道具ID 都走同一个公开接口。
   *
   * 默认展开哪几只是按**这次查询的意图**定的：搜**怪物名**（无魂猴）→ 名字里含关键词的
   * 那几只默认摊开；搜**道具名**（锅盖）→ 一只都不展开，缩略行里已经写着命中那条掉落的概率。
   * `nextPage > 1` 是「加载更多」：新一页**接在后面**而不是把前面的冲掉。
   */
  const handleSearchDrops = async (nextPage = 1, override?: string, display?: string) => {
    // 显式给的词优先（联动「谁掉」用道具 ID 查更准，见方案 5.1）；翻页要用
    // **当初搜的那个词**，而不是用户可能已经改过的输入框内容。
    const target =
      override?.trim() || (nextPage > 1 && drops ? drops.meta.keyword : keyword.trim());
    if (!target) return;
    const isCurrent = beginRequest();
    setDropsLoading(true);
    setErrorMsg("");
    try {
      if (!guard("search_drops", "掉落速查", COOLDOWN.query)) return;
      const result = await invoke<DropSearchResult>("search_drops", {
        keyword: target,
        page: nextPage,
      });
      if (!isCurrent()) return;
      setDrops((prev) =>
        nextPage > 1 && prev ? { ...result, results: [...prev.results, ...result.results] } : result,
      );
      // 结果区文案只在新搜索时更新（翻页沿用当前词）；联动进来时 display 是道具名
      if (nextPage === 1) setDropsDisplay(display ?? target);
      if (nextPage === 1) setRunId((n) => n + 1);
      const autoExpand = Object.fromEntries(
        result.results
          .filter((hit) => hit.mob.name.includes(result.meta.keyword.trim()))
          .map((hit) => [hit.mob.mobId, true]),
      );
      setExpandedMobs((prev) => (nextPage > 1 ? { ...autoExpand, ...prev } : autoExpand));
      setDropsPage(nextPage);
    } catch (err: unknown) {
      if (!isCurrent()) return;
      setErrorMsg(typeof err === "string" ? err : "掉落查询出错");
      setDrops(null);
      setDropsPage(1);
    } finally {
      setDropsLoading(false);
    }
  };

  /** 「查人」：调后端的四档判定（同名 + 同服才算确定命中）。 */
  const handleCheckPlayer = async () => {
    if (!keyword.trim()) return;
    const isCurrent = beginRequest();
    setChecking(true);
    setErrorMsg("");
    try {
      const result = await invoke<BlacklistVerdict>("check_blacklist", { name: keyword.trim() });
      if (!isCurrent()) return;
      setVerdict(result);
    } catch (err: unknown) {
      if (!isCurrent()) return;
      setErrorMsg(typeof err === "string" ? err : "黑名单比对出错");
    } finally {
      setChecking(false);
    }
  };

  const handleSearch = async (forceRefresh = false, override?: string) => {
    // 下拉选中时带着那个词查（不能等 setKeyword 刷完 state，那是下一轮的事）
    const term = (override ?? keyword).trim();
    if (!term) return;
    const isCurrent = beginRequest();
    setLoading(true);
    setErrorMsg("");
    try {
      if (!guard("query_market", "拍卖查询", COOLDOWN.query)) return;
      const result = await invoke<MarketQueryResult>("query_market", {
        keyword: term,
        serverId,
        forceRefresh,
      });
      if (!isCurrent()) return;
      setItems(result.items);
      setRunId((n) => n + 1);
      setSource(result.source);
      setCacheAge(result.cached_seconds_ago);
      // 历史只记「以后再点还能查到东西」的词：失败 / 打错 / 0 结果都不记
      if (result.items.length > 0) rememberQuery(term);
      // 结果一来就选中第一条，右栏立刻有内容
      const first = result.items[0];
      if (first) selectItem(first);
      else setSelectedId(null);
    } catch (err: unknown) {
      if (!isCurrent()) return;
      setErrorMsg(typeof err === "string" ? err : "查询物价出错");
    } finally {
      setLoading(false);
    }
  };

  const openSite = (url: string) => {
    if (!url) return;
    invoke("open_external_url", { url }).catch(() => {});
  };

  /**
   * 掉落行「价」：切到查价模式，输入框填道具名，立即查价。
   *
   * 冷却期内**不跳转、只提示**（方案 5.4）：`peekGuard` 只问不占，真正的闸由
   * `handleSearch` 自己过 —— 直接调 `guard` 会把这个窗口占掉，紧跟着的查询
   * 反而被拦下。
   */
  const jumpToPrice = (name: string) => {
    if (!name.trim()) return;
    if (!peekGuard("query_market", "拍卖查询", COOLDOWN.query)) return;
    switchMode("price");
    setKeyword(name);
    // 拍卖只支持按词查，用名字（方案 5.2）
    void handleSearch(false, name);
  };

  /** 详情栏「谁掉」：切到掉落模式，输入框显示道具名，实际查道具 ID 更准（方案 5.1）。 */
  const jumpToDrops = (item: MarketItem) => {
    if (!peekGuard("search_drops", "掉落速查", COOLDOWN.query)) return;
    switchMode("drop");
    setKeyword(item.name);
    // 没解析出道具 ID 的条目退回用名字查；display 让结果区文案也用道具名
    void handleSearchDrops(1, item.id || item.name, item.name);
  };

  /**
   * 回车 / 按钮：按当前模式交给对应的查询。
   *
   * `busy` 时直接返回：按钮上有 `disabled`，但**回车绕过它** —— 连按几下回车
   * 就是几个并发请求打向站点，既慢又容易触发风控。
   */
  const runSearch = () => {
    if (loading || checking || dropsLoading) return;
    if (mode === "price") return handleSearch();
    if (mode === "drop") return handleSearchDrops();
    return handleCheckPlayer();
  };

  const moveSelection = (delta: 1 | -1) => {
    if (items.length === 0) return;
    const index = Math.max(0, items.findIndex((item) => item.id === selectedId));
    const next = items[(index + delta + items.length) % items.length];
    selectItem(next);
  };

  const onInputKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    // 中文输入法选词的回车不是「提交查询」，不挡的话每选一次词都会发一次请求
    if (e.nativeEvent.isComposing) return;
    // 下拉开着时 Esc 只关下拉，并且挡住 window 级的 Esc
    if (e.key === "Escape" && history.open) {
      e.stopPropagation();
      history.close();
      return;
    }
    // Tab 换模式（输入框里 Tab 本来没有别的用途）
    if (e.key === "Tab") {
      e.preventDefault();
      const index = MODES.findIndex((entry) => entry.key === mode);
      switchMode(MODES[(index + (e.shiftKey ? -1 : 1) + MODES.length) % MODES.length].key);
      return;
    }
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      const delta = e.key === "ArrowDown" ? 1 : -1;
      // 历史下拉开着：↑↓ 走下拉；关着：↑↓ 换选中的那条结果
      if (mode === "price" && history.open) {
        e.preventDefault();
        history.move(delta);
      } else if (mode === "price" && items.length > 0) {
        e.preventDefault();
        moveSelection(delta);
      }
      return;
    }
    if (e.key !== "Enter") return;
    if (mode === "price") {
      if (loading) return;
      const picked = history.takeHighlighted();
      if (picked) {
        setKeyword(picked);
        handleSearch(false, picked);
        return;
      }
      history.close();
    }
    runSearch();
  };

  const selected = items.find((item) => item.id === selectedId) ?? null;

  // ↑↓ 换选中项时，让选中行始终在可视范围内
  useEffect(() => {
    document.querySelector(".tm-row.sel")?.scrollIntoView({ block: "nearest" });
  }, [selectedId]);
  const busy = loading || checking || dropsLoading;
  const placeholder =
    mode === "price"
      ? "物品名，回车查价（锅盖、桑拿服、白日梦…）"
      : mode === "drop"
        ? "怪物名 / 道具名 / ID（锅盖 = 谁掉它，无魂猴 = 它掉什么）"
        : "角色名，回车比对黑名单";
  const goLabel = mode === "price" ? "查价" : mode === "drop" ? "查掉落" : "查一下";

  return (
    <div className="ov-root tm-root">
      <ResizeHandles />
      <div className="tm-panel">
        {/* 标题栏（拖动区）：品牌 + 模式页签 + 频道 / 内存 + 主题 + 关闭 */}
        <div className="tm-head" {...drag}>
          <span className="tm-brand">
            枫之助
          </span>
          <div className="tm-tabs" role="tablist">
            {MODES.map((entry) => (
              <button
                key={entry.key}
                type="button"
                role="tab"
                aria-selected={mode === entry.key}
                className={`tm-tab ${mode === entry.key ? "on" : ""}`}
                onClick={() => switchMode(entry.key)}
              >
                {entry.label}
              </button>
            ))}
          </div>
          <span className="tm-mono tm-meta" style={{ marginLeft: "auto" }}>
            {memory?.is_warning ? (
              <b style={{ color: "var(--tm-bd)" }}>⚠ 内存 {memory.percent.toFixed(0)}%（建议进商城刷新）</b>
            ) : (
              <span
                title={
                  memory
                    ? `已用 ${memory.used_mb}MB / 共 ${memory.total_mb}MB，警戒线 ${memory.threshold}%`
                    : "正在向系统读取内存占用…"
                }
              >
                内存 {memory ? `${memory.percent.toFixed(0)}%` : "读取中…"}
              </span>
            )}
          </span>
          <span className="tm-channel">
            <ChannelBadge compact align="right" />
          </span>
          <button
            type="button"
            className="tm-ib"
            onClick={toggleTheme}
            title={theme === "light" ? "当前：日间主题（点击切到夜间）" : "当前：夜间主题（点击切到日间）"}
            aria-label="切换主题"
          >
            {theme === "light" ? <Sun size={14} /> : <Moon size={14} />}
          </button>
          <button type="button" className="tm-ib" title="收起悬浮窗 (Esc)" onClick={() => invoke("hide_hud")}>
            <X size={14} />
          </button>
        </div>

        {/* 输入栏 */}
        <div className="tm-bar">
          {mode !== "drop" && (
            <select
              className="tm-select"
              value={serverId}
              onChange={(e) => chooseServer(Number(e.target.value))}
              title="切换区服（所有窗口一起切）"
            >
              {servers.map((server) => (
                <option key={server.id} value={server.id}>
                  {server.name}
                </option>
              ))}
            </select>
          )}
          <label className="tm-input">
            <b className="tm-mono">
              {mode === "price" ? (
                <Search size={13} />
              ) : mode === "drop" ? (
                <Sword size={13} />
              ) : (
                <UserSearch size={13} />
              )}
            </b>
            <input
              ref={inputRef}
              type="text"
              value={keyword}
              placeholder={placeholder}
              onChange={(e) => {
                setKeyword(e.target.value);
                if (mode === "price") history.show(e.target.value);
              }}
              onFocus={(e) => {
                if (mode === "price") history.show(e.currentTarget.value);
              }}
              onBlur={history.close}
              onKeyDown={onInputKeyDown}
            />
            {keyword && (
              <button
                type="button"
                // 不让点击把焦点挪走：不然输入框先 blur（关掉下拉），再 show 反而把下拉挂在没聚焦的输入框旁
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => {
                  setKeyword("");
                  if (mode === "price") history.show("");
                }}
                style={{ background: "none", border: 0, color: "var(--tm-mu)", cursor: "pointer", display: "flex" }}
                aria-label="清空"
              >
                <X size={13} />
              </button>
            )}
          </label>
          <span className="tm-kbd tm-mono">Enter</span>
          <button type="button" className="tm-go" onClick={runSearch} disabled={busy}>
            {busy ? "…" : goLabel}
          </button>

          {mode === "price" && history.open && history.items.length > 0 && (
            <div className="tm-drop">
              <div className="cap">最近查价（↑↓ 选择，回车确认）</div>
              {history.items.map((term, index) => (
                <button
                  key={term}
                  type="button"
                  className={index === history.highlight ? "hl" : ""}
                  onMouseDown={(e) => e.preventDefault()}
                  onClick={() => {
                    setKeyword(term);
                    history.close();
                    handleSearch(false, term);
                  }}
                >
                  <HistoryIcon size={12} style={{ color: "var(--tm-dm)" }} />
                  <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{term}</span>
                </button>
              ))}
            </div>
          )}
        </div>

        {/* 结果区 */}
        <div className="tm-body">
          <div className="tm-list">
            {errorMsg && <div className="tm-err">{errorMsg}</div>}

            {/* ---- 查价 ---- */}
            {mode === "price" && !errorMsg && items.length === 0 && (
              <div className="tm-empty">
                <Search size={26} />
                <span>{loading ? "查询中…" : "输入物品名按下回车查价"}</span>
                {!loading && <span style={{ fontSize: 10 }}>↑↓ 换结果 · Tab 换模式</span>}
              </div>
            )}
            {mode === "price" && items.length > 0 && (
              <>
                <div className="tm-th tm-mono">
                  <span style={{ width: 16 }}>#</span>
                  <span style={{ flex: 1 }}>物品</span>
                  <span style={{ minWidth: 84, textAlign: "right" }}>最低在售</span>
                </div>
                <div className="tm-scroll" key={runId}>
                  {items.map((item, index) => (
                    <React.Fragment key={`${item.id}-${index}`}>
                      <button
                        type="button"
                        style={{ "--i": Math.min(index, 10) } as React.CSSProperties}
                        className={`tm-row ${item.id === selectedId ? "sel" : ""}`}
                        onClick={() => selectItem(item)}
                      >
                        <span className="idx tm-mono">{index + 1}</span>
                        {item.icon_url && <img src={item.icon_url} alt="" loading="lazy" />}
                        <span className="nm">{item.name}</span>
                        <span className="px tm-mono">{item.lowest_price}</span>
                      </button>
                      {/* 窄窗时右栏收进这里（CSS 只在窄容器里显示它） */}
                      {item.id === selectedId && (
                        <div className="tm-inline-detail">
                          <HudItemDetail
                            item={item}
                            detail={itemDetail.details[item.id]}
                            loading={itemDetail.loadingId === item.id}
                            error={itemDetail.openId === item.id ? itemDetail.error : ""}
                            onLoad={() => itemDetail.toggle(item.id)}
                            onDrops={() => jumpToDrops(item)}
                          />
                        </div>
                      )}
                    </React.Fragment>
                  ))}
                </div>
              </>
            )}

            {/* ---- 掉落 ---- */}
            {mode === "drop" &&
              (drops ? (
                <div className="tm-scroll" key={runId}>
                  <div className="tm-th tm-mono" style={{ justifyContent: "space-between" }}>
                    <span>
                      「{dropsDisplay || drops.meta.keyword}」· {drops.meta.total} 只怪 · 命中掉落 {drops.meta.dropTotal} 条
                    </span>
                    {drops.matches.mobs.length > 0 && <span>名字命中 {drops.matches.mobs.length}</span>}
                  </div>

                  {drops.results.length === 0 && (
                    <div className="tm-empty">
                      <span>没查到「{dropsDisplay || drops.meta.keyword}」的掉落资料</span>
                      <span style={{ fontSize: 10 }}>换个说法试试：全名或直接填 ID</span>
                    </div>
                  )}

                  {drops.results.map((hit, hitIndex) => {
                    const matched = hit.drops.filter((drop) => drop.matched);
                    const expanded = !!expandedMobs[hit.mob.mobId];
                    return (
                      <div
                        key={`${hit.mob.mobId}-${hitIndex}`}
                        className="tm-mob tm-row-in"
                        style={{ "--i": Math.min(hitIndex, 10) } as React.CSSProperties}
                      >
                        <button
                          type="button"
                          className="tm-row"
                          style={{ borderTop: 0, minHeight: 40 }}
                          onClick={() =>
                            setExpandedMobs((prev) => ({ ...prev, [hit.mob.mobId]: !prev[hit.mob.mobId] }))
                          }
                          title={expanded ? "收起掉落" : "展开这只怪的掉落与出没地图"}
                        >
                          {expanded ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
                          {hit.mob.icon ? (
                            <img src={hit.mob.icon} alt="" width={28} height={28} loading="lazy" />
                          ) : (
                            <Sword size={16} style={{ color: "var(--tm-dm)" }} />
                          )}
                          <span style={{ flex: 1, minWidth: 0 }}>
                            <span style={{ display: "flex", gap: 6, alignItems: "center", fontWeight: 700 }}>
                              <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                                {hit.mob.name}
                              </span>
                              {hit.mob.boss && <span className="tm-tag boss">BOSS</span>}
                            </span>
                            <span className="tm-mono" style={{ display: "block", color: "var(--tm-mu)", fontSize: 10 }}>
                              Lv.{hit.mob.level || "-"} · HP {hit.mob.hp || "-"} · EXP {hit.mob.exp || "-"} · 掉落{" "}
                              {hit.drops.length}
                            </span>
                          </span>
                          {matched.length > 0 && (
                            <span style={{ textAlign: "right", flex: "none" }}>
                              <span className="tm-mono" style={{ display: "block", color: "var(--tm-wn)", fontWeight: 700 }}>
                                {matched[0].chanceText}
                              </span>
                              <span style={{ display: "block", color: "var(--tm-dm)", fontSize: 9 }}>
                                {matched[0].item.name}
                              </span>
                            </span>
                          )}
                        </button>

                        {expanded && (
                          <div className="tm-mob-body">
                            {hit.drops.length === 0 && (
                              <span style={{ color: "var(--tm-dm)", fontSize: 11 }}>站点没有记录这只怪物的掉落。</span>
                            )}
                            {hit.drops.map((drop, index) => (
                              <HudDropRow
                                key={`${drop.item.itemId}-${index}`}
                                drop={drop}
                                onOpen={openSite}
                                onPrice={jumpToPrice}
                              />
                            ))}
                            {hit.maps.length > 0 && (
                              <div style={{ paddingTop: 4 }}>
                                <div style={{ color: "var(--tm-mu)", fontSize: 10, display: "flex", gap: 4, alignItems: "center", marginBottom: 4 }}>
                                  <MapPin size={11} />
                                  出没地图 {hit.maps.length} 个
                                </div>
                                <div style={{ display: "flex", flexWrap: "wrap", gap: 4 }}>
                                  {hit.maps.slice(0, HUD_MAPS_PREVIEW).map((map) => (
                                    <button
                                      key={map.mapId}
                                      type="button"
                                      className="tm-chip"
                                      onClick={() => openSite(map.pageUrl)}
                                      title={`地图 ID ${map.mapId} · 点开在小册子查看`}
                                    >
                                      {map.name}
                                    </button>
                                  ))}
                                  {hit.maps.length > HUD_MAPS_PREVIEW && (
                                    <span style={{ color: "var(--tm-dm)", fontSize: 10, alignSelf: "center" }}>
                                      等 {hit.maps.length} 个
                                    </span>
                                  )}
                                </div>
                              </div>
                            )}
                            <button type="button" className="tm-link" style={{ textAlign: "left" }} onClick={() => openSite(hit.mob.pageUrl)}>
                              在小册子看这只怪的图鉴 ↗
                            </button>
                          </div>
                        )}
                      </div>
                    );
                  })}

                  {/* 翻页：站点一页只给 12 只怪，不接这一段用户会以为只有第一页那么多 */}
                  {drops.meta.totalPages > 1 && (
                    <div className="tm-th tm-mono" style={{ justifyContent: "space-between", borderTop: "1px solid var(--tm-line)" }}>
                      <span>
                        第 {dropsPage} / {drops.meta.totalPages} 页 · 已显示 {drops.results.length} 只
                      </span>
                      {dropsPage < drops.meta.totalPages && (
                        <button
                          type="button"
                          className="tm-link"
                          disabled={dropsLoading}
                          onClick={() => handleSearchDrops(dropsPage + 1)}
                        >
                          {dropsLoading ? "加载中…" : "加载更多 ›"}
                        </button>
                      )}
                    </div>
                  )}
                </div>
              ) : (
                !errorMsg && (
                  <div className="tm-empty">
                    <Sword size={26} />
                    <span>{dropsLoading ? "查询中…" : "输入怪物名 / 道具名 / ID，回车查掉落"}</span>
                    {!dropsLoading && (
                      <span style={{ fontSize: 10 }}>
                        搜「锅盖」→ 哪几只怪掉它 · 搜「无魂猴」→ 它的数据和掉落
                      </span>
                    )}
                  </div>
                )
              ))}

            {/* ---- 查人 ---- */}
            {mode === "player" &&
              (verdict ? (
                <div className="tm-scroll">
                  <div className={`tm-verdict ${verdict.status}`}>
                    <div style={{ display: "flex", flexWrap: "wrap", alignItems: "center", gap: 8 }}>
                      {verdict.status === "clear" ? <ShieldCheck size={16} /> : <ShieldAlert size={16} />}
                      <b style={{ fontSize: 13 }}>{PLAYER_VERDICT_LABEL[verdict.status]}</b>
                      <span className="tm-tag tm-mono">{verdict.checked_name}</span>
                      {verdict.checked_server && (
                        <span className="sub">
                          {verdict.checked_server} · 库内 {verdict.total_local} 条
                        </span>
                      )}
                    </div>
                    <div className="sub">{verdict.note}</div>
                    {verdict.entry && (
                      <div style={{ border: "1px solid var(--tm-line)", borderRadius: 4, padding: "6px 8px", fontSize: 11, color: "var(--tm-tx)" }}>
                        <div style={{ display: "flex", flexWrap: "wrap", gap: 8, alignItems: "center" }}>
                          <b>{verdict.entry.player_name}</b>
                          <span className="tm-tag">{verdict.entry.server}</span>
                          <span className="tm-tag boss">{verdict.entry.category}</span>
                          <span className="tm-mono" style={{ color: "var(--tm-dm)" }}>
                            {verdict.entry.created_at}
                          </span>
                        </div>
                        <div style={{ marginTop: 4 }}>原因：{verdict.entry.reason}</div>
                      </div>
                    )}
                    {verdict.related.length > 0 && verdict.status !== "hit" && (
                      <ul className="sub" style={{ margin: 0, paddingLeft: 16 }}>
                        {verdict.related.slice(0, 3).map((entry) => (
                          <li key={entry.id ?? entry.player_name}>
                            {entry.player_name}（{entry.server}）· {entry.category}
                          </li>
                        ))}
                      </ul>
                    )}
                    <div className="sub">{PLAYER_VERDICT_HINT[verdict.status]}</div>
                  </div>
                </div>
              ) : (
                !errorMsg && (
                  <div className="tm-empty">
                    <ShieldQuestion size={26} />
                    <span>{checking ? "比对中…" : "输入角色名按下回车，立刻比对本地黑名单库"}</span>
                    {!checking && <span style={{ fontSize: 10 }}>按当前预设区服比对；记录在别的服只算「注意」</span>}
                  </div>
                )
              ))}
          </div>

          {/* 右栏（宽窗）：选中项的价格与图鉴详情 */}
          {mode === "price" && selected && (
            <aside className="tm-detail">
              <div key={selected.id} className="anim-fade">
              <HudItemDetail
                item={selected}
                detail={itemDetail.details[selected.id]}
                loading={itemDetail.loadingId === selected.id}
                error={itemDetail.openId === selected.id ? itemDetail.error : ""}
                onLoad={() => itemDetail.toggle(selected.id)}
                onDrops={() => jumpToDrops(selected)}
              />
              </div>
            </aside>
          )}
        </div>

        {/* 页脚 */}
        <div className="tm-foot tm-mono">
          <span className="src">
            {mode === "price" && source === "live" && (
              <>
                <span style={{ color: "var(--tm-ac)" }}>●</span> 实时抓取 · {items.length} 条
              </>
            )}
            {mode === "price" && source === "cache" && (
              <>
                <span style={{ color: "var(--tm-in)" }}>●</span> 本地缓存 {formatAge(cacheAge)}前 · {items.length} 条
              </>
            )}
            {mode === "price" && !source && <span>结果会标明：实时 / 缓存</span>}
            {mode === "price" && (
              <button
                type="button"
                className="tm-link"
                onClick={() => handleSearch(true)}
                disabled={loading || !keyword.trim()}
                title="忽略本地缓存，重新向小册子抓取一次"
                style={{ display: "inline-flex", alignItems: "center", gap: 3 }}
              >
                <RefreshCw size={10} />
                强制刷新
              </button>
            )}
            {mode !== "price" && <span>数据来自 冒险岛怀旧服小册子</span>}
            {build && (
              <span className="hint" title={`${build.exe_path}\n构建于 ${build.built_at}`}>
                构建 {build.built_at.slice(5, 16)}
              </span>
            )}
          </span>
          <span className="hint">
            <span className="tm-kbd">Esc</span> 或 <span className="tm-kbd">{hotkeyHint}</span> 收起
          </span>
        </div>
      </div>
    </div>
  );
};
