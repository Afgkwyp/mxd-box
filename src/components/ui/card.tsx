import React from "react";
import { cn } from "../../lib/utils";

/**
 * 卡片（shadcn/ui 的 `Card` 那套 API 与视觉语言）。
 *
 * 项目里**并没有装 shadcn/ui** —— 没有 `components.json`、没有 radix、没有
 * `class-variance-authority`，界面全是 Tailwind v4 手写的。所以「卡片」以前不存在，
 * 需要什么都是各写各的 `div className="p-5 rounded-xl bg-slate-900/60 border ..."`，
 * 于是同一页里几块内容的圆角、内边距、边框深浅都不一样。
 *
 * 这里按 shadcn 的骨架补一套：`Card` / `CardHeader` / `CardTitle` / `CardContent` /
 * `CardFooter`。颜色继续用项目自己的 Tailwind 调色板类（深/浅主题是靠替换
 * `--color-slate-*` 这些变量实现的，见 index.css），所以卡片自动跟随主题。
 *
 * 尺寸比 shadcn 默认更紧（`p-3` 而不是 `p-6`）：这是给 340px 宽的悬浮窗用的，
 * 默认那套内边距在这么窄的地方会把内容挤没。
 */
export const Card: React.FC<React.HTMLAttributes<HTMLDivElement>> = ({
  className,
  ...props
}) => (
  <div
    className={cn(
      "rounded-xl border border-white/10 bg-slate-900/60 text-slate-100",
      "shadow-sm overflow-hidden",
      className,
    )}
    {...props}
  />
);

export const CardHeader: React.FC<React.HTMLAttributes<HTMLDivElement>> = ({
  className,
  ...props
}) => (
  <div
    className={cn("flex items-center justify-between gap-2 px-3 pt-2 pb-1", className)}
    {...props}
  />
);

export const CardTitle: React.FC<React.HTMLAttributes<HTMLDivElement>> = ({
  className,
  ...props
}) => (
  <div
    className={cn("flex items-center gap-1.5 text-[11px] font-medium text-slate-300", className)}
    {...props}
  />
);

export const CardContent: React.FC<React.HTMLAttributes<HTMLDivElement>> = ({
  className,
  ...props
}) => <div className={cn("px-3 pb-2.5", className)} {...props} />;

export const CardFooter: React.FC<React.HTMLAttributes<HTMLDivElement>> = ({
  className,
  ...props
}) => (
  <div
    className={cn("flex items-center justify-between gap-2 px-3 py-1.5 border-t border-white/5", className)}
    {...props}
  />
);
