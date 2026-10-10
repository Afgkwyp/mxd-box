import React, { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ServerMonitorCard } from "./ServerMonitorCard";
import { MesoAlertCard } from "./MesoAlertCard";
import { InlineNotice, type Notice } from "./Notice";
import type { AppSettings } from "../types";

/**
 * 「提醒」页 —— 开服提醒 + 金价到价提醒。等开服那阵子才会看的东西，从偏好设置里搬出来单独成一页。
 *
 * 开关与间隔都是**改完立刻生效**（`patch_settings` 只改传进去的那几个字段）：
 * 「等开服」这件事本身就有时间压力，让用户改完再去点一次保存、还得自己确认
 * 到底生效没有，是这一页最不该有的摩擦。
 */
export const AlertSettings: React.FC = () => {
  /** null = 还没读到（宁可在界面上写「读取中」，也不先摆一个可能是假的状态） */
  const [enabled, setEnabled] = useState<boolean | null>(null);
  const [intervalSec, setIntervalSec] = useState<number | null>(null);
  const [notice, setNotice] = useState<Notice | null>(null);

  useEffect(() => {
    invoke<AppSettings>("get_settings")
      .then((settings) => {
        setEnabled(settings.monitor_enabled);
        setIntervalSec(settings.monitor_interval_sec);
      })
      .catch((err: unknown) => setNotice({ kind: "error", message: `读取设置失败：${err}` }));
  }, []);

  const patch = async (payload: Record<string, unknown>, failure: string) => {
    try {
      await invoke("patch_settings", { patch: payload });
    } catch (err: unknown) {
      setNotice({ kind: "error", message: `${failure}：${err}` });
    }
  };

  const handleEnabledChange = (value: boolean) => {
    setEnabled(value);
    patch({ monitor_enabled: value }, "保存开服监控开关失败");
  };

  const handleIntervalChange = (value: number) => {
    setIntervalSec(value);
    patch({ monitor_interval_sec: value }, "保存拉取间隔失败");
  };

  return (
    <div className="space-y-5 max-w-4xl">
      <p className="text-xs text-slate-400 px-1">服务器真的恢复、金价到了你设的数时提醒你（改完立刻生效）</p>

      <InlineNotice notice={notice} onClose={() => setNotice(null)} />

      {enabled === null || intervalSec === null ? (
        <div className="p-5 rounded-xl bg-slate-900/40 border border-white/5 text-xs text-slate-500">
          正在读取当前设置…
        </div>
      ) : (
        <div className="flex flex-col gap-5">
          <ServerMonitorCard
            enabled={enabled}
            intervalSec={intervalSec}
            onEnabledChange={handleEnabledChange}
            onIntervalChange={handleIntervalChange}
          />

          <MesoAlertCard />

          <div className="p-4 rounded-xl bg-slate-900/40 border border-white/5 text-[11px] text-slate-400 leading-relaxed">
            所有提醒走同一个出口：响铃、任务栏闪烁、右下角弹窗。点「测试开服提醒」能听到、能看到，
            就说明开服、金价到价、内存到线、黑名单命中时一定会提醒你。
          </div>
        </div>
      )}
    </div>
  );
};
