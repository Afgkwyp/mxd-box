import React, { useEffect, useState } from "react";
import { Package, ShieldAlert, Sword } from "lucide-react";
import { MarketSearch, type MarketSeed } from "../components/MarketSearch";
import { MobDrops } from "../components/MobDrops";
import { BlacklistManager } from "../components/BlacklistManager";
import { SubTabs, type TabDef } from "../components/ui/kit";

/**
 * 查询：原来的「拍卖行物价查询器」「怪物掉落速查」「避坑黑名单库」三个页签合成一页，
 * 用页内分段切换 —— 和查价窗（Alt+F）的 查价 / 掉落 / 查人 是同一套心智。
 *
 * 三块内容本身原样保留（各自的状态、缓存、冷却都不动），切走再切回来会重新挂载，
 * 和以前在侧栏里切页签的行为一致。
 */
export type QueryTab = "price" | "drop" | "player";

const TABS: TabDef<QueryTab>[] = [
  { key: "price", label: "物价", icon: <Package className="w-3.5 h-3.5" /> },
  { key: "drop", label: "掉落", icon: <Sword className="w-3.5 h-3.5" /> },
  { key: "player", label: "查人 · 黑名单库", icon: <ShieldAlert className="w-3.5 h-3.5" /> },
];

export const QueryPage: React.FC<{ seed?: MarketSeed }> = ({ seed }) => {
  const [tab, setTab] = useState<QueryTab>("price");

  // 总览点「最近查价」带词进来：切到物价页，MarketSearch 自己接手去查
  useEffect(() => {
    if (seed?.term) setTab("price");
  }, [seed?.nonce, seed?.term]);

  return (
    <div className="space-y-4">
      <SubTabs tabs={TABS} value={tab} onChange={setTab} />
      {tab === "price" && <MarketSearch seed={seed} />}
      {tab === "drop" && <MobDrops />}
      {tab === "player" && <BlacklistManager />}
    </div>
  );
};
