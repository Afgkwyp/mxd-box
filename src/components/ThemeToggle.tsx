import React from "react";
import { Moon, Sun } from "lucide-react";
import { useTheme } from "../lib/theme";

interface ThemeToggleProps {
  /** 图标尺寸档位（紧凑档给 HUD 标题栏用） */
  compact?: boolean;
  className?: string;
}

/**
 * 日间 / 夜间主题切换。
 *
 * 按钮上只放图标，不放文字 —— 太阳 = 当前是日间（点击切到夜间），
 * 月亮 = 当前是夜间。含义写在 title / aria-label 里，鼠标悬停就能看到。
 *
 * 换肤的实际工作在 src/index.css 的 html.theme-light 段完成，
 * 这里只负责把主题写到 <html> 上并跨窗口同步。
 */
export const ThemeToggle: React.FC<ThemeToggleProps> = ({ compact = false, className = "" }) => {
  const [theme, toggleTheme] = useTheme();
  const isLight = theme === "light";

  return (
    <button
      type="button"
      onClick={toggleTheme}
      title={isLight ? "当前：日间主题（点击切到夜间）" : "当前：夜间主题（点击切到日间）"}
      aria-label={isLight ? "切换到夜间主题" : "切换到日间主题"}
      className={`shrink-0 inline-flex items-center justify-center rounded-lg border border-white/10 bg-slate-800/80 text-slate-300 hover:text-amber-300 hover:border-amber-500/40 transition-colors cursor-pointer ${
        compact ? "p-1" : "p-1.5"
      } ${className}`}
    >
      {isLight ? (
        <Sun className={compact ? "w-4 h-4" : "w-3.5 h-3.5"} />
      ) : (
        <Moon className={compact ? "w-4 h-4" : "w-3.5 h-3.5"} />
      )}
    </button>
  );
};
