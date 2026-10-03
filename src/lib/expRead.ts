/**
 * 读不到经验时界面上的**一句话**（不搬后端那三四行诊断文案）：
 * 用户只需要知道「没读到 + 该怎么办」，细节留给日志。
 */
export const READ_SHORT: Record<string, string> = {
  no_window: "没找到游戏窗口（游戏没开？）",
  minimized: "游戏窗口最小化了",
  capture_failed: "抓不到游戏画面",
  blank_frame: "游戏画面还是空白的（还没画出来？）",
  no_field: "没识别到经验条",
  contradiction: "读数对不上，正在重试",
};

/**
 * 读不到是因为**游戏不在**（没开 / 最小化了）。
 *
 * 这时候校准位置没有任何意义 —— 画面都没有，框什么都是白框。界面只说一句
 * 「游戏没开」，不摆「校准位置」按钮，免得让人以为是自己哪里没设好。
 */
export function gameAway(readState: string | null | undefined): boolean {
  return readState === "no_window" || readState === "minimized";
}

/** 游戏不在时的短句（悬浮窗、总览这种窄地方用）。 */
export function gameAwayText(readState: string | null | undefined): string {
  return readState === "minimized" ? "游戏窗口最小化了" : "游戏没开";
}
