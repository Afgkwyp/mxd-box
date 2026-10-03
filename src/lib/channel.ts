import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { ChannelState } from "../types";

/**
 * 当前频道（几线）：先问一次当前值，之后跟着 `channel-update` 事件走。
 *
 * 后端每 3 秒查一次游戏进程的 TCP 连接端口（见 `channel.rs`），只在变化时广播。
 * 窗口刚打开、事件还没发过的那几秒必须靠命令兜一次，否则界面只能空着。
 *
 * 和内存采样同一套规矩：**只接受长得像通道状态的载荷**，不认识的就不认，
 * 免得某个事件换字段后把界面写崩。
 */
export function useChannelSample(): ChannelState | null {
  const [state, setState] = useState<ChannelState | null>(null);

  useEffect(() => {
    let alive = true;
    invoke<ChannelState | null>("get_channel_state")
      .then((payload) => {
        if (alive && payload && typeof payload.source === "string") setState(payload);
      })
      .catch(() => {});

    const unlisten = listen<ChannelState>("channel-update", (event) => {
      const payload = event.payload;
      if (payload && typeof payload.source === "string") setState(payload);
    });

    return () => {
      alive = false;
      unlisten.then((off) => off());
    };
  }, []);

  return state;
}
