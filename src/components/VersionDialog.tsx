import React, { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Download, Sparkles, X } from "lucide-react";
import type { UpdateInfo } from "../types";
import { COOLDOWN, guard } from "../lib/throttle";
import type { ReleaseNote } from "../lib/changelog";

/**
 * 启动时的「这一版更新了什么」弹窗。
 *
 * **只在版本真的变了的时候弹**（包括第一次安装）。判断放在后端
 * （`take_version_greeting`）：它比对本地记着的「上次见过的版本」和当前版本，
 * 不一样就返回当前版本号并记下来 —— 所以这个窗一辈子只在换版本时出现一次，
 * 不会每次开都烦人。放后端还有一个好处：只弹一次这件事**不依赖前端的执行次数**
 * （WebView 重载、多窗口都不会让它弹第二遍）。
 *
 * 内容只列主要的功能更新和修掉的问题，不写细节。
 */
export const VersionDialog: React.FC<{
  release: ReleaseNote;
  onClose: () => void;
}> = ({ release, onClose }) => {
  const [downloading, setDownloading] = useState(false);
  const [downloadMessage, setDownloadMessage] = useState<{
    text: string;
    kind: "success" | "error" | "info";
  } | null>(null);

  const downloadUpdate = async () => {
    if (downloading || !guard("check_for_update", "版本自检", COOLDOWN.heavy)) return;
    setDownloading(true);
    setDownloadMessage(null);
    try {
      const info = await invoke<UpdateInfo>("check_for_update");
      if (info.error) {
        setDownloadMessage({ kind: "error", text: `没能确认有没有新版本：${info.error}` });
        return;
      }
      if (!info.has_update || !info.download_url) {
        setDownloadMessage({ kind: "info", text: "当前没有可下载的新版本。" });
        return;
      }
      const path = await invoke<string>("download_update", { url: info.download_url });
      setDownloadMessage({ kind: "success", text: `已保存到：${path}` });
    } catch (err: unknown) {
      setDownloadMessage({
        kind: "error",
        text: typeof err === "string" ? err : `下载失败：${String(err)}`,
      });
    } finally {
      setDownloading(false);
    }
  };

  const closeWhenIdle = () => {
    if (!downloading) onClose();
  };

  return (
  <div
    className="anim-fade fixed inset-0 z-[999] flex items-center justify-center bg-slate-950/60 p-4"
    onClick={closeWhenIdle}
  >
    <div
      className="anim-pop w-full max-w-md rounded-2xl border border-amber-500/30 bg-slate-900 shadow-2xl overflow-hidden"
      onClick={(event) => event.stopPropagation()}
    >
      <div className="flex items-start justify-between gap-3 px-5 pt-4 pb-2">
        <div>
          <div className="flex items-center gap-2">
            <Sparkles className="w-4 h-4 text-amber-400" />
            <span className="text-sm font-bold text-slate-100">
              已更新到 v{release.version}
            </span>
          </div>
          <p className="mt-1 text-xs text-slate-400">{release.summary}</p>
        </div>
        <button
          type="button"
          onClick={closeWhenIdle}
          disabled={downloading}
          title="关闭"
          className="p-0.5 rounded text-slate-500 hover:text-slate-200 hover:bg-white/10 disabled:opacity-40 cursor-pointer"
        >
          <X className="w-4 h-4" />
        </button>
      </div>

      <ul className="px-5 py-3 space-y-1.5">
        {release.items.map((item) => (
          <li key={item} className="flex items-start gap-2 text-[13px] text-slate-300">
            <span className="mt-1.5 w-1 h-1 rounded-full bg-amber-400/80 shrink-0" />
            <span>{item}</span>
          </li>
        ))}
      </ul>

      <div className="px-5 py-3 border-t border-white/10 space-y-2">
        {downloadMessage && (
          <p
            className={`text-[11px] break-all ${
              downloadMessage.kind === "success"
                ? "text-emerald-300"
                : downloadMessage.kind === "error"
                  ? "text-red-300"
                  : "text-slate-400"
            }`}
            role="status"
          >
            {downloadMessage.text}
          </p>
        )}
        <div className="flex justify-end gap-2">
          <button
            type="button"
            onClick={downloadUpdate}
            disabled={downloading}
            className="px-3 py-1.5 rounded-lg border border-amber-500/40 text-amber-300 hover:bg-amber-500/10 text-xs font-semibold disabled:opacity-50 cursor-pointer"
          >
            <Download className="w-3.5 h-3.5 inline mr-1" />
            {downloading ? "正在下载…" : "下载到桌面"}
          </button>
          <button
            type="button"
            onClick={closeWhenIdle}
            disabled={downloading}
            className="px-4 py-1.5 rounded-lg bg-amber-500 hover:bg-amber-400 text-xs font-bold text-slate-950 disabled:opacity-40 cursor-pointer"
          >
            知道了
          </button>
        </div>
      </div>
    </div>
  </div>
  );
};
