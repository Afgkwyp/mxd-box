import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { MemoryPayload } from "../types";

/**
 * 内存占用采样：先问一次当前值，之后跟着 `memory-update` 事件走。
 *
 * 为什么必须「先问一次」：事件是主循环每 2 秒发一次的，窗口刚打开那两秒里
 * 如果只等事件，界面就只能先摆一个**编造的**数字（曾经写死成 16384MB / 44%），
 * 而且看上去和真数据一模一样 —— 这恰恰是这个工具最不该做的事（它本身就是来报
 * 内存的）。所以拿不到数据时返回 `null`，由界面显示「读取中…」。
 */
export function useMemorySample(): MemoryPayload | null {
  const [memory, setMemory] = useState<MemoryPayload | null>(null);

  useEffect(() => {
    let alive = true;
    invoke<MemoryPayload | null>("get_memory_status")
      .then((payload) => {
        if (alive && payload && typeof payload.percent === "number") setMemory(payload);
      })
      .catch(() => {});

    const unlisten = listen<MemoryPayload>("memory-update", (event) => {
      // 只接受长得像采样的载荷：如果哪天某个事件带上别的字段（或字段变名），
      // 直接写进状态会让界面在 `percent.toFixed()` 上崩掉（悬浮窗会变空白），
      // 而正确的做法是“不认识的就不认”。
      const payload = event.payload;
      if (payload && typeof payload.percent === "number") {
        setMemory(payload);
      }
    });

    return () => {
      alive = false;
      unlisten.then((off) => off());
    };
  }, []);

  return memory;
}
