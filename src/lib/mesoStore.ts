import { useEffect, useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { COOLDOWN, guard } from "./throttle";
import type { MesoReport } from "../types";

/**
 * 金价数据的**共享仓库**：总览页和行情页共用同一份，不各拉各的。
 *
 * 以前两个页面各自 `invoke("get_meso_report")`，而行情页又套了 5 秒的冷却闸：
 * 总览 ⇄ 行情 来回切，第二次挂载就被「金价刷新冷却」拦下，页面还是空的。
 * 现在：
 *
 * * 数据放在模块级的一个仓库里，**切页不丢**（回到页面立刻有数据，不用等）；
 * * 同一时刻只有一个请求在飞（`inflight` 去重），多个组件同时要也只发一次；
 * * 「够新」（默认 60 秒内）就直接用现成的，不再去问后端；
 * * 冷却闸**只挡用户手动点刷新**（防连点），自动 / 挂载触发的请求不过闸 ——
 *   它们本来就有「够新就不请求」这道更聪明的闸。
 *
 * 后端本身还有 10 分钟缓存（`get_meso_report`），所以这里多问一次也不会真的多打站点。
 */

interface Snapshot {
  report: MesoReport | null;
  /** 上次成功取到的时刻（毫秒时间戳） */
  updatedAt: number | null;
  loading: boolean;
  /** 最近一次失败的原因；成功后清空。已有旧数据时界面应当继续显示旧数据 */
  error: string;
}

let snapshot: Snapshot = { report: null, updatedAt: null, loading: false, error: "" };
const listeners = new Set<() => void>();
let inflight: Promise<void> | null = null;

const AUTO_REFRESH_MS = 10 * 60 * 1000;

function update(patch: Partial<Snapshot>) {
  snapshot = { ...snapshot, ...patch };
  listeners.forEach((listener) => listener());
}

/**
 * 取一次金价。`maxAgeMs` 内已有数据就直接返回；`force` 无视新旧。
 * 失败不抛：原因写进仓库的 `error`。
 */
export function refreshMeso({
  force = false,
  maxAgeMs = 60_000,
}: { force?: boolean; maxAgeMs?: number } = {}): Promise<void> {
  if (!force && snapshot.report && snapshot.updatedAt && Date.now() - snapshot.updatedAt < maxAgeMs) {
    return Promise.resolve();
  }
  if (inflight) return inflight;

  update({ loading: true });
  inflight = invoke<MesoReport>("get_meso_report")
    .then((report) => update({ report, updatedAt: Date.now(), error: "", loading: false }))
    .catch((err: unknown) =>
      update({ error: typeof err === "string" ? err : "获取金价失败", loading: false }),
    )
    .finally(() => {
      inflight = null;
    });
  return inflight;
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/**
 * 用金价：挂载时取一次（够新就不取），之后每 10 分钟补一次，
 * 窗口重新可见 / 获得焦点时如果数据已经过期也补一次（桌面应用常常一直开着）。
 */
export function useMesoReport() {
  const state = useSyncExternalStore(subscribe, () => snapshot);

  useEffect(() => {
    void refreshMeso();
    const timer = window.setInterval(
      () => void refreshMeso({ maxAgeMs: AUTO_REFRESH_MS - 30_000 }),
      AUTO_REFRESH_MS,
    );
    const catchUp = () => {
      if (document.visibilityState !== "visible") return;
      void refreshMeso({ maxAgeMs: AUTO_REFRESH_MS });
    };
    document.addEventListener("visibilitychange", catchUp);
    window.addEventListener("focus", catchUp);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", catchUp);
      window.removeEventListener("focus", catchUp);
    };
  }, []);

  /** 用户手动点刷新：过冷却闸（防连点），然后无视新旧强制取。 */
  const refresh = () => {
    if (!guard("get_meso_report", "金价刷新", COOLDOWN.query)) return;
    void refreshMeso({ force: true });
  };

  return { ...state, refresh };
}
