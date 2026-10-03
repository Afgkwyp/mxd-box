/**
 * 查价关键词历史：两个查价入口（主窗口的拍卖查询、悬浮窗的查价模式）
 * 共用的「最近搜过什么」。
 *
 * ## 为什么只记**成功**的查询
 *
 * 输入框是打字的地方，打错、打半截是常态 —— 失败的查询也记的话，
 * 下拉里会堆满「锅盖」旁边躺着的「锅高」「hg」。所以记账的时机放在
 * 响应成功落界之后（调用方自己把握），而不是回车那一刻。
 *
 * ## 为什么存 localStorage 而不是后端
 *
 * 这是「我最近搜过什么」，纯界面便利，和设置、黑名单那种要跟着
 * 数据走的数据不同；窗口重载也不该丢，localStorage 正好。
 */
import { useState } from "react";

const STORAGE_KEY = "mxdbox.query-history";
const MAX_ITEMS = 10;

/** 读历史：最近的在前。存储坏了 / 是旧格式时安静地当没有。 */
export function loadQueryHistory(): string[] {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    const parsed: unknown = raw ? JSON.parse(raw) : null;
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((item): item is string => typeof item === "string" && item.trim() !== "");
  } catch {
    return [];
  }
}

/**
 * 记一条成功的查询：去重、最近的在前、最多留 `MAX_ITEMS` 条。
 * 关键词要先 trim 好 —— 带着空格存进去，下次点它就查不出同一批结果。
 */
export function rememberQuery(keyword: string): void {
  const term = keyword.trim();
  if (!term) return;
  try {
    const next = [term, ...loadQueryHistory().filter((item) => item !== term)].slice(0, MAX_ITEMS);
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
  } catch {
    /* 存储不可用（隐私模式等）时历史就没了，不影响查询本身 */
  }
}

/**
 * 给一个查价输入框装上「最近使用」下拉的状态与键盘行为。
 *
 * 交互约定（两个入口保持一致）：
 * * focus / 输入时弹出，按当前输入**前缀过滤**（打「锅」只留锅盖那几条）；
 * * ↑/↓ 在列表里移动高亮（不选也行，直接回车就是普通查询）；
 * * 回车在高亮项上 = 回填并立刻查询；没高亮 = 正常提交；
 * * Esc / 点到别处（blur）关闭 —— 条目上用 `onMouseDown preventDefault`
 *   挡住失焦，点击才能完整走到 onClick。
 *
 * hook 只管状态，下拉长什么样、选中之后怎么查是调用方的事 ——
 * 两个入口的提交函数不一样（主窗是 handleSearch，悬浮窗还要分模式）。
 */
export function useQueryHistory() {
  const [open, setOpen] = useState(false);
  const [items, setItems] = useState<string[]>([]);
  const [highlight, setHighlight] = useState(-1);

  /**
   * focus / 继续输入时弹出：没得可显示就不占地方。
   *
   * 匹配用**包含**而不是前缀：用户记在脑子里的是核心词（「桑拿服」），
   * 历史里存的却是当时的完整输入（「桑拿服 卷轴」）—— 只比前缀的话
   * 一条都匹配不上，下拉形同虚设。前缀命中的仍排在前面（越具体越靠前）。
   */
  const show = (typed: string) => {
    const term = typed.trim().toLowerCase();
    const all = loadQueryHistory();
    const list = term
      ? [
          ...all.filter((item) => item.toLowerCase().startsWith(term)),
          ...all.filter(
            (item) =>
              !item.toLowerCase().startsWith(term) && item.toLowerCase().includes(term),
          ),
        ]
      : all;
    setItems(list);
    setOpen(list.length > 0);
    setHighlight(-1);
  };

  const close = () => {
    setOpen(false);
    setHighlight(-1);
  };

  /** ↑/↓ 移动高亮；从「没选」往两头走都能进列表（取模循环）。 */
  const move = (delta: 1 | -1) => {
    if (items.length === 0) return;
    setHighlight((current) => {
      const next = current + delta;
      if (next < 0) return items.length - 1;
      if (next >= items.length) return 0;
      return next;
    });
  };

  /**
   * 回车时问一句「该不该走下拉」：有高亮就返回那一项并收起，
   * 没有就返回 null，让调用方走自己的正常查询路径。
   */
  const takeHighlighted = (): string | null => {
    if (!open || highlight < 0 || highlight >= items.length) return null;
    close();
    return items[highlight];
  };

  return { open, items, highlight, show, close, move, takeHighlighted };
}
