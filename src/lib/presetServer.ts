import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { loadServers } from "./servers";
import type { AppSettings, ServerInfo } from "../types";

/**
 * 全局预设区服。
 *
 * 以前主窗口的拍卖查询和悬浮窗各自 `useState(1)` 写死蓝蜗牛，两个窗口互不知道 ——
 * 用户每开一次窗都要重新选一次服，而且在悬浮窗里选完，主窗口还是老样子。
 *
 * 现在这份「在哪个服」只有一处状态（Rust 侧的 `default_server_id`）：
 * 谁改了都写回去，写完通过 `default-server-changed` 事件广播，所有窗口一起切。
 */
export function usePresetServer(): {
  serverId: number;
  servers: ServerInfo[];
  /** 切换并写回全局预设（所有窗口都会跟着切） */
  chooseServer: (id: number) => void;
  /** 预设还没读回来时为 true —— 界面用它避免「先显示蓝蜗牛再跳到你那个服」的闪烁 */
  loading: boolean;
} {
  const [servers, setServers] = useState<ServerInfo[]>([]);
  const [serverId, setServerId] = useState<number | null>(null);

  useEffect(() => {
    let cancelled = false;

    loadServers()
      .then((list) => {
        if (!cancelled) setServers(list);
      })
      .catch(() => {});

    invoke<AppSettings>("get_settings")
      .then((settings) => {
        if (!cancelled) setServerId(settings.default_server_id);
      })
      .catch(() => {
        // 读不到就用第一个服兜底，宁可显示默认值也不要把界面卡在「加载中」
        if (!cancelled) setServerId(1);
      });

    const unlisten = listen<number>("default-server-changed", (event) => {
      setServerId(event.payload);
    });

    return () => {
      cancelled = true;
      unlisten.then((f) => f());
    };
  }, []);

  const chooseServer = useCallback((id: number) => {
    // 先本地切，界面立刻有反馈；写库失败时下一次读设置会纠正回来
    setServerId(id);
    invoke("set_default_server", { serverId: id }).catch(() => {});
  }, []);

  return {
    serverId: serverId ?? 1,
    servers,
    chooseServer,
    loading: serverId === null,
  };
}
