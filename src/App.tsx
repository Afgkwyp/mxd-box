import { lazy, Suspense } from "react";
import { ThrottleToast } from "./components/ThrottleToast";

/**
 * 每个窗口只加载自己那一份代码。
 *
 * 六个窗口共用同一个 index.html，以前所有视图都打在一个包里：经验窗（F10）一行图表都不画，
 * 却要先把主窗口的全部页面和图表库解析一遍才能显示。现在按 `?window=` 懒加载，
 * 悬浮窗只拿自己用得到的那几十 KB。
 */
const view = <T extends Record<string, React.ComponentType>>(
  load: () => Promise<T>,
  name: keyof T,
) => lazy(() => load().then((module) => ({ default: module[name] })));

const AlertToastView = view(() => import("./views/AlertToastView"), "AlertToastView");
const CalibrationOverlayView = view(
  () => import("./views/CalibrationOverlayView"),
  "CalibrationOverlayView",
);
const FloatView = view(() => import("./views/FloatView"), "FloatView");
const HudView = view(() => import("./views/HudView"), "HudView");
const MainView = view(() => import("./views/MainView"), "MainView");
const LoginModalView = view(() => import("./views/LoginModalView"), "LoginModalView");

/**
 * 无边框悬浮窗：窗口是透明的（tauri.conf 里 transparent=true），
 * 而全站 body 有底色 —— 必须把底色让开，否则窗口会显示成一个方块，
 * 面板的圆角也就白做了。
 *
 * 放在模块加载时做（早于首次渲染），而不是各视图在 useEffect 里加：
 * 少一次「底色先闪一下」的机会。样式规则见 index.css 的 html.overlay-window。
 */
const OVERLAY_WINDOWS = ["hud", "float", "alert", "calibration-overlay"];

const win = new URLSearchParams(window.location.search).get("window");

if (win && OVERLAY_WINDOWS.includes(win)) {
  document.documentElement.classList.add("overlay-window");
}

/** 懒加载的那一小会儿什么都不画：窗口本身要么透明、要么是底色，不需要占位。 */
export function App() {
  return (
    <Suspense fallback={null}>
      <WindowView />
    </Suspense>
  );
}

function WindowView() {
  // 校准蒙版窗：透明置顶，正好盖在游戏客户区上（见 CalibrationOverlayView）
  if (win === "calibration-overlay") {
    return <CalibrationOverlayView />;
  }

  // 查价悬浮窗（Alt+F）与精简悬浮窗（F10）是两个不同的窗口，别搞混
  if (win === "hud") {
    return (
      <>
        <HudView />
        <ThrottleToast />
      </>
    );
  }

  if (win === "float") {
    return (
      <>
        <FloatView />
        <ThrottleToast />
      </>
    );
  }

  if (win === "login") {
    return (
      <>
        <LoginModalView />
        <ThrottleToast />
      </>
    );
  }

  // 开服提醒窗 / 黑名单命中提醒窗：一个独立的、无边框置顶的小窗
  if (win === "alert") {
    return <AlertToastView />;
  }

  return (
    <>
      <MainView />
      <ThrottleToast />
    </>
  );
}

export default App;
