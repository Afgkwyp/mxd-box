import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { HotkeyStatus } from "../types";

/**
 * 某个悬浮窗快捷键**实际在用**的写法。
 *
 * 这个 hook 存在的唯一理由是：界面上凡是提到快捷键的地方，都必须说真话。
 * 已经栽过两次同样的跟头 ——
 *
 * 1. 侧栏底部那个「呼出 / 收起查价窗 (Alt+F)」按钮：只在窗口挂载时读一次，而主窗口
 *    从程序启动就一直挂着，改键发生在设置页里，于是括号里永远停在旧值（用户改了键
 *    它还在写 Alt+F）。那个按钮最后被直接删掉了。
 * 2. 主窗口黑名单页的一句提示里写死了 `Alt+F`，同一个毛病。
 *
 * 所以这里做两件事：先问一次当前值，再订阅后端的 `hotkeys-changed` 广播跟到底。
 * 拿不到就退回 `fallback`（本版默认值）—— 宁可显示默认值，也不能显示一个已经不成立的键。
 */
export function useHotkey(
  action: HotkeyStatus["action"],
  fallback: string
): HotkeyStatus["hotkey"] {
  const [hotkey, setHotkey] = useState(fallback);

  useEffect(() => {
    let alive = true;

    const apply = (statuses: HotkeyStatus[]) => {
      // 只认长得对的载荷：事件名分发错了（或字段改名）时不要让界面显示 `undefined`
      if (!Array.isArray(statuses) || !alive) return;
      const mine = statuses.find((status) => status?.action === action);
      if (mine?.hotkey) setHotkey(mine.hotkey);
    };

    invoke<HotkeyStatus[]>("get_hotkey_status").then(apply).catch(() => {});
    const unlisten = listen<HotkeyStatus[]>("hotkeys-changed", (event) => apply(event.payload));

    return () => {
      alive = false;
      unlisten.then((off) => off());
    };
  }, [action]);

  return hotkey;
}
