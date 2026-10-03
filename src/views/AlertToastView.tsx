import React, { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { BellRing, Coins, ExternalLink, Rocket, ShieldAlert, X } from "lucide-react";
import type { AlertPayload } from "../types";

/**
 * 提醒窗（`?window=alert`）—— 开服提醒与「剪贴板命中黑名单」都走它。
 *
 * 这是**唯一一个 100% 由我们控制的可见提醒通道**，也是这次真正把「点测试只听得到
 * 声音、看不到东西」修掉的东西：这个工具箱是未安装版（没有开始菜单快捷方式、
 * 没有注册 AppUserModelID），Windows 会把原生 Toast **静默丢掉** —— `show()`
 * 返回成功、日志里一个错都没有、屏幕上一片安静。只能自己画一个。
 *
 * 两条取内容的路径（必须都有）：
 *   1. 挂载时 `get_latest_open_alert` 主动拉一次 —— 提醒窗是「先建窗、后加载
 *      页面」，Rust 那边发事件时这个页面往往还没开始监听，那条事件一定会丢；
 *   2. 之后监听 `open-alert` —— 窗已经开着时再来一次提醒，直接换内容即可。
 *
 * 界面刻意做得极简：这个窗会在游戏全屏（或无边框窗口）状态下盖在右下角，
 * 三秒内要能读完「开服了 + 谁说的 + 该干嘛」。
 */
export const AlertToastView: React.FC = () => {
  const [payload, setPayload] = useState<AlertPayload | null>(null);

  useEffect(() => {
    invoke<AlertPayload | null>("get_latest_open_alert")
      .then((data) => {
        if (data) setPayload(data);
      })
      .catch(() => {});

    const unlisten = listen<AlertPayload>("open-alert", (event) => {
      setPayload(event.payload);
    });

    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  const dismiss = () => {
    invoke("dismiss_open_alert").catch(() => {});
  };

  const openMain = () => {
    invoke("focus_main_window")
      .catch(() => {})
      .finally(dismiss);
  };

  const isTest = payload?.source === "test";
  // 四类提醒共用一个窗：测试 / 开服 / 剪贴板命中黑名单 / 金价到价
  const isBlacklist = payload?.source === "clipboard";
  const isMeso = payload?.source === "meso";

  return (
    <div className="h-full w-full p-2">
      {/* 无边框窗口的拖拽区 */}
      <div
        data-tauri-drag-region
        className={`h-full w-full rounded-2xl border bg-slate-950 overflow-hidden flex flex-col ${
          isBlacklist ? "border-red-500/50" : "border-amber-500/40"
        }`}
      >
        <div
          data-tauri-drag-region
          className="px-4 pt-3 pb-2 flex items-start gap-2.5 border-b border-white/10"
        >
          <div className="w-8 h-8 shrink-0 rounded-xl bg-amber-500/20 border border-amber-500/40 flex items-center justify-center">
            {isTest ? (
              <BellRing className="w-4 h-4 text-amber-300" />
            ) : isBlacklist ? (
              <ShieldAlert className="w-4 h-4 text-red-300" />
            ) : isMeso ? (
              <Coins className="w-4 h-4 text-amber-300" />
            ) : (
              <Rocket className="w-4 h-4 text-amber-300" />
            )}
          </div>

          <div className="min-w-0 flex-1">
            <div className="text-sm font-extrabold text-amber-200 leading-tight">
              {payload?.title ?? "提醒"}
            </div>
            <div className="text-[11px] text-slate-500 mt-0.5">
              {payload ? `${payload.source_label} · ${payload.at}` : "正在读取提醒内容…"}
            </div>
          </div>

          <button
            onClick={dismiss}
            title="知道了（关掉这个提醒窗）"
            className="shrink-0 p-1 rounded-lg text-slate-500 hover:text-slate-200 hover:bg-white/10 transition-colors cursor-pointer"
          >
            <X className="w-3.5 h-3.5" />
          </button>
        </div>

        <div className="flex-1 px-4 py-3 overflow-y-auto">
          <p className="text-xs text-slate-200 leading-relaxed whitespace-pre-line">
            {payload?.body ?? "这次提醒的内容已经取不到了，点「知道了」关掉即可。"}
          </p>
        </div>

        <div className="px-4 py-2.5 border-t border-white/10 flex items-center gap-2">
          <button
            onClick={openMain}
            className="flex-1 py-1.5 rounded-lg bg-amber-500 hover:bg-amber-400 text-slate-950 text-xs font-bold flex items-center justify-center gap-1.5 cursor-pointer"
          >
            <ExternalLink className="w-3.5 h-3.5" />
            打开枫之助
          </button>
          <button
            onClick={dismiss}
            className="px-3 py-1.5 rounded-lg bg-slate-800 hover:bg-slate-700 text-slate-200 border border-white/15 text-xs font-semibold cursor-pointer"
          >
            知道了
          </button>
        </div>

        <div className="px-4 pb-2 text-[10px] text-slate-600">
          20 秒后自动关闭 · 同一个事件只提醒一次
        </div>
      </div>
    </div>
  );
};
