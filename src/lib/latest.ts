import { useCallback, useRef } from "react";

/**
 * 「只认最后一次请求」的序号，用来挡住竞态。
 *
 * 场景很常见：在悬浮窗里连着改两次关键词回车（或者点了「加载更多」之后又改了词），
 * 两个请求同时在飞，**先发的那个可能后回来** —— 结果界面显示的是上一次查询的内容，
 * 而输入框里写着新的关键词。用户看到的是「搜了 A 显示的却是 B」，或者「翻页之后
 * 结果变成另一批」。查询本身没有错，只是回包的先后顺序不该决定画面。
 *
 * 用法：
 * ```ts
 * const beginRequest = useLatestRequest();
 * const isCurrent = beginRequest();
 * const data = await invoke(...);
 * if (!isCurrent()) return;   // 已经有更新的请求了，这份结果丢掉
 * setItems(data);
 * ```
 */
export function useLatestRequest(): () => () => boolean {
  const seq = useRef(0);

  return useCallback(() => {
    const mine = ++seq.current;
    return () => seq.current === mine;
  }, []);
}
