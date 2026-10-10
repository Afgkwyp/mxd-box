import React, { useEffect, useState } from "react";
import { Package, ShieldAlert, Sword } from "lucide-react";
import { COOLDOWN, peekGuard } from "../lib/throttle";
import { MarketSearch, type MarketSeed } from "../components/MarketSearch";
import { MobDrops, type DropSeed } from "../components/MobDrops";
import { BlacklistManager } from "../components/BlacklistManager";
import { SubTabs, type TabDef } from "../components/ui/kit";

/**
 * 查询：原来的「拍卖行物价查询器」「怪物掉落速查」「避坑黑名单库」三个页签合成一页，
 * 用页内分段切换 —— 和查价窗（Alt+F）的 查价 / 掉落 / 查人 是同一套心智。
 *
 * 「物价」「掉落」互相跳转（点掉落行的「查价」/ 结果卡的「谁掉」），所以这两块
 * **保持挂载、只切显示**：看一眼价格再回来接着看掉落表，结果和展开状态都还在。
 * 「查人」和联动无关，维持切走就卸载的旧行为。
 */
export type QueryTab = "price" | "drop" | "player";

const TABS: TabDef<QueryTab>[] = [
  { key: "price", label: "物价", icon: <Package className="w-3.5 h-3.5" /> },
  { key: "drop", label: "掉落", icon: <Sword className="w-3.5 h-3.5" /> },
  { key: "player", label: "查人 · 黑名单库", icon: <ShieldAlert className="w-3.5 h-3.5" /> },
];

export const QueryPage: React.FC<{ seed?: MarketSeed }> = ({ seed }) => {
  const [tab, setTab] = useState<QueryTab>("price");
  /** 页内联动 seed：点「查价」/「谁掉」带词进另一个页签 */
  const [priceLink, setPriceLink] = useState<MarketSeed | undefined>();
  const [dropLink, setDropLink] = useState<DropSeed | undefined>();

  // 总览点「最近查价」带词进来：切到物价页，MarketSearch 自己接手去查
  useEffect(() => {
    if (seed?.term) setTab("price");
  }, [seed?.nonce, seed?.term]);

  // 外部 seed（总览「最近查价」）和页内联动 seed 合并成一个来源：谁的 nonce 新用谁。
  // MarketSearch 的 effect 只认 nonce 变化，所以合并完仍然只触发真正新的一次。
  const priceSeed =
    seed && (!priceLink || seed.nonce >= priceLink.nonce) ? seed : priceLink;

  /** 「掉落 → 查价」：先过闸，被拦下就只弹提示、页签和输入框都不动（方案 5.4）。 */
  const searchPrice = (name: string) => {
    if (!peekGuard("query_market", "拍卖查询", COOLDOWN.query)) return;
    setPriceLink({ term: name, nonce: Date.now() });
    setTab("price");
  };

  /** 「查价 → 掉落」：`term` 是输入框里显示的名字，`itemId` 是实际查询词（更准，见方案 5.1）。 */
  const searchDrops = (term: string, itemId?: string) => {
    if (!peekGuard("search_drops", "掉落速查", COOLDOWN.query)) return;
    setDropLink({ term, query: itemId, nonce: Date.now() });
    setTab("drop");
  };

  return (
    <div className="space-y-4">
      <SubTabs tabs={TABS} value={tab} onChange={setTab} />
      <div hidden={tab !== "price"}>
        <MarketSearch seed={priceSeed} onSearchDrops={searchDrops} />
      </div>
      <div hidden={tab !== "drop"}>
        <MobDrops seed={dropLink} onSearchPrice={searchPrice} />
      </div>
      {tab === "player" && <BlacklistManager />}
    </div>
  );
};
