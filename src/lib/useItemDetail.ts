import { useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { COOLDOWN, guard } from "./throttle";
import type { ItemDetail } from "../types";

interface UseItemDetail {
  /** 当前展开的那条物品 ID（null = 都收起） */
  openId: string | null;
  /** 已经取到的图鉴资料，按物品 ID 存着，反复点不会再请求 */
  details: Record<string, ItemDetail>;
  /** 正在读取的那条 */
  loadingId: string | null;
  error: string;
  toggle: (itemId: string) => void;
}

/**
 * 物价结果里的「物品详情」：
 * 点了才去取小册子图鉴页（官方基准属性 + NPC 出售价），并缓存在组件里。
 * 主窗口和 HUD 共用这一份逻辑，免得两边行为不一致。
 */
export function useItemDetail(): UseItemDetail {
  const [openId, setOpenId] = useState<string | null>(null);
  const [details, setDetails] = useState<Record<string, ItemDetail>>({});
  const [loadingId, setLoadingId] = useState<string | null>(null);
  const [error, setError] = useState("");
  /** 只认最后一次请求的序号：迟到的旧响应不许碰 loadingId / error（资料照常进缓存） */
  const seq = useRef(0);

  const toggle = async (itemId: string) => {
    if (!itemId) return;

    if (openId === itemId) {
      setOpenId(null);
      return;
    }

    // 冷却闸必须在**动 openId 之前**过：拦下时什么都不改。
    // 放在后面的话，面板已经切到新物品却拿不到资料 —— 一块空白挂着一个
    // 「收起详情」按钮、零报错，看起来像坏了（拦下本身有 ThrottleToast 提示，
    // 这里不用再报一遍）。
    if (!details[itemId]) {
      // 图鉴详情要打站点（同一个物品看一眼就够了，连点没意义）
      if (!guard("get_item_detail", "物品详情", COOLDOWN.query)) return;
    }

    const mine = ++seq.current;
    setOpenId(itemId);
    setError("");

    if (details[itemId]) return;

    setLoadingId(itemId);
    try {
      const detail = await invoke<ItemDetail>("get_item_detail", { itemId });
      setDetails((prev) => ({ ...prev, [itemId]: detail }));
    } catch (err: unknown) {
      // 已有更新的请求在飞时，这条失败不归现在展开的物品管，别把它的面板污染了
      if (seq.current !== mine) return;
      setError(typeof err === "string" ? err : "读取物品图鉴失败");
    } finally {
      // 复位也要认序号：快速「开 A → 开 B」时，A 的迟到返回不能把 B 的 loading 关掉
      if (seq.current === mine) setLoadingId(null);
    }
  };

  return { openId, details, loadingId, error, toggle };
}
