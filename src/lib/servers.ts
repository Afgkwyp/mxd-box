import { invoke } from "@tauri-apps/api/core";
import type { ServerInfo } from "../types";

/**
 * 区服清单的唯一数据源在 Rust 侧（`mxdc_client::SERVERS`）。
 *
 * 真实只有 5 个服：蓝蜗牛 / 蘑菇仔 / 绿水灵 / 漂漂猪 / 小白兔，
 * 而且**站点没有「全服」这个选项** —— 之前 UI 里各写一份、还多出一个
 * 「全服=1」，于是既漏掉蓝蜗牛，又让「全服综合比价」实际在查蓝蜗牛。
 *
 * 这里做一次性缓存，避免每个组件各请求一次。
 */
let pending: Promise<ServerInfo[]> | null = null;

export function loadServers(): Promise<ServerInfo[]> {
  if (!pending) {
    pending = invoke<ServerInfo[]>("get_server_list").catch((err) => {
      // 失败时清掉缓存，下次挂载可以重试
      pending = null;
      throw err;
    });
  }
  return pending;
}
