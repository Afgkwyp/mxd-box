import { useSyncExternalStore } from "react";
import { emit, listen } from "@tauri-apps/api/event";

/**
 * 界面动效的总开关（设置 → 通用）。
 *
 * 为什么不跟系统的「动画效果」走（`prefers-reduced-motion`）：很多玩家为了省性能把
 * Windows 的动画效果关了，于是这个软件里的过渡、数字滚动、小结卡片的翻面和镭射，
 * 在他们机器上从来没播过 —— 连作者自己都没见过。系统那个开关管的是整个 Windows，
 * 用户关它多半不是冲着这个小工具来的。所以动效由软件自己的开关决定，默认开；
 * 真不想看动效的人，在设置里关掉就全停。
 *
 * 做法和主题一样（见 theme.ts）：写到 `<html data-motion>` 上、记进 localStorage、
 * 跨窗口广播。CSS 那边 `html[data-motion="off"]` 把所有动画和过渡压到 0
 * （index.css 末尾）；JS 驱动的动效（数字滚动、卡片自己晃）读 `motionEnabled()`。
 */

const STORAGE_KEY = "mxdbox.motion";
const EVENT_NAME = "motion-changed";

const subscribers = new Set<() => void>();
let current = readStored();

function readStored(): boolean {
  try {
    return localStorage.getItem(STORAGE_KEY) !== "off";
  } catch {
    return true;
  }
}

function applyToDom(enabled: boolean) {
  document.documentElement.dataset.motion = enabled ? "on" : "off";
}

function notify(enabled: boolean) {
  current = enabled;
  subscribers.forEach((fn) => fn());
}

/** 现在放不放动效（给不在 React 里的代码、或者只在动效开始那一刻读一次的地方用）。 */
export function motionEnabled(): boolean {
  return current;
}

/** 开关动效：即时生效 + 持久化 + 广播给其它窗口。 */
export function setMotion(enabled: boolean) {
  try {
    localStorage.setItem(STORAGE_KEY, enabled ? "on" : "off");
  } catch {
    /* 存储不可用时也不该影响开关本身 */
  }
  applyToDom(enabled);
  notify(enabled);
  emit(EVENT_NAME, enabled).catch(() => {});
}

/** 应用启动时调用一次（在 React 渲染之前，免得第一屏的入场动画按错的设置播）。 */
export function initMotion() {
  current = readStored();
  applyToDom(current);

  listen<boolean>(EVENT_NAME, (event) => {
    const enabled = event.payload !== false;
    if (enabled === current) return;
    applyToDom(enabled);
    notify(enabled);
  }).catch(() => {});
}

function subscribe(onChange: () => void) {
  subscribers.add(onChange);
  return () => {
    subscribers.delete(onChange);
  };
}

/** const [motion, setMotion] = useMotion(); */
export function useMotion(): [boolean, (enabled: boolean) => void] {
  return [useSyncExternalStore(subscribe, motionEnabled), setMotion];
}
