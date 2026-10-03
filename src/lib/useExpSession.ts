import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { ExpStatus } from "../types";

/**
 * 经验统计的实时状态 + 开始 / 暂停 / 继续 / 结束 / 确认计入历史。
 * 经验窗（F10）和主窗口总览页共用这一份，两处行为不会各改各的。
 *
 * ## 按钮必须「立刻有反应」
 *
 * 几秒才采一次样，所以**不能等 `exp-update` 事件回来再更新界面**：
 * 1. 点下去**先按预期改本地状态**（乐观更新），界面当帧就变；
 * 2. 同时把命令发出去，用**命令的返回值**（后端直接把新状态回给我们）覆盖掉；
 * 3. 事件仍然在监听，只是它现在是「兜底」而不是唯一来源。
 *
 * 开始 / 暂停 / 结束**不吃冷却**：它们是本地状态切换，不发任何网络请求，
 * 套冷却会让「暂停之后马上点开始」被自己拦掉。只用 `busy` 防重入。
 */
export function useExpSession() {
  const [exp, setExp] = useState<ExpStatus | null>(null);
  const [busy, setBusy] = useState(false);
  /** 后端拒绝时的原话（比如「还定不出等级」）—— 不能吞，吞了就变成「点了没反应」 */
  const [actionError, setActionError] = useState("");

  /** 立刻要一次最新状态（命令的返回值也走它，保证界面当帧就更新）。 */
  const refresh = useCallback(() => {
    invoke<ExpStatus>("get_exp_status")
      .then(setExp)
      .catch(() => {});
  }, []);

  useEffect(() => {
    refresh();
    const unlisten = listen<ExpStatus>("exp-update", (event) => setExp(event.payload));
    return () => {
      unlisten.then((off) => off());
    };
  }, [refresh]);

  /**
   * ⚠️ 暂停之后要走的是 `resume_exp_session`，**不是** `start_exp_session`：
   * 后端的 `start()` 在「已暂停」时会直接拒绝（`已经在统计里了`），
   * 用错命令的表现就是「点了没反应」。所以按状态选命令，并且把后端返回的错误**显示出来**。
   */
  const run = useCallback(
    (
      command: "start_exp_session" | "resume_exp_session" | "pause_exp_session" | "end_exp_session",
      optimisticPhase: "running" | "paused" | "idle",
    ) => {
      if (busy) return;
      setBusy(true);
      setActionError("");
      setExp((prev) => (prev ? { ...prev, phase: optimisticPhase } : prev));
      const promise =
        command === "end_exp_session"
          ? invoke(command).then(() => invoke<ExpStatus>("get_exp_status"))
          : invoke<ExpStatus>(command);
      promise
        .then((status) => setExp(status as ExpStatus))
        .catch((err: unknown) => {
          setActionError(typeof err === "string" ? err : String(err));
          refresh();
        })
        .finally(() => setBusy(false));
    },
    [busy, refresh],
  );

  /** 播放键该发哪条命令：未开始 → start，已暂停 → resume，统计中 → pause。 */
  const toggle = useCallback(() => {
    if (exp?.phase === "running") run("pause_exp_session", "paused");
    else if (exp?.phase === "paused") run("resume_exp_session", "running");
    else run("start_exp_session", "running");
  }, [exp?.phase, run]);

  const end = useCallback(() => run("end_exp_session", "idle"), [run]);

  /** 「待确认」的那一段：计入历史 / 丢弃。 */
  const resolve = useCallback(
    (save: boolean) => {
      if (busy) return;
      setBusy(true);
      setActionError("");
      invoke("resolve_exp_session", { save, mapName: exp?.map_name ?? "" })
        .then(() => invoke<ExpStatus>("get_exp_status"))
        .then(setExp)
        .catch((err: unknown) => {
          setActionError(typeof err === "string" ? err : String(err));
          refresh();
        })
        .finally(() => setBusy(false));
    },
    [busy, exp?.map_name, refresh],
  );

  return { exp, busy, actionError, toggle, end, resolve, refresh };
}
