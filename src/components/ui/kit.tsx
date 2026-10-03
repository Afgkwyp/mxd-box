import React, { useLayoutEffect, useRef, useState } from "react";
import { cn } from "../../lib/utils";

/**
 * 主窗口用的几个基础件（重设计后统一用它们，不再各页各写各的）。
 *
 * 颜色全走项目自己的调色板 / `--ui-*` 令牌，深浅主题自动跟随。
 * 只做到「够用」：没有引入 Radix 之类的库 —— 这里要的就是开关、分段页签、面板，
 * 键盘与读屏所需的 role / aria 手写即可。
 */

/**
 * 滑动指示块：容器里带 `data-active="true"` 的那个子元素，由一块绝对定位的底板「滑」过去。
 *
 * 导航、页签切换时底板平滑移动，比「旧的灭、新的亮」更有连贯感。
 * 首次测量时不加过渡（否则页面一打开底板会从左上角飞过来），下一帧才打开动画。
 * 容器或子元素尺寸变化（窗口缩放、侧栏收窄）时用 ResizeObserver 重新对位。
 */
export function useSlider<T extends HTMLElement>(activeKey: string) {
  const ref = useRef<T>(null);
  const [box, setBox] = useState<{ top: number; left: number; width: number; height: number } | null>(null);
  const [animate, setAnimate] = useState(false);

  useLayoutEffect(() => {
    const root = ref.current;
    if (!root) return;
    const measure = () => {
      const el = root.querySelector<HTMLElement>('[data-active="true"]');
      if (!el) return setBox(null);
      setBox({ top: el.offsetTop, left: el.offsetLeft, width: el.offsetWidth, height: el.offsetHeight });
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(root);
    root.querySelectorAll("button").forEach((b) => observer.observe(b));
    const frame = window.requestAnimationFrame(() => setAnimate(true));
    return () => {
      observer.disconnect();
      window.cancelAnimationFrame(frame);
    };
  }, [activeKey]);

  const ease = "cubic-bezier(.2,.8,.2,1)";
  const style: React.CSSProperties = box
    ? {
        position: "absolute",
        top: box.top,
        left: box.left,
        width: box.width,
        height: box.height,
        transition: animate
          ? `top .3s ${ease}, left .3s ${ease}, width .3s ${ease}, height .3s ${ease}`
          : "none",
        pointerEvents: "none",
      }
    : { display: "none" };
  return { ref, style };
}

/** 面板：总览与各页里的内容块。 */
export const Panel: React.FC<
  Omit<React.HTMLAttributes<HTMLElement>, "title"> & { title?: React.ReactNode; meta?: React.ReactNode }
> = ({ title, meta, className, children, ...rest }) => (
  <section
    className={cn(
      "rounded-2xl border border-white/10 bg-slate-900/70 p-4 min-w-0 transition-colors duration-200 hover:border-amber-500/25",
      className,
    )}
    {...rest}
  >
    {(title || meta) && (
      <div className="flex items-center justify-between gap-2 mb-3 text-xs text-slate-400">
        <span className="font-bold text-sm text-slate-100 flex items-center gap-2 min-w-0">{title}</span>
        {meta && <span className="flex items-center gap-2 min-w-0">{meta}</span>}
      </div>
    )}
    {children}
  </section>
);

/** 开关。`label` 给读屏用（界面上另有文字说明它是什么）。 */
export const Switch: React.FC<{
  checked: boolean;
  onChange: (value: boolean) => void;
  label: string;
  disabled?: boolean;
}> = ({ checked, onChange, label, disabled }) => (
  <button
    type="button"
    role="switch"
    aria-checked={checked}
    aria-label={label}
    disabled={disabled}
    onClick={() => onChange(!checked)}
    className={cn(
      "relative w-9 h-5 rounded-full shrink-0 transition-colors duration-200 cursor-pointer disabled:opacity-40 disabled:cursor-default",
      checked ? "bg-amber-500" : "bg-slate-700",
    )}
  >
    <span
      className={cn(
        "absolute top-0.5 left-0.5 w-4 h-4 rounded-full bg-slate-950 transition-transform duration-200 ease-[cubic-bezier(.3,1.4,.5,1)]",
        checked && "translate-x-4",
      )}
    />
  </button>
);

export interface TabDef<T extends string> {
  key: T;
  label: string;
  icon?: React.ReactNode;
}

/** 页内分段页签（查询页的 物价 / 掉落 / 查人，设置页的 通用 / 悬浮窗）。切换时琥珀色底板滑过去。 */
export function SubTabs<T extends string>({
  tabs,
  value,
  onChange,
}: {
  tabs: TabDef<T>[];
  value: T;
  onChange: (key: T) => void;
}) {
  const { ref, style } = useSlider<HTMLDivElement>(value);
  return (
    <div
      ref={ref}
      role="tablist"
      className="relative inline-flex gap-1 p-1 rounded-xl bg-slate-900/70 border border-white/10"
    >
      <span aria-hidden className="rounded-lg bg-amber-500" style={style} />
      {tabs.map((tab) => {
        const on = tab.key === value;
        return (
          <button
            key={tab.key}
            type="button"
            role="tab"
            aria-selected={on}
            data-active={on}
            onClick={() => onChange(tab.key)}
            className={cn(
              "relative z-10 inline-flex items-center gap-1.5 px-3.5 py-1.5 rounded-lg text-xs font-semibold cursor-pointer",
              on ? "text-slate-950" : "text-slate-400 hover:text-slate-100",
            )}
          >
            {tab.icon}
            {tab.label}
          </button>
        );
      })}
    </div>
  );
}

/** 快捷键小标。 */
export const Kbd: React.FC<{ children: React.ReactNode; className?: string }> = ({
  children,
  className,
}) => (
  <kbd
    className={cn(
      "px-1.5 py-px rounded-md text-[10px] font-mono bg-slate-800 border border-white/10 text-slate-300",
      className,
    )}
  >
    {children}
  </kbd>
);
