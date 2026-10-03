/**
 * 防连点：同一件事在 N 秒内只放行一次。
 *
 * ## 为什么要有它
 *
 * 这个软件的按钮背后大多是**真的去打小册子那个网站**（拍卖查询、金价、掉落速查、
 * 版本自检…）。这些请求本身不快，用户点一下没立刻看到东西就会再点一下 ——
 * 于是同一秒里发出去五六个请求。对用户是白等，对那个站点是不必要的压力
 * （而且它并不是我们自己的服务）。
 *
 * 所以凡是「按一下就会出去发请求」的地方，都先过一道 [`guard`]：
 *
 * * 第一次点 → 放行；
 * * 冷却期里再点 → **拦下来**，并且广播一条提示，告诉他还要等几秒。
 *
 * 只提示不静默拦截是刻意的：静默拦下来，用户只会觉得「点了没反应、这软件坏了」。
 *
 * ## 为什么不用防抖（debounce）
 *
 * 防抖是「等用户停下来再发」，适合输入框；按钮要的是「立刻发一次，然后短期别再发」，
 * 那是节流（throttle）+ 冷却提示。这两件事不一样，别弄混。
 */

/** 一件「事」的冷却记录。key 用命令名或者「命令名 + 参数摘要」。 */
const lastFiredAt = new Map<string, number>();

/** 被拦下时通知界面（`ThrottleToast` 订阅它）。 */
type Listener = (info: { label: string; waitMs: number }) => void;
const listeners = new Set<Listener>();

export function subscribeThrottle(listener: Listener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** 各类操作的默认冷却（秒）。查询类短一点，重活长一点。 */
export const COOLDOWN = {
  /** 打站点查数据：拍卖、掉落、金价、物品详情 */
  query: 5,
  /** 版本自检、登录凭据校验这类「重活」，而且结果本来就不会秒变 */
  heavy: 10,
  /** 会改本地状态的操作（写库 / 结束会话 / 生成图片） */
  mutate: 3,
} as const;

/**
 * 过一道闸：能发就返回 `true`，冷却期里返回 `false` 并广播提示。
 *
 * ```ts
 * if (!guard("query_market", "拍卖查询", COOLDOWN.query)) return;
 * ```
 */
export function guard(
  key: string,
  label: string,
  seconds: number = COOLDOWN.query,
): boolean {
  const now = Date.now();
  const last = lastFiredAt.get(key);
  if (last != null) {
    const elapsed = now - last;
    const window = seconds * 1000;
    if (elapsed < window) {
      const waitMs = window - elapsed;
      for (const listener of listeners) listener({ label, waitMs });
      return false;
    }
  }
  lastFiredAt.set(key, now);
  return true;
}

/**
 * 「跑之前先过闸」的包装：被拦下就抛一个 `Throttled`，调用方的 catch 会拿到它。
 *
 * 给那些已经把逻辑写在 `try/catch` 里的地方用，省得每个调用点都写一遍 if。
 */
export class Throttled extends Error {
  constructor(label: string, waitMs: number) {
    super(`${label}太频繁了，请 ${Math.ceil(waitMs / 1000)} 秒后再试`);
    this.name = "Throttled";
  }
}

/** 不重复提示的忙碌标记：同一个 key 只允许一个在飞。 */
const inFlight = new Set<string>();

/** 标记「开始 / 结束」一件在飞的事（和冷却独立：冷却管频率，它管重入）。 */
export function begin(key: string): boolean {
  if (inFlight.has(key)) return false;
  inFlight.add(key);
  return true;
}

export function end(key: string): void {
  inFlight.delete(key);
}
