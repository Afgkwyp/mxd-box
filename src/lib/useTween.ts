import { useEffect, useRef, useState } from "react";
import { motionEnabled } from "./motion";

/**
 * 数字缓动：目标值变了，显示值在 `duration` 毫秒内滚过去（easeOutCubic）。
 *
 * 用在「一眼要看的大数字」上（金价、经验 / 小时）：直接跳变看不出变了多少，
 * 滚一下能感觉到「涨了 / 跌了」。
 *
 * * `fromZero`：首次出现时从 0 滚到目标值（总览页的入场效果）；默认首次直接显示目标值，
 *   悬浮窗每次呼出不该再滚一遍；
 * * 目标为 `null`（还没数据 / 读不到）时原样返回 `null`，不拿 0 去顶替 —— 「没有」和「是 0」不是一回事；
 * * 设置里关了「界面动效」时不动画，直接给目标值。
 */
export function useTween(
  target: number | null,
  { duration = 600, fromZero = false }: { duration?: number; fromZero?: boolean } = {},
): number | null {
  const [shown, setShown] = useState<number | null>(fromZero && target != null ? 0 : target);
  const shownRef = useRef(shown);
  useEffect(() => {
    shownRef.current = shown;
  }, [shown]);

  useEffect(() => {
    if (target == null) {
      setShown(null);
      return;
    }
    // 首次拿到数据且要求「从 0 滚起」：把 null 当 0
    const from = shownRef.current ?? (fromZero ? 0 : null);
    if (from == null || from === target || !motionEnabled()) {
      setShown(target);
      return;
    }
    let frame = 0;
    const start = performance.now();
    const tick = (now: number) => {
      const t = Math.min(1, (now - start) / duration);
      const eased = 1 - Math.pow(1 - t, 3);
      setShown(from + (target - from) * eased);
      if (t < 1) frame = window.requestAnimationFrame(tick);
    };
    frame = window.requestAnimationFrame(tick);
    return () => window.cancelAnimationFrame(frame);
  }, [target, duration, fromZero]);

  return shown;
}
