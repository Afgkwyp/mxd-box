import { useEffect, useState } from "react";
import { emit, listen } from "@tauri-apps/api/event";

/**
 * 深色 / 浅色主题。
 *
 * 换肤本身完全由 CSS 变量完成（见 src/index.css 的 html.theme-light 段），
 * 这里只负责三件事：
 *   1. 把主题写到 <html> 上；
 *   2. 记住用户的选择（localStorage）；
 *   3. 在多个窗口之间同步 —— 主窗口切成日间，HUD 也要立刻跟着变。
 *
 * 为什么需要模块级的订阅表：同一窗口里可能有好几个组件要用主题（切换按钮、
 * 图表取色），而 Tauri 的 emit 是「跨窗口广播」，不保证回过来源窗口，
 * 所以本窗口内部同步走订阅表，跨窗口同步走 emit/listen。
 */

export type Theme = "dark" | "light";

const STORAGE_KEY = "mxdbox.theme";
const EVENT_NAME = "theme-changed";

const subscribers = new Set<(theme: Theme) => void>();
let current: Theme = readStored();

function readStored(): Theme {
  try {
    return localStorage.getItem(STORAGE_KEY) === "light" ? "light" : "dark";
  } catch {
    return "dark";
  }
}

function applyToDom(theme: Theme) {
  const root = document.documentElement;
  root.classList.toggle("theme-light", theme === "light");
  root.dataset.theme = theme;
}

function notify(theme: Theme) {
  current = theme;
  subscribers.forEach((fn) => fn(theme));
}

/** 切换主题：即时生效 + 持久化 + 广播给其它窗口。 */
export function setTheme(theme: Theme) {
  try {
    localStorage.setItem(STORAGE_KEY, theme);
  } catch {
    /* 存储不可用时也不该影响换肤 */
  }
  applyToDom(theme);
  notify(theme);
  emit(EVENT_NAME, theme).catch(() => {});
}

/**
 * 应用启动时调用一次（在 React 渲染之前，避免先闪一下深色）。
 * 返回当前主题，方便日志或调试。
 */
export function initTheme(): Theme {
  current = readStored();
  applyToDom(current);

  listen<Theme>(EVENT_NAME, (event) => {
    const theme: Theme = event.payload === "light" ? "light" : "dark";
    if (theme === current) return;
    applyToDom(theme);
    notify(theme);
  }).catch(() => {});

  return current;
}

/** 读取并切换主题：const [theme, toggleTheme] = useTheme(); */
export function useTheme(): [Theme, () => void] {
  const [theme, setLocal] = useState<Theme>(current);

  useEffect(() => {
    subscribers.add(setLocal);
    setLocal(current); // 挂载前主题可能已被别的组件改过
    return () => {
      subscribers.delete(setLocal);
    };
  }, []);

  return [theme, () => setTheme(current === "light" ? "dark" : "light")];
}
