import React, { useEffect, useRef, useState } from "react";
import { Keyboard, RotateCcw } from "lucide-react";

interface Props {
  /** 当前快捷键，例如 "F10" */
  value: string;
  /** 捕获到新组合时回调（已规范成 "Ctrl+Shift+K" 这种写法） */
  onChange: (hotkey: string) => void;
  /** 「恢复默认」用的默认值 */
  defaultValue: string;
  /**
   * 捕获开始 / 结束的通知。
   *
   * 父组件收到 `true` 时要**临时注销全局快捷键** —— 这是这个交互能不能用的前提：
   * 全局键是 OS 级的 `RegisterHotKey`，键被注册时系统会把按键直接投递给我们的
   * 消息队列，**webview 根本收不到 keydown**。所以要改 `F10` 时按 `F10`，
   * 前端什么也收不到，反而把悬浮窗呼了出来；`preventDefault` 对 OS 级分发无效。
   */
  onCaptureChange?: (capturing: boolean) => void;
}

/** 修饰键单独按下时不算一个组合（否则用户按 Ctrl 就得到一个 "Ctrl" 快捷键）。 */
const MODIFIER_CODES = new Set(["ControlLeft", "ControlRight", "ShiftLeft", "ShiftRight", "AltLeft", "AltRight", "MetaLeft", "MetaRight"]);

/**
 * 把 `KeyboardEvent.code` 变成本工具链认识的键名。
 *
 * 用 `code` 而不是 `key`：`key` 会被键盘布局和输入法改掉（中文输入法下按 K
 * 可能给出别的字符），而 `code` 是物理按键，和 `tauri` 的 `Shortcut` 解析
 * 用的是同一套名字（`KeyK` / `Digit1` / `F10` / `ArrowUp`…）。
 */
function codeToKeyName(code: string): string | null {
  if (/^Key[A-Z]$/.test(code)) return code.slice(3);
  if (/^Digit[0-9]$/.test(code)) return code.slice(5);
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) return code;
  if (/^Numpad[0-9]$/.test(code)) return `Numpad${code.slice(6)}`;

  const named: Record<string, string> = {
    ArrowUp: "ArrowUp",
    ArrowDown: "ArrowDown",
    ArrowLeft: "ArrowLeft",
    ArrowRight: "ArrowRight",
    Space: "Space",
    Enter: "Enter",
    Tab: "Tab",
    Backquote: "Backquote",
    Minus: "Minus",
    Equal: "Equal",
    BracketLeft: "BracketLeft",
    BracketRight: "BracketRight",
    Semicolon: "Semicolon",
    Quote: "Quote",
    Backslash: "Backslash",
    Comma: "Comma",
    Period: "Period",
    Slash: "Slash",
    Home: "Home",
    End: "End",
    PageUp: "PageUp",
    PageDown: "PageDown",
    Insert: "Insert",
    Delete: "Delete",
  };
  return named[code] ?? null;
}

/**
 * 按压式快捷键输入框。
 *
 * 以前是一个普通文本框，要用户自己**打字**输入 `Alt+F` —— 这个交互有两个问题：
 * 键盘上并没有一个叫 `+` 的主键，「`Alt` 该写 Alt 还是 AltLeft」也没有人知道，
 * 所以经常存进去一个 `tauri` 解析不了、或者根本收不到事件的串（然后看起来就像
 * 「快捷键设了但按了没反应」）。现在改成：点一下输入框、**直接按下想用的组合**。
 */
export const HotkeyInput: React.FC<Props> = ({
  value,
  onChange,
  defaultValue,
  onCaptureChange,
}) => {
  const [capturing, setCapturing] = useState(false);
  const [hint, setHint] = useState("");
  const boxRef = useRef<HTMLDivElement>(null);
  /**
   * 用 ref 存回调：父组件每次渲染都会给一个新函数，不该因此反复注销/重注册热键。
   * 必须声明在下面那个捕获 effect **之前**：同一次提交里 effect 按声明顺序执行，
   * 先同步回调、再发捕获通知。
   */
  const notify = useRef(onCaptureChange);
  useEffect(() => {
    notify.current = onCaptureChange;
  });

  /**
   * 捕获期间通知父组件注销全局快捷键；结束（捕获成功 / Esc / 点到别处 /
   * 组件卸载）时再通知挂回去。用 effect 的 cleanup 做「一定会恢复」这一步，
   * 就不会出现「改键改到一半把快捷键弄没了」。
   */
  useEffect(() => {
    if (!capturing) return;
    notify.current?.(true);
    return () => notify.current?.(false);
  }, [capturing]);

  // 捕获时监听的是 window：按键焦点可能落在任何地方（用户刚点完别的输入框）
  useEffect(() => {
    if (!capturing) return;

    const handle = (event: KeyboardEvent) => {
      // 捕获期间把所有按键都吃掉：否则按 F10 就真的把悬浮窗呼出来了，
      // 用户还在设置页里，会以为程序乱跳。
      event.preventDefault();
      event.stopPropagation();

      if (event.code === "Escape") {
        setCapturing(false);
        setHint("");
        return;
      }

      if (MODIFIER_CODES.has(event.code)) {
        setHint("继续按主键（例如按住 Alt 再按 F）…");
        return;
      }

      const parts: string[] = [];
      if (event.ctrlKey) parts.push("Ctrl");
      if (event.shiftKey) parts.push("Shift");
      if (event.altKey) parts.push("Alt");
      if (event.metaKey) parts.push("Super");
      const key = codeToKeyName(event.code);
      if (!key) {
        setHint("这个键不被支持，换一个试试（字母 / 数字 / F1-F24 / 方向键…）");
        return;
      }
      parts.push(key);

      onChange(parts.join("+"));
      setCapturing(false);
      setHint("");
    };

    // capture 阶段监听，抢在页面其它快捷键前面
    window.addEventListener("keydown", handle, true);
    return () => window.removeEventListener("keydown", handle, true);
  }, [capturing, onChange]);

  // 点别处就结束捕获（避免「输入框看起来还能打字」的错觉）
  useEffect(() => {
    if (!capturing) return;
    const onBlur = (event: MouseEvent) => {
      if (boxRef.current && !boxRef.current.contains(event.target as Node)) {
        setCapturing(false);
        setHint("");
      }
    };
    document.addEventListener("mousedown", onBlur);
    return () => document.removeEventListener("mousedown", onBlur);
  }, [capturing]);

  return (
    <div className="space-y-1.5">
      <div ref={boxRef} className="flex items-center gap-2">
        <button
          type="button"
          onClick={() => {
            setCapturing(true);
            setHint("请直接按下想用的组合键…");
          }}
          className={`flex-1 px-3 py-2 rounded-lg border tabular-nums text-sm flex items-center justify-center gap-2 transition-colors cursor-pointer ${
            capturing
              ? "bg-amber-500/15 border-amber-500/60 text-amber-200 animate-pulse"
              : "bg-slate-800 border-white/10 text-slate-100 hover:border-amber-500/40"
          }`}
        >
          <Keyboard className="w-3.5 h-3.5 opacity-70" />
          {capturing ? "按下快捷键…" : value || "（未设置）"}
        </button>

        <button
          type="button"
          onClick={() => onChange(defaultValue)}
          title={`恢复为默认 ${defaultValue}`}
          className="px-2.5 py-2 rounded-lg bg-slate-800 hover:bg-slate-700 text-slate-300 border border-white/10 text-xs flex items-center gap-1.5 cursor-pointer"
        >
          <RotateCcw className="w-3.5 h-3.5" />
          默认
        </button>
      </div>

      <p className="text-[11px] text-slate-500 leading-relaxed">
        {hint || ""}
      </p>
    </div>
  );
};
