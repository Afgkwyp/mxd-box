import React, { useEffect } from "react";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";

/**
 * 无边框透明悬浮窗的「缩放热区」+「尺寸记忆」。
 *
 * ## 缩放怎么做
 *
 * 窗口是 `decorations: false` + `transparent: true`，系统不给缩放边框，所以在面板外圈
 * 那一圈透明留白（`WINDOW_GUTTER`）里放 8 个热区，按下时交给系统：
 * `startResizeDragging(direction)`。
 *
 * 以前的精简窗用的是「自己算指针 + setSize + CSS zoom 整体等比放大」，那套是为了
 * 「内容不被裁掉」；现在内容是**自适应**的（容器查询，见 styles/overlay.css）——
 * 窗口多大内容就排多大、放不下的行自己收起来，不再需要把整个界面当图片缩放，
 * 所以可以把缩放完全交给系统，也就不存在「系统和 setSize 互抢窗口」的抖动了。
 *
 * ## 尺寸记忆
 *
 * 用户拖出来的大小写进 localStorage，下次启动时 `setSize` 回去。用 DOM 的 `resize`
 * 事件而不是 Tauri 的 `onResized`：`innerWidth/innerHeight` 本身就是逻辑像素，
 * 和 `LogicalSize` 同一套，不必再乘缩放因子。
 */
export const WINDOW_GUTTER = 6;

type Direction =
  | "North"
  | "South"
  | "East"
  | "West"
  | "NorthEast"
  | "NorthWest"
  | "SouthEast"
  | "SouthWest";

const EDGE = 6;
const CORNER = 14;

const HANDLES: Array<{ dir: Direction; style: React.CSSProperties; cursor: string }> = [
  { dir: "North", style: { top: 0, left: CORNER, right: CORNER, height: EDGE }, cursor: "ns-resize" },
  { dir: "South", style: { bottom: 0, left: CORNER, right: CORNER, height: EDGE }, cursor: "ns-resize" },
  { dir: "West", style: { left: 0, top: CORNER, bottom: CORNER, width: EDGE }, cursor: "ew-resize" },
  { dir: "East", style: { right: 0, top: CORNER, bottom: CORNER, width: EDGE }, cursor: "ew-resize" },
  { dir: "NorthWest", style: { top: 0, left: 0, width: CORNER, height: CORNER }, cursor: "nwse-resize" },
  { dir: "NorthEast", style: { top: 0, right: 0, width: CORNER, height: CORNER }, cursor: "nesw-resize" },
  { dir: "SouthWest", style: { bottom: 0, left: 0, width: CORNER, height: CORNER }, cursor: "nesw-resize" },
  { dir: "SouthEast", style: { bottom: 0, right: 0, width: CORNER, height: CORNER }, cursor: "nwse-resize" },
];

export const ResizeHandles: React.FC = () => (
  <>
    {HANDLES.map(({ dir, style, cursor }) => (
      <div
        key={dir}
        aria-hidden="true"
        onPointerDown={(event) => {
          if (event.button !== 0) return;
          event.preventDefault();
          getCurrentWindow()
            .startResizeDragging(dir)
            .catch(() => {});
        }}
        style={{ position: "absolute", zIndex: 50, cursor, touchAction: "none", ...style }}
      />
    ))}
  </>
);

/**
 * 悬浮窗每次被快捷键呼出时播一下「浮上来」的入场动画。
 *
 * 窗口是 hide / show 出来的，页面一直活着，挂载动画只会在启动时播一次；
 * 所以要自己知道「被呼出了」，给 <html> 加一下 `ov-enter`（样式见 styles/overlay.css）。
 *
 * ## 为什么收起前要先把自己清空（`ov-hidden`）
 *
 * 窗口隐藏之后，系统那边留着的是**最后画出来的那一帧**。直接隐藏的话那一帧是完整的面板，
 * 下次呼出的一瞬间先贴出来的就是它，接着页面才开始从透明淡入 —— 看起来是
 * 「闪一下 → 消失 → 再淡入」。所以 Rust 那边收起前会先发 `overlay-hiding`（见 lib.rs 的
 * `hide_overlay`），这里把整个界面清成透明，它等这一帧画出来再真的隐藏。
 *
 * ## 为什么「被呼出了」要听三个信号
 *
 * 可见性变化、获得焦点、Rust 发的 `overlay-shown`：前两个各有不来的时候，而界面收起前
 * 已经是透明的，漏掉一次就是呼出来一个空窗口。三个里谁先到谁算，后到的不重播动画
 * （窗口一直开着、只是从游戏点回来拿到焦点，也不重播）。
 */
export function useEnterAnimation() {
  useEffect(() => {
    const root = document.documentElement;
    let timer: number | undefined;
    // 只在「从清空的状态被呼出」时播：窗口一直开着、只是又拿到了焦点（从游戏点回来）不该重播
    const play = () => {
      if (!root.classList.contains("ov-hidden")) return;
      root.classList.remove("ov-hidden");
      root.classList.remove("ov-enter");
      // 强制回流，让同一个 class 能重复触发动画
      void root.offsetWidth;
      root.classList.add("ov-enter");
      window.clearTimeout(timer);
      timer = window.setTimeout(() => root.classList.remove("ov-enter"), 400);
    };
    const blank = () => {
      window.clearTimeout(timer);
      root.classList.remove("ov-enter");
      root.classList.add("ov-hidden");
    };
    const onVisible = () => {
      if (document.visibilityState === "visible") play();
    };
    document.addEventListener("visibilitychange", onVisible);
    window.addEventListener("focus", play);
    const current = getCurrentWindow();
    const unlisten = [current.listen("overlay-hiding", blank), current.listen("overlay-shown", play)];

    // 启动时窗口是藏着的：先清空，等第一次呼出再播（否则第一次呼出照样先闪一下）。
    // 问不到可见性（浏览器里调界面时）就当它开着。
    blank();
    current
      .isVisible()
      .then((visible) => {
        if (visible !== false) play();
      })
      .catch(play);

    return () => {
      window.clearTimeout(timer);
      document.removeEventListener("visibilitychange", onVisible);
      window.removeEventListener("focus", play);
      unlisten.forEach((pending) => pending.then((off) => off()).catch(() => {}));
    };
  }, []);
}

/**
 * 拖动窗口：自己处理，不用 `data-tauri-drag-region`。
 *
 * 那个属性会把**双击**当成「最大化 / 还原」，而悬浮窗一最大化就铺满整屏（用户实测报的）。
 * 这里改成：按下后鼠标真的挪动了几个像素才开始拖（`startDragging`），
 * 单纯的点击 / 双击不会进入系统的拖动循环，也就不会被吞掉 —— 标题栏的「双击收起」才能正常触发。
 * 按钮、输入框、下拉框上按下不算拖动。
 */
export function useWindowDrag() {
  return {
    onMouseDown: (event: React.MouseEvent) => {
      if (event.button !== 0) return;
      if ((event.target as HTMLElement).closest("button,input,select,textarea,a,label,[data-no-drag]")) return;
      const startX = event.screenX;
      const startY = event.screenY;
      const cleanup = () => {
        window.removeEventListener("mousemove", move);
        window.removeEventListener("mouseup", cleanup);
      };
      const move = (e: MouseEvent) => {
        if (e.buttons !== 1) return cleanup();
        if (Math.abs(e.screenX - startX) + Math.abs(e.screenY - startY) > 4) {
          cleanup();
          getCurrentWindow()
            .startDragging()
            .catch(() => {});
        }
      };
      window.addEventListener("mousemove", move);
      window.addEventListener("mouseup", cleanup);
    },
  };
}

interface StoredSize {
  w: number;
  h: number;
}

function readSize(key: string): StoredSize | null {
  try {
    const raw = window.localStorage.getItem(key);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Partial<StoredSize>;
    if (
      typeof parsed.w === "number" &&
      typeof parsed.h === "number" &&
      parsed.w >= 120 &&
      parsed.h >= 40 &&
      parsed.w <= 4000 &&
      parsed.h <= 3000
    ) {
      return { w: parsed.w, h: parsed.h };
    }
  } catch {
    /* 存储不可用 / 旧格式：当没有 */
  }
  return null;
}

/**
 * 记住并恢复窗口大小。`key` 每个窗口一份（`mxdbox.window.hud` / `.float`）。
 *
 * 返回 `resizeTo(w, h)`：程序主动改大小时用（比如精简窗的「收起成单行条」）。
 * 主动改大小同样会触发 `resize` 事件，所以会被一并记下 —— 调用方如果不想让
 * 「收起态」成为下次启动的默认大小，就在自己那边把展开态的尺寸记好再恢复。
 */
export function useWindowSize(key: string, minW: number, minH: number) {
  useEffect(() => {
    const saved = readSize(key);
    if (saved) {
      getCurrentWindow()
        .setSize(new LogicalSize(Math.max(minW, saved.w), Math.max(minH, saved.h)))
        .catch(() => {});
    }

    let timer: number | undefined;
    const onResize = () => {
      window.clearTimeout(timer);
      timer = window.setTimeout(() => {
        try {
          window.localStorage.setItem(
            key,
            JSON.stringify({ w: Math.round(window.innerWidth), h: Math.round(window.innerHeight) }),
          );
        } catch {
          /* 存不了就算了，只是下次不记得 */
        }
      }, 300);
    };
    window.addEventListener("resize", onResize);
    return () => {
      window.clearTimeout(timer);
      window.removeEventListener("resize", onResize);
    };
  }, [key, minW, minH]);

  return (w: number, h: number) =>
    getCurrentWindow()
      .setSize(new LogicalSize(Math.round(w), Math.round(h)))
      .catch(() => {});
}
