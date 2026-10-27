import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { BuffStatus } from "../types";

/**
 * BUFF 识别的实时状态：先问一次当前值，之后跟着后端的 `buff-status` 事件走
 * （后端只在状态变了才发，约 2 次 / 秒）。主窗口的 BUFF 卡片和经验窗都用它。
 */
export function useBuffStatus(): BuffStatus | null {
  const [status, setStatus] = useState<BuffStatus | null>(null);
  useEffect(() => {
    let alive = true;
    invoke<BuffStatus>("get_buff_status")
      .then((value) => alive && setStatus(value))
      .catch(() => {});
    const unlisten = listen<BuffStatus>("buff-status", (event) => setStatus(event.payload));
    return () => {
      alive = false;
      unlisten.then((stop) => stop());
    };
  }, []);
  return status;
}
