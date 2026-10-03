import React, { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { VersionDialog } from "./VersionDialog";
import { RELEASES, type ReleaseNote } from "../lib/changelog";

/**
 * 换版本时弹一次的更新说明。
 *
 * 该不该弹由**后端**决定（`take_version_greeting`：比对「上次见过的版本」和当前版本，
 * 一样就不弹）。放后端是为了「只弹一次」不依赖前端的执行次数 —— 开窗、WebView 重载
 * 都会跑一遍挂载逻辑。
 *
 * 从 MainView 里拆出来，是为了让外壳只管布局。
 */
export const UpdateNotesGreeting: React.FC = () => {
  const [greeting, setGreeting] = useState<ReleaseNote | null>(null);

  useEffect(() => {
    invoke<string | null>("take_version_greeting")
      .then((version) => {
        if (!version) return;
        const release = RELEASES.find((item) => item.version === version);
        // 后端说版本变了，但这份说明里没有这一版（比如刚发的版本忘了写）：
        // 那就别弹一个空窗，只记一条日志。
        if (release) setGreeting(release);
        else console.warn(`没有 v${version} 的更新说明`);
      })
      .catch(() => {});
  }, []);

  return greeting ? <VersionDialog release={greeting} onClose={() => setGreeting(null)} /> : null;
};
