import React, { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Cpu, Keyboard, Layers, AlertTriangle } from "lucide-react";
import { HotkeyInput } from "./HotkeyInput";
import { InlineNotice, type Notice } from "./Notice";
import type { AppSettings, HotkeyStatus } from "../types";

/**
 * 「悬浮窗与内存」—— 打游戏时随手会调的那几样。
 *
 * 从偏好设置里搬出来的（原来三组挤在一个页面里靠切换卡片选，用户的原话是
 * 「感觉像是把一堆东西揉在一个地方」）。搬成左侧导航页之后这里**没有「保存」按钮**，
 * 因为这一页的每一项都是即改即生效的：
 *
 * * 内存警戒线松手就写库（`set_memory_threshold`）；
 * * 快捷键按下去就重注册（`apply_hotkey`），成败当场回显。
 *
 * 为什么不再留一个「保存全部配置」：它和「改完立刻生效」是两种心智模型，混在一起
 * 就会出现「改了键能用、改了内存要再点一次保存」这种不一致；而且设置页拆开之后，
 * 那个按钮的「全部」已经没有任何意义了 —— 它只看得见自己这一页。
 */
export const FloatSettings: React.FC = () => {
  const [settings, setSettings] = useState<AppSettings | null>(null);
  /** 失败信息：页内提示条，不用原生 alert */
  const [notice, setNotice] = useState<Notice | null>(null);
  /** 哪个输入框正在等用户按键（捕获期间全局快捷键必须先注销） */
  const [capturing, setCapturing] = useState<"price_hud" | "lite_float" | null>(null);
  /** 两个悬浮窗快捷键的注册结果：**必须显示出来**。注册失败是静默的，用户只会觉得「按了没反应」。 */
  const [hotkeyStatuses, setHotkeyStatuses] = useState<HotkeyStatus[]>([]);
  const [visibleFlags, setVisibleFlags] = useState<Record<string, boolean>>({});

  /**
   * 改键捕获期间：把全局快捷键临时注销掉，结束时再挂回去。
   *
   * 不这么做的话捕获是**坏的**：全局键由 OS 直接接管（`RegisterHotKey`），
   * 按下当前那个键时 webview 收不到 keydown、悬浮窗却被呼了出来；
   * 想「把 F10 改成别的键」的用户第一下按的就是 F10，看到的就是「没反应」。
   *
   * 用 effect 的 cleanup 保证**一定会挂回去** —— 捕获成功、Esc 取消、点到别处、
   * 直接离开这一页，全部走同一条恢复路径（后端照最近一次收到的配置注册，
   * 所以和 `apply_hotkey` 谁先到都不影响结果）。
   */
  useEffect(() => {
    if (!capturing) return;
    invoke("begin_hotkey_capture").catch(() => {});

    return () => {
      invoke<HotkeyStatus[]>("end_hotkey_capture")
        .then(setHotkeyStatuses)
        .catch(() => {});
    };
  }, [capturing]);

  useEffect(() => {
    invoke<AppSettings>("get_settings")
      .then(setSettings)
      .catch((err: unknown) => setNotice({ kind: "error", message: `读取设置失败：${err}` }));

    invoke<HotkeyStatus[]>("get_hotkey_status")
      .then(setHotkeyStatuses)
      .catch(() => {});
  }, []);

  /**
   * 内存警戒线：拖完就生效，不用等「保存」。
   *
   * 拖动过程不写库（每挪一像素都写一次 SQLite 没意义），松手才提交一次。
   */
  const commitMemoryThreshold = () => {
    if (!settings) return;
    invoke("set_memory_threshold", { threshold: settings.memory_threshold }).catch(
      (err: unknown) => setNotice({ kind: "error", message: `保存内存警戒线失败：${err}` })
    );
  };

  /**
   * 换快捷键：**立刻**重注册并回显结果。
   *
   * 这是修掉「改了键没反应」那类困惑的关键一步：注册成败当场可见。
   * 两个悬浮窗的键一起重注册，因为「两边填成同一个键」也是一种错，得能看出来。
   */
  const handleHotkeyChange = useCallback(
    async (action: "price_hud" | "lite_float", hotkey: string) => {
      setSettings((prev) =>
        prev
          ? action === "price_hud"
            ? { ...prev, hotkey }
            : { ...prev, lite_float_hotkey: hotkey }
          : prev
      );
      try {
        const statuses = await invoke<HotkeyStatus[]>("apply_hotkey", { action, hotkey });
        // 空数组 = 正处在改键模式、这次没有注册，注册结果由「捕获结束」那条返回
        if (statuses.length) setHotkeyStatuses(statuses);
      } catch (err: unknown) {
        setNotice({ kind: "error", message: `注册快捷键失败：${err}` });
      }
    },
    []
  );

  const handleTestFloat = async (action: "price_hud" | "lite_float") => {
    await invoke(action === "price_hud" ? "toggle_hud" : "toggle_lite_float").catch(() => {});
    // 窗口显示是投递到事件循环的，等一拍再回读，否则读到的永远是切换前的状态
    setTimeout(() => {
      invoke<boolean>("get_float_window_visible", { action })
        .then((visible) => setVisibleFlags((prev) => ({ ...prev, [action]: visible })))
        .catch(() => {});
    }, 350);
  };

  const statusOf = (action: "price_hud" | "lite_float") =>
    hotkeyStatuses.find((status) => status.action === action);

  return (
    <div className="space-y-5 max-w-4xl">
      <p className="text-xs text-slate-400 px-1">两个悬浮窗的呼出键、内存警戒线（改完立刻生效）</p>

      <InlineNotice notice={notice} onClose={() => setNotice(null)} />

      <div className="grid grid-cols-1 md:grid-cols-2 gap-5">
        {/* 内存警戒阈值 */}
        <div className="p-5 rounded-2xl bg-slate-900/70 border border-white/10 space-y-4">
          <div className="flex items-center gap-2 text-sm font-bold text-slate-200">
            <Cpu className="w-4 h-4 text-amber-400" />
            物理内存警戒阈值
          </div>
          <p className="text-xs text-slate-400 leading-relaxed">
            内存占用到这个比例时，主窗口侧栏与悬浮窗里的内存显示会变红，提醒你进商城刷新内存。
          </p>

          {settings ? (
            <div className="space-y-2 pt-2">
              <div className="flex items-center justify-between text-xs">
                <span className="text-slate-400">预警警戒线</span>
                <span className="text-base font-extrabold text-amber-400 tabular-nums">
                  {settings.memory_threshold}%
                </span>
              </div>
              <input
                type="range"
                min={50}
                max={95}
                step={1}
                value={settings.memory_threshold}
                onChange={(e) =>
                  setSettings({ ...settings, memory_threshold: Number(e.target.value) })
                }
                onPointerUp={commitMemoryThreshold}
                onKeyUp={commitMemoryThreshold}
                className="w-full accent-amber-500 cursor-pointer"
              />
              <div className="flex justify-between text-[11px] text-slate-500 tabular-nums">
                <span>50% (极敏感)</span>
                <span>85% (推荐默认)</span>
                <span>95% (极度吃紧)</span>
              </div>
            </div>
          ) : (
            <p className="text-xs text-slate-500">读取中…</p>
          )}
        </div>

        {/* 两个悬浮窗的快捷键（按压式捕获） */}
        <div className="p-5 rounded-2xl bg-slate-900/70 border border-white/10 space-y-5 md:col-span-2">
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-2 text-sm font-bold text-slate-200">
              <Keyboard className="w-4 h-4 text-amber-400" />
              悬浮窗呼出快捷键
            </div>
            <span className="text-[10px] px-1.5 py-0.5 rounded bg-slate-800 text-slate-400 border border-white/10">
              按压设置
            </span>
          </div>

          <p className="text-xs text-slate-400 leading-relaxed">
            全局生效。两个悬浮窗都是「按一次呼出、再按一次收起」。点输入框后直接按组合键即可。
          </p>

          {!settings && <p className="text-xs text-slate-500">读取中…</p>}

          {settings && (
            <div className="grid grid-cols-1 md:grid-cols-2 gap-5">
              {[
                {
                  action: "lite_float" as const,
                  title: "经验窗",
                  desc: "经验 / 小时、等级进度；边缘拖动缩放，双击标题栏收起成单行条",
                  value: settings.lite_float_hotkey,
                },
                {
                  action: "price_hud" as const,
                  title: "查价悬浮窗",
                  desc: "拍卖行搜索 + 物品详情 + 查黑名单",
                  value: settings.hotkey,
                },
              ].map((entry) => {
                const status = statusOf(entry.action);
                const visible = visibleFlags[entry.action];
                return (
                  <div
                    key={entry.action}
                    className="p-3 rounded-xl bg-slate-950/40 border border-white/5 space-y-2.5"
                  >
                    <div className="flex items-baseline justify-between gap-2">
                      <span className="text-xs font-bold text-slate-200">{entry.title}</span>
                      <span className="text-[10px] text-slate-500">{entry.desc}</span>
                    </div>

                    <HotkeyInput
                      value={entry.value}
                      defaultValue={status?.default_hotkey ?? ""}
                      onChange={(hotkey) => handleHotkeyChange(entry.action, hotkey)}
                      onCaptureChange={(active) => setCapturing(active ? entry.action : null)}
                    />

                    {/* 注册结果：这一行就是「为什么按了没反应」的答案 */}
                    {status && (
                      <div
                        className={`text-[11px] leading-relaxed rounded-lg p-2 border ${
                          status.registered
                            ? "bg-emerald-950/40 border-emerald-500/30 text-emerald-300"
                            : "bg-red-950/40 border-red-500/30 text-red-300"
                        }`}
                      >
                        {status.registered ? (
                          <>
                            ✅ 已注册：<span className="tabular-nums">{status.hotkey}</span>
                          </>
                        ) : (
                          <>⚠️ 没注册上：{status.error ?? "未知原因"}</>
                        )}
                      </div>
                    )}

                    <div className="flex items-center gap-2">
                      <button
                        type="button"
                        onClick={() => handleTestFloat(entry.action)}
                        className="px-2.5 py-1 rounded-lg bg-slate-800 hover:bg-slate-700 text-[11px] text-amber-300 border border-white/10 flex items-center gap-1.5 cursor-pointer"
                      >
                        <Layers className="w-3.5 h-3.5" />
                        测试呼出 / 收起
                      </button>
                      {visible !== undefined && (
                        <span className="text-[11px] text-slate-400">
                          {visible ? "已显示" : "已收起"}
                        </span>
                      )}
                    </div>
                  </div>
                );
              })}
            </div>
          )}

          <p className="text-[11px] text-slate-500 leading-relaxed">
            显示「没注册上」通常是这个键被别的程序占用了，换一个组合即可。
          </p>
        </div>
      </div>

      {hotkeyStatuses.some((status) => !status.registered) && (
        <div className="flex items-center gap-2 text-[11px] text-amber-200/80 bg-amber-950/30 border border-amber-500/25 rounded-lg p-3">
          <AlertTriangle className="w-3.5 h-3.5 shrink-0" />
          有快捷键没注册上，按了不会呼出悬浮窗；换一个组合试试。
        </div>
      )}
    </div>
  );
};
