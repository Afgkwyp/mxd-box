export interface MemoryPayload {
  total_mb: number;
  used_mb: number;
  percent: number;
  threshold: number;
  is_warning: boolean;
}

/**
 * 当前频道（几线）。
 *
 * 来源不是游戏画面 —— 怀旧服只在换线窗口里显示线号，磁盘上不落记录；但游戏
 * 是「一条线一个服务端端口」，后端查系统 TCP 表把远端端口换算成线号
 * （见 `src-tauri/src/channel.rs`）。
 */
export interface ChannelState {
  /** null = 现在不知道（游戏没开 / 这条服务器节点还没校准过） */
  channel: number | null;
  /** 区服名（绿水灵 / 蓝蜗牛 …）；null = 这台节点不在已录入的区服里 */
  server: string | null;
  /** auto 自动读到 / manual 手动定过 / uncalibrated 节点还没校准 / stale 上次记录 */
  source: "auto" | "manual" | "uncalibrated" | "stale";
  /** 远端服务器地址（校准按节点记） */
  host: string | null;
  /** 远端端口：线号 = 端口 − base */
  port: number | null;
  /** 这个节点学到的基准；null = 还没校准 */
  base: number | null;
  /** 这条线最后一次确认还连着的时间；stale 时就是发现连接没了的时刻 */
  updated_unix: number;
  /** 最近待过的线：新的在前，不含现在这条 */
  recent: ChannelVisit[];
}

/** 待过、已经离开的一条线 —— 掉线后回想「上次在哪条线」用。 */
export interface ChannelVisit {
  server: string | null;
  channel: number;
  /** 最后一次看到还在这条线的时间 */
  last_seen_unix: number;
  /** true = 连接断了以后才离开的（掉线 / 下线 / 关游戏）；false = 直接换线走的 */
  dropped: boolean;
}

/**
 * 小册子「开服监控」的一次状态变化（站点页面上那个「状态变化」列表）。
 *
 * 站点每分钟自己探一次登录入口，我们接它的结论做开服提醒 —— 这比自己盯一个
 * 可能轮换掉的网关端口更接近「官方开服了没有」。
 */
export interface MonitorHistoryPoint {
  /** 本地时间，例如 "2026-09-21 17:48:01" */
  at: string;
  /** 站点原始状态码 */
  status: string;
  /** 中文说法 */
  label: string;
  /** 是否已确认可以登录 */
  open: boolean;
  /** 那一刻登录入口自己的说法 */
  login_label: string;
  /** 那一刻官方公告的状态 */
  official_label: string;
}

export interface ServerMonitorStatus {
  /** 用户有没有开这个监控 */
  enabled: boolean;
  interval_sec: number;
  /** 站点原始状态码：login_recovered / maintenance / confirming / unavailable / unknown */
  status: string;
  status_label: string;
  /** 是否已确认可以登录（只有 login_recovered 为 true） */
  open: boolean;
  /** 当前状态从什么时候开始的 */
  changed_at: string;
  /** 站点这次采样的时刻 */
  checked_at: string;
  /** 站点这次采样的原始时间戳（epoch 秒）：年龄的算术用它，`checked_at` 是给人看的 */
  checked_at_epoch: number;
  /** 距今多少秒前采到；从未成功过 = null。**每次读都重算**，不是采样时算好的定值 */
  checked_ago_sec: number | null;
  /**
   * 这份结论已经不新鲜：监控已停用、从没拿到过数据，或距上次成功采样超过 5 分钟。
   * 它不改变结论本身（`status` / `open` 照旧保留），只告诉界面：这话已经旧了。
   */
  snapshot_stale: boolean;
  official_label: string;
  official_detail: string;
  login_label: string;
  login_detail: string;
  /** 最近的状态变化（新 → 旧） */
  history: MonitorHistoryPoint[];
  /** 抓取失败的原因（网络不通 / 站点报错），成功时为 null */
  error: string | null;
  /** 数据来源说明 */
  source: string;
}

/** 一次开服提醒的内容（右下角提醒窗里显示的就是它）。 */
export interface AlertPayload {
  title: string;
  body: string;
  /** 触发时刻，例如 "11:16:01" */
  at: string;
  /** "monitor"（开服监控）/ "test"（测试按钮）/ "clipboard"（黑名单命中）/ "meso"（金价到价） */
  source: string;
  source_label: string;
}

export interface MarketItem {
  id: string;
  name: string;
  icon_url: string;
  server_name: string;
  lowest_price: string;
  detail_link: string;
  category: string;
}

export interface ServerInfo {
  id: number;
  name: string;
}

/** 图鉴页上的一条「标签 / 数值」。 */
export interface ItemFact {
  label: string;
  value: string;
}

/** 装备的基准属性，例如 物理防御力 +10（9-12）。 */
export interface ItemProperty {
  label: string;
  value: string;
  range: string | null;
}

/** 小册子道具图鉴上的官方资料（装备即基准属性 + NPC 出售价）。 */
export interface ItemDetail {
  id: string;
  name: string;
  icon_url: string;
  equipment: boolean;
  reqs: ItemFact[];
  jobs: string[];
  facts: ItemFact[];
  props: ItemProperty[];
  upgrade: string;
  sell_price: string;
}

/** 一次物价查询的结果，带“数据从哪来”。 */
export interface MarketQueryResult {
  items: MarketItem[];
  /** "live" = 本次真的去小册子抓的；"cache" = 命中的本地缓存 */
  source: "live" | "cache";
  /** source = "cache" 时，这份缓存是几秒前写进去的 */
  cached_seconds_ago: number | null;
}

/** 当前正在运行的二进制身份，用来确认“到底在跑哪个构建”。 */
export interface BuildInfo {
  version: string;
  /** 二进制文件时间，例如 "2026-09-22 02:15:03" */
  built_at: string;
  exe_path: string;
}

export interface MesoRate {
  server_name: string;
  wan_rate: string;
  yuan_rate: string;
  package_price: string;
  stock: string;
}

export interface MesoHistoryPoint {
  /** 展示用时间标签，例如 "09-22 01:00" */
  at: string;
  /** 原始 RFC3339 时间戳 */
  raw: string;
  /** 区服名 -> 1 元可兑换万金 */
  values: Record<string, number>;
}

export interface MesoHistory {
  /** 折线顺序，固定按站点区服表排列 */
  series: string[];
  hour: MesoHistoryPoint[];
  day: MesoHistoryPoint[];
}

/** 金价页一次要的全部数据：当前报价 + 历史走势快照。 */
export interface MesoReport {
  latest: MesoRate[];
  history: MesoHistory;
}

/**
 * 掉落速查（小册子的公开接口 `/api/drop-search.php`，不需要登录）。
 *
 * 字段名是后端归一化之后的写法（站点原始键是 `itemid` / `mobid` / `mapid`）。
 */
export interface DropItemRef {
  itemId: number;
  name: string;
  icon: string;
  reqLevel: number;
  mainCategory: string;
  subCategory: string;
  /** 小册子道具图鉴页（后端拼好的绝对地址） */
  pageUrl: string;
}

export interface DropMob {
  mobId: number;
  name: string;
  level: number;
  /** 1 = BOSS（站点给的是数字，后端已归一成布尔） */
  boss: boolean;
  categoryLabel: string;
  icon: string;
  /** HP / EXP 在站点那边是字符串，而且老怪物常常整块缺失 */
  hp: string;
  exp: string;
  pad: string;
  mad: string;
  pageUrl: string;
}

export interface DropEntry {
  item: DropItemRef;
  chance: number;
  /** 站点算好的可读概率，例如 "0.007%" —— 直接显示这个 */
  chanceText: string;
  min: number;
  max: number;
  /** 是否命中你查的那个关键词 */
  matched: boolean;
  questid: number;
}

export interface DropMap {
  mapId: number;
  /** 名字里自带怪物数量，例如「智慧森林（30只）」 */
  name: string;
  street: string;
  icon: string;
  pageUrl: string;
}

export interface DropMobResult {
  mob: DropMob;
  drops: DropEntry[];
  maps: DropMap[];
  matchedDropCount: number;
  reasons: string[];
}

export interface DropSearchResult {
  ok: boolean;
  meta: {
    keyword: string;
    page: number;
    pageSize: number;
    total: number;
    totalPages: number;
    dropTotal: number;
    boss: boolean;
    source: string;
    sourceLabel: string;
  };
  matches: {
    items: DropItemRef[];
    mobs: DropMob[];
    itemTotal: number;
    mobTotal: number;
  };
  results: DropMobResult[];
}

export interface BlacklistEntry {
  id?: number;
  server: string;
  player_name: string;
  category: string;
  reason: string;
  created_at: string;
}

/**
 * 查一个人的结论。
 *
 * 刻意不是布尔值：
 * * `hit` 同名 + 同服，可以当真；
 * * `other_server` 同名但记录在别的服 —— 同名很常见，不能判死；
 * * `similar` 名字很像（互相包含），疑似小号式改名；
 * * `clear` 本地库没有，**不等于这人干净**。
 */
export interface BlacklistVerdict {
  status: "hit" | "other_server" | "similar" | "clear";
  checked_name: string;
  checked_server: string;
  total_local: number;
  entry: BlacklistEntry | null;
  related: BlacklistEntry[];
  note: string;
}

export interface AppSettings {
  memory_threshold: number;
  /** 呼出 / 收起**查价悬浮窗**的全局快捷键（默认 Alt+F） */
  hotkey: string;
  /** 呼出 / 收起**精简悬浮窗**（金价 + 内存 + 经验）的全局快捷键（默认 F10） */
  lite_float_hotkey: string;
  /** 经验统计「开始 / 暂停」的全局快捷键（默认 Ctrl+Alt+P） */
  exp_hotkey: string;
  /** 全局预设区服（小册子区服表里的 id）：拍卖查询与悬浮窗都默认用它 */
  default_server_id: number;
  /** 是否订阅小册子「开服监控」（站点每分钟探一次登录入口） */
  monitor_enabled: boolean;
  monitor_interval_sec: number;
  auth_cookie: string;
  /** true = 关闭主窗口后只退到托盘常驻；false = 直接结束进程 */
  close_to_tray: boolean;
  /** 剪贴板嗅探：复制到像角色名的文本时自动比对黑名单，确定命中就响铃 + 弹提醒窗 */
  blacklist_watch: boolean;
}

/**
 * 一次版本自检的结果。
 *
 * `error` 有值 = **没能确认**（连不上 / 页面上找不到版本号），
 * 这和「已是最新」是两件事，界面上必须分开说 —— 把「没问到」说成「已是最新」
 * 等于告诉用户“不用更新”，而事实是我们根本不知道。
 */
export interface UpdateInfo {
  current: string;
  latest: string | null;
  has_update: boolean;
  download_url: string | null;
  notes: string | null;
  /**
   * 这次是怎么读出来的：`"json"`（读的清单）/ `"quark"`（夸克网盘分享接口）/
   * `"page"`（静态页面里的文件名）/ `"none"`（没读成）
   */
  source: "json" | "quark" | "page" | "none";
  error: string | null;
}

/** 全局快捷键能绑的动作。 */
export type HotkeyAction = "price_hud" | "lite_float" | "exp_toggle";

/** 某个悬浮窗快捷键的注册结果（界面要能看见「这个键到底注册上了没有」）。 */
export interface HotkeyStatus {
  /** "price_hud"（查价悬浮窗） / "lite_float"（精简悬浮窗） / "exp_toggle"（经验统计开始/暂停） */
  action: HotkeyAction;
  /** 中文名 */
  label: string;
  /** 实际在用的写法，例如 "F10" */
  hotkey: string;
  registered: boolean;
  /** 注册失败时的一句话（不认识 / 被别的程序占用 / 两个窗撞键） */
  error: string | null;
  /** 本版默认值，界面的「恢复默认」用它 */
  default_hotkey: string;
}

/**
 * 一段会话的数据可信度。
 *
 * 一个「每小时 30 万」的数字，如果有一半时间是游戏被别的窗口挡着的，那它既不是
 * 真的也不假 —— 它只是**不能拿去比较**。所以每段都给一个结论 + 一句理由。
 *
 * 注意 `activeRatio`（真在涨经验的时间占比）和 `coverage`（能读到画面的时间占比）
 * 是两件事：前者是玩法（挂机多不多），后者才是数据可不可信。界面分开说。
 */
export interface ExpSessionQuality {
  /** 2 = 数据完整 / 1 = 可参考 / 0 = 不建议用于比较 */
  grade: number;
  label: string;
  reason: string;
  /** 能读到画面的时间占比（0~1） */
  coverage: number;
  /** 真正在涨经验的时间占比（0~1） */
  active_ratio: number;
  /** 读到了结构但没过校验的帧数（“数学拒绝帧”） */
  rejected_frames: number;
  /** 文字和同画面的经验条对不上、被交叉验证拒掉的帧数 */
  bar_conflicts: number;
  /** 增量大得不像真的、被丢掉的次数 */
  gated_gains: number;
}

/**
 * 一段经验会话的实时状态（`get_exp_status` 里的 `session`）。
 *
 * 「有效时间」把暂停那几段扣掉了 —— 吃饭时按暂停，回来后这一段的平均速率
 * 不会被那两个小时拉低。
 */
export interface ExpSessionStatus {
  started_at: string;
  /**
   * 开始 / 结束的**墙钟**时刻（秒级时间戳）。小结卡片上那个时间段用它 ——
   * 它和 `active_secs`（有效时间，暂停那段不算）不是一回事。
   * 结束时刻在点「结束本段」那一刻就冻结，之后不会随时间漂移。
   */
  started_unix: number;
  ended_unix: number;
  /** 会话内有效时间（秒），暂停的时间不算 */
  active_secs: number;
  gained_exp: number;
  start_level: number | null;
  current_level: number | null;
  start_percent: number | null;
  current_percent: number | null;
  level_ups: number;
  /** 这一段自己的平均每小时（按有效时间算） */
  per_hour: number | null;
  /** 到目前的读数质量（段落中的实时结论） */
  quality: ExpSessionQuality;
  /** 当前累计经验（从 1 级 0 经验算起）；定不出等级时是 null */
  cumulative: number | null;
  /** 按升级分段的效率（含进行中的一段；没升过级时只有一个元素） */
  stages: ExpStagePoint[];
  /** 预计还要多久升级（秒）；等级或速率定不出来时是 null */
  eta_secs: number | null;
  /** 距升级还差多少经验（定不出等级时是 null） */
  remaining_exp: number | null;
}

/** 一段「阶段效率」：一次升级 → 下一次升级之间的速率。 */
export interface ExpStagePoint {
  /** 这一阶段的等级 */
  level: number;
  /** 阶段开始时的会话内有效时间（秒） */
  started_at_secs: number;
  /** 阶段结束时的会话内有效时间（秒） */
  ended_at_secs: number;
  /** 本阶段获得的净经验 */
  gained: number;
  /** 本阶段的有效时长（秒） */
  secs: number;
  /** 每小时（阶段太短或没有收益时是 null） */
  per_hour: number | null;
  /** 阶段是否还在进行 */
  ongoing: boolean;
}

/** 结束之后等用户确认「计入历史吗」的那一段。 */
export interface ExpPending {
  active_secs: number;
  gained_exp: number;
  quality: ExpSessionQuality;
  /**
   * 结束瞬间冻结的整段结论（等级区间、时速、分段、升级次数）。
   *
   * 会话一结束 `session` 就空了，没有这份快照，确认框里就只剩两个数 ——
   * 想看一眼"这一段打得怎么样"或生成小结卡片，数据就已经不在手上了。
   */
  summary: ExpSessionStatus;
}

/**
 * 经验统计的当前状态（每秒从 Rust 读一次，也有 `exp-update` 事件推送）。
 *
 * `read_state` 为 `ok` 之外的值时，界面必须**明说原因**：读不到就是读不到，
 * 绝不能拿上一个值凑合 —— 那会让一条平坦的经验曲线看起来像「你在挂机」。
 */
export interface ExpStatus {
  /** idle（没开始）/ running（统计中）/ paused（已暂停） */
  phase: string;
  phase_label: string;
  /**
   * ok / no_window / minimized / capture_failed / blank_frame / no_field / contradiction
   */
  read_state: string;
  /** 一句话：出错时是原因 + 怎么办 */
  read_message: string;
  /** 认不出的原始串（只有读数和经验表对不上时才有） */
  raw: string | null;
  exp: number | null;
  percent: number | null;
  level: number | null;
  last_read_at: string | null;
  /** 最近一分钟 / 最近一小时的经验（按实际测量区间折算） */
  per_minute: number | null;
  per_hour: number | null;
  /** 两个数各自实际测了多久（秒）—— 不满一分钟/一小时时要显示出来 */
  minute_span_secs: number;
  hour_span_secs: number;
  /** 连续多久没有新收益（秒） */
  idle_secs: number;
  /** 恢复后效率：一段「读不到画面」之后，从恢复的第一个可信帧起算的每小时 */
  recovery_per_hour: number | null;
  /** 恢复后效率实际测了多久（秒） */
  recovery_span_secs: number;
  /** 停手了：界面要把数字置灰写「已停止」，而不是看着它慢慢掉到 0 */
  stopped: boolean;
  session: ExpSessionStatus | null;
  /** 当前这一段的读数质量；没在统计时为 null */
  quality: ExpSessionQuality | null;
  /** 当前累计经验（从 1 级 0 经验算起）；定不出等级时是 null */
  cumulative: number | null;
  /** 待确认是否计入历史的那一段 */
  pending: ExpPending | null;
  window_title: string | null;
  /**
   * 现在在哪个地图。
   *
   * **不是认字认出来的** —— 小地图上那几个字只有 11 像素高，OCR 连干净的
   * 12 像素黑字都会读错（实测「坠落主义」读成「坐落主义」），认错一个地名比不显示更糟。
   * 这里记的是小地图那一块的**像素指纹**：每张新地图你填一次名字，之后就一直是自动的。
   */
  map_name: string | null;
  /** 遇到一张没认过的地图：界面请他填一次（填完以后就一直自动） */
  map_unknown: boolean;
  /**
   * 这一次抓屏走的是哪条路：
   * * `window` = **后台截屏**（抓窗口自己，游戏被别的窗口压住也照读）；
   * * `screen` = 兜底从屏幕抓（只在后台截屏失败时出现，那时游戏必须在前台）。
   */
  capture_method: string | null;
  /** 内置字形表里认识的字符（正常情况下永远是 `%().0123456789EPX`） */
  font_symbols: string;
  /** 当前用的校准框（比例坐标 + 来源）；没有校准时为 null（整幅画面自动定位） */
  region: RegionProfile | null;
  /** 手动框连续读不到：界面提示「校准可能失效，重新框一次」 */
  region_lost: boolean;
  /** 一次性提示（升级 / 换角色 / 掉经验） */
  notice: string | null;
  /** 练级目标的进度；没设目标时为 null */
  goal: ExpGoal | null;
}

/** 练级目标：到目标等级还差多少、按本段时速还要多久。 */
export interface ExpGoal {
  target_level: number;
  /** 到「目标等级 0%」还差的经验；现在的等级还没读到时为 null，已经到了是 0 */
  remaining_exp: number | null;
  /** 按本段平均时速还要多久（秒）；没在统计 / 没涨经验 / 已经到了都是 null */
  eta_secs: number | null;
  reached: boolean;
}

/** 金价到价提醒的设置（`src-tauri/src/meso_alert.rs`）。 */
export interface MesoAlertConfig {
  enabled: boolean;
  /** 1 元换的万金 ≥ 这个数时提醒；null = 不盯这个方向 */
  above: number | null;
  /** 1 元换的万金 ≤ 这个数时提醒；null = 不盯这个方向 */
  below: number | null;
}

/** 历史里的一段会话。 */
export interface ExpHistoryRow {
  id: number;
  started_unix: number;
  ended_unix: number;
  active_secs: number;
  start_level: number | null;
  end_level: number | null;
  start_percent: number | null;
  end_percent: number | null;
  gained_exp: number;
  /** 2 = 数据完整 / 1 = 可参考 / 0 = 不建议用于比较 */
  quality: number;
  quality_reason: string;
  coverage: number;
  idle_ratio: number;
  /** 这一段的练级地图（用户自己填的；没填是空串） */
  map_name: string;
}

/** 历史汇总（「共 N 段 · 累计练了多久 · 一共多少经验」）。 */
export interface ExpTotals {
  sessions: number;
  active_secs: number;
  gained_exp: number;
}

/** `get_exp_history` 的返回：列表 + 汇总。 */
export interface ExpHistoryPayload {
  rows: ExpHistoryRow[];
  totals: ExpTotals;
}

/** 曲线上的一个采样点（每分钟一个）。 */
export interface ExpCurvePoint {
  captured_unix: number;
  exp: number;
  percent: number;
  level: number | null;
}

/** 提醒历史的一行（alert_history 表）。kind: server_open / blacklist_hit / monitor / test */
export interface AlertHistoryRow {
  id: number;
  kind: string;
  title: string;
  detail: string;
  fired_unix: number;
}

/**
 * 一条校准记录（region_profiles 表）里的**比例矩形**：x / y / w / h 都是
 * 相对客户区的比例（0..1），与分辨率 / DPI 无关。
 */
export interface NormRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/**
 * 一条校准记录（region_profiles 表）。
 *
 * **按比例记，不按分辨率分开记**：经验行跟着游戏分辨率 / 界面缩放走，
 * 同一个比例框在 1080p / 1440p / 4K 下会落到同一行字上。
 * 读不出来时会自动退回整幅画面重新定位，不需要第二套坐标。
 */
export interface RegionProfile {
  kind: string;
  rect: NormRect;
  /** 'auto' = 自动定位学的 / 'manual' = 用户手动框的（自动档不会被自动覆盖） */
  source: string;
  /** 学到的字形高度（客户区物理像素）；拿不到时为 null */
  text_height: number | null;
  learned_unix: number;
}

/** 一次校准测试 / 自动识别的结果（test_exp_region / auto_calibrate_exp 命令）。 */
export interface CalibrationTest {
  ok: boolean;
  message: string;
  raw: string | null;
  exp: number | null;
  percent: number | null;
  level: number | null;
  text_height: number | null;
  /** 成功时：测出来 / 学到的框 */
  profile: RegionProfile | null;
}

/** 校准蒙版窗的几何（calibration_overlay_geometry 命令）。 */
export interface CalibrationGeometry {
  /** 游戏客户区尺寸（物理像素）—— 蒙版窗正好盖这么大一块 */
  client_w: number;
  client_h: number;
  /** 已有的校准档（打开时先把它画出来，方便微调而不是从头拖） */
  profile: RegionProfile | null;
}

/**
 * 校准用的一帧游戏画面（calibration_frame 命令的返回）。
 *
 * png 是客户区整幅画面按 max_width 等比缩小的结果 —— 所以 image_w/image_h
 * 与 client_w/client_h 之比就是「图像像素 → 客户区像素」的换算系数，
 * 前端鼠标坐标换算全靠这一对（见 CalibrationDialog 顶部的三层等式）。
 */
export interface CalibrationFrame {
  png_base64: string;
  image_w: number;
  image_h: number;
  client_w: number;
  client_h: number;
  /** "Windowed" | "Framed" | "Borderless" —— 顺手给用户说清我们抓的是哪种窗口 */
  screen_mode: string;
}
