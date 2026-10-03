import React, { useEffect, useState } from "react";
import { subscribeThrottle } from "../lib/throttle";

/**
 * 「操作太频繁」的提示条。
 *
 * 挂在 App 上，所以主窗口、查价悬浮窗、精简悬浮窗里被拦下的操作都看得到 ——
 * 每个窗口各写一份的话，迟早有窗口漏掉，而「点了没反应」正是我们想避免的那种体验。
 *
 * 只显示一句话，不解释为什么。几秒后自己消失。
 */
export const ThrottleToast: React.FC = () => {
  const [message, setMessage] = useState("");
  const [token, setToken] = useState(0);

  useEffect(() => {
    let timer = 0;
    const unsubscribe = subscribeThrottle(({ label, waitMs }) => {
      setMessage(`${label}：请 ${Math.ceil(waitMs / 1000)} 秒后再试`);
      setToken((value) => value + 1);
    });
    return () => {
      unsubscribe();
      window.clearTimeout(timer);
    };
  }, []);

  useEffect(() => {
    if (token === 0) return;
    const timer = window.setTimeout(() => setMessage(""), 2600);
    return () => window.clearTimeout(timer);
  }, [token]);

  if (!message) return null;
  return (
    <div className="pointer-events-none fixed left-1/2 top-3 z-[999] -translate-x-1/2">
      <div className="anim-toast rounded-lg border border-amber-500/40 bg-slate-950 px-3 py-1.5 text-xs text-amber-200">
        {message}
      </div>
    </div>
  );
};
