/**
 * 开发用的 Tauri 模拟层：在普通浏览器里打开 `http://127.0.0.1:5173/?window=hud&mock` 就能看三个窗口，
 * 用来调界面与「拖动窗口大小时内容怎么自适应」（浏览器窗口大小 = 窗口大小）。
 *
 * 只在 `vite dev` 下、URL 带 `mock` 时加载（见 main.tsx），不会进正式构建。
 * 数据是编出来的示例，不代表真实行情。
 * 可选参数：`phase=idle`（未开始）、`unreadable=1`（读不到经验条）、`unreadable=away`（游戏没开）、`warn=1`（内存告警）。
 */
/* eslint-disable @typescript-eslint/no-explicit-any */
const w = window as any;
const params = new URLSearchParams(location.search);
const label = params.get("window") ?? "main";

const callbacks = new Map<number, (payload: unknown) => void>();
const listeners = new Map<string, Set<number>>();
let nextId = 1;

function emit(event: string, payload: unknown) {
  listeners.get(event)?.forEach((id) => callbacks.get(id)?.({ event, id, payload }));
}

let phase: "idle" | "running" | "paused" = params.get("phase") === "idle" ? "idle" : "running";
let activeSecs = 2538;
// `goal=190` 带一个练级目标进来；页面上改了也记在这里
let goalLevel: number | null = params.get("goal") ? Number(params.get("goal")) : null;
let mesoAlert = { enabled: false, above: null as number | null, below: null as number | null };
let gained = 18_350_000;
// 「结束本段」之后等用户确认的那一段（看分享卡片的入口要它）
let pending: unknown = null;

/** 一张编出来的「游戏画面」：框角色形象时用（真机上是 calibration_frame 抓的客户区） */
function fakeFrame() {
  const canvas = document.createElement("canvas");
  canvas.width = 1024;
  canvas.height = 768;
  const ctx = canvas.getContext("2d")!;
  const sky = ctx.createLinearGradient(0, 0, 0, 768);
  sky.addColorStop(0, "#9fc4e8");
  sky.addColorStop(1, "#e9eef2");
  ctx.fillStyle = sky;
  ctx.fillRect(0, 0, 1024, 768);
  ctx.fillStyle = "#c8a36b";
  ctx.fillRect(0, 470, 1024, 40);
  ctx.fillStyle = "#8a6a43";
  ctx.fillRect(0, 510, 1024, 258);
  // 角色：几块色块拼的小人，站在画面中间偏下
  ctx.fillStyle = "#7fb7e6";
  ctx.fillRect(488, 380, 48, 34);
  ctx.fillStyle = "#ffe2c4";
  ctx.fillRect(494, 404, 36, 26);
  ctx.fillStyle = "#f4f4f4";
  ctx.fillRect(496, 430, 32, 40);
  ctx.fillStyle = "#222";
  ctx.fillRect(490, 476, 44, 14);
  const data = canvas.toDataURL("image/png");
  return {
    png_base64: data.slice(data.indexOf(",") + 1),
    image_w: 1024,
    image_h: 768,
    client_w: 1024,
    client_h: 768,
    screen_mode: "Windowed",
  };
}

function expStatus() {
  const running = phase === "running";
  return {
    phase,
    phase_label: phase,
    read_state: params.get("unreadable") === "away" ? "no_window" : params.get("unreadable") ? "no_field" : "ok",
    read_message: "",
    raw: null,
    exp: 1_234_567_890,
    percent: 47.32,
    level: 187,
    last_read_at: null,
    per_minute: running ? 434_000 : 0,
    per_hour: running ? 26_030_000 : 0,
    minute_span_secs: 60,
    hour_span_secs: 3600,
    idle_secs: 0,
    recovery_per_hour: null,
    recovery_span_secs: 0,
    stopped: false,
    session:
      phase === "idle"
        ? null
        : {
            started_at: "",
            started_unix: 0,
            ended_unix: 0,
            active_secs: activeSecs,
            gained_exp: gained,
            start_level: 187,
            current_level: 187,
            start_percent: 40.1,
            current_percent: 47.32,
            level_ups: 0,
            per_hour: 26_030_000,
            quality: { grade: 2, label: "数据完整", reason: "", coverage: 1, active_ratio: 1, rejected_frames: 0, bar_conflicts: 0, gated_gains: 0 },
            cumulative: 1_234_567_890,
            stages: [
              { level: 187, started_at_secs: 0, secs: 2538, per_hour: 26_030_000, ongoing: true },
            ],
            eta_secs: 16_860,
            remaining_exp: 121_900_000,
          },
    quality: { grade: 1, label: "可参考", reason: "画面偶有遮挡", coverage: 0.93, active_ratio: 1, rejected_frames: 0, bar_conflicts: 0, gated_gains: 0 },
    cumulative: 1_234_567_890,
    pending,
    window_title: null,
    map_name: "冰封雪域",
    map_unknown: false,
    capture_method: "window",
    font_symbols: "%().0123456789EPX",
    region: null,
    region_lost: false,
    character: { id: 1, name: "枫叶牧师", job: "牧师" },
    goal:
      goalLevel == null
        ? null
        : { target_level: goalLevel, remaining_exp: 318_400_000, eta_secs: running ? 44_040 : null, reached: false },
  };
}

/** `channel=live`（默认，掉线重登后在别的线）/ `switched`（自己换的线）/ `stale`（游戏没开）/ `unknown`（认不出线号）/ `none`（从没连过） */
function channelState() {
  const now = Math.floor(Date.now() / 1000);
  const mode = params.get("channel") ?? "live";
  const recent = [
    { server: "绿水灵", channel: 47, last_seen_unix: now - 420, dropped: true },
    { server: "绿水灵", channel: 12, last_seen_unix: now - 5400, dropped: false },
    { server: "蓝蜗牛", channel: 8, last_seen_unix: now - 90_000, dropped: false },
  ];
  const live = { channel: 3, server: "绿水灵", source: "auto", host: "43.142.194.76", port: 8587, base: null, updated_unix: now, recent };
  if (mode === "switched") return { ...live, recent: recent.slice(1) };
  if (mode === "stale") return { ...live, source: "stale", host: null, port: null, channel: 47, updated_unix: now - 420, recent: recent.slice(1) };
  if (mode === "unknown") return { ...live, source: "uncalibrated", channel: null, server: null, host: "10.0.0.8", port: 8484 };
  if (mode === "none") return { ...live, source: "stale", channel: null, server: null, host: null, port: null, recent: [] };
  return live;
}

const servers = [
  { id: 1, name: "蓝蜗牛" },
  { id: 2, name: "蘑菇仔" },
  { id: 3, name: "绿水灵" },
  { id: 4, name: "漂漂猪" },
  { id: 5, name: "小白兔" },
];
const rates: Record<string, number> = { 蓝蜗牛: 8.94, 蘑菇仔: 8.62, 绿水灵: 8.31, 漂漂猪: 8.47, 小白兔: 8.05 };

function mesoReport() {
  const series = servers.map((s) => s.name);
  const hour = Array.from({ length: 36 }, (_, i) => ({
    at: `09-30 ${String(i % 24).padStart(2, "0")}:00`,
    raw: "",
    values: Object.fromEntries(
      series.map((n, k) => [n, +(rates[n] - 0.25 + Math.sin(i / 4 + k) * 0.12 + i * 0.008).toFixed(2)]),
    ),
  }));
  return {
    latest: series.map((n) => ({
      server_name: n,
      wan_rate: rates[n].toFixed(2),
      yuan_rate: "0.116",
      package_price: "50",
      stock: "1.2 亿",
    })),
    history: { series, hour, day: hour },
  };
}

const items = [
  { id: "1092000", name: "锅盖", icon_url: "", server_name: "蘑菇仔", lowest_price: "1,380 万", detail_link: "", category: "盾牌" },
  { id: "1092001", name: "银色锅盖", icon_url: "", server_name: "蘑菇仔", lowest_price: "2,950 万", detail_link: "", category: "盾牌" },
  { id: "1092002", name: "金色锅盖", icon_url: "", server_name: "蘑菇仔", lowest_price: "1.1 亿", detail_link: "", category: "盾牌" },
  { id: "1092003", name: "锅盖（红）", icon_url: "", server_name: "蘑菇仔", lowest_price: "720 万", detail_link: "", category: "盾牌" },
  { id: "1092004", name: "锅盖（蓝）", icon_url: "", server_name: "蘑菇仔", lowest_price: "690 万", detail_link: "", category: "盾牌" },
];

// 练级历史（可以在页面上逐条删）
/** 两个号：历史里单数行算大号的、双数行算小号的 */
let characterRows = [
  { id: 1, name: "枫叶牧师", job: "牧师", level: 187, exp: 1_234_567_890, percent: 47.32, last_seen_unix: 1790710000 },
  { id: 2, name: "小号abc", job: "刺客", level: 43, exp: 61_200, percent: 8.5, last_seen_unix: 1790606000 },
];
const characterOf = (rowId: number) =>
  characterRows.some((item) => item.id === 2 - (rowId % 2)) ? 2 - (rowId % 2) : null;

let historyRows = [
      { id: 1, started_unix: 1790700000, ended_unix: 1790710000, active_secs: 9800, gained_exp: 72_000_000, start_level: 186, start_percent: 61.2, end_level: 187, end_percent: 12.4, map_name: "冰封雪域", quality: 2, quality_reason: "", coverage: 0.99, idle_ratio: 0.08 },
      { id: 2, started_unix: 1790600000, ended_unix: 1790606000, active_secs: 5600, gained_exp: 39_000_000, start_level: 186, start_percent: 20.1, end_level: 186, end_percent: 58.9, map_name: "", quality: 1, quality_reason: "画面偶有遮挡", coverage: 0.9, idle_ratio: 0.3 },
      { id: 3, started_unix: 1790500000, ended_unix: 1790504000, active_secs: 3900, gained_exp: 24_500_000, start_level: 185, start_percent: 88.3, end_level: 186, end_percent: 20.1, map_name: "死亡之林", quality: 2, quality_reason: "", coverage: 0.98, idle_ratio: 0.12 },
      { id: 4, started_unix: 1790400000, ended_unix: 1790403000, active_secs: 2700, gained_exp: 15_200_000, start_level: 185, start_percent: 61.0, end_level: 185, end_percent: 88.3, map_name: "死亡之林", quality: 0, quality_reason: "一半时间读不到画面", coverage: 0.52, idle_ratio: 0.2 },
      { id: 5, started_unix: 1790300000, ended_unix: 1790308000, active_secs: 7200, gained_exp: 40_000_000, start_level: 185, start_percent: 12.0, end_level: 185, end_percent: 61.0, map_name: "死亡之林", quality: 2, quality_reason: "", coverage: 0.97, idle_ratio: 0.15 },
];

const handlers: Record<string, (args: any) => unknown> = {
  get_settings: () => ({
    memory_threshold: 85,
    hotkey: "Alt+F",
    lite_float_hotkey: "F10",
    exp_hotkey: "Ctrl+Alt+P",
    default_server_id: 2,
    monitor_enabled: true,
    monitor_interval_sec: 30,
    auth_cookie: "",
    close_to_tray: true,
    blacklist_watch: true,
  }),
  get_server_list: () => servers,
  get_exp_status: () => expStatus(),
  set_exp_goal: ({ level }: any) => ((goalLevel = level ?? null), expStatus()),
  get_meso_alert: () => mesoAlert,
  set_meso_alert: ({ config }: any) => (mesoAlert = config),
  start_exp_session: () => ((phase = "running"), expStatus()),
  resume_exp_session: () => ((phase = "running"), expStatus()),
  pause_exp_session: () => ((phase = "paused"), expStatus()),
  end_exp_session: () => {
    const { session, quality } = expStatus();
    const now = Math.floor(Date.now() / 1000);
    const summary = { ...session, started_unix: now - activeSecs - 600, ended_unix: now };
    pending = { active_secs: activeSecs, gained_exp: gained, quality, summary };
    phase = "idle";
    return summary;
  },
  resolve_exp_session: ({ save }: any) => ((pending = null), save ? "已计入历史" : "已丢弃"),
  calibration_frame: fakeFrame,
  save_share_card: () => "C:\\Users\\me\\Pictures\\枫之助\\冰封雪域-经验小结-20261002-203100.png",
  copy_share_card: () => null,
  get_meso_report: mesoReport,
  get_memory_status: () => ({
    total_mb: 16384,
    used_mb: 7412,
    percent: params.get("warn") ? 91 : 45.2,
    threshold: 85,
    is_warning: !!params.get("warn"),
  }),
  get_channel_state: channelState,
  get_server_monitor_status: () => ({
    enabled: true,
    interval_sec: 30,
    status: "open",
    status_label: "开服中",
    open: true,
    changed_at: "",
    checked_at: "14:32",
    checked_at_epoch: 1,
    checked_ago_sec: 20,
    snapshot_stale: false,
    official_label: "",
    official_detail: "",
    login_label: "",
    login_detail: "",
    history: [],
    error: null,
    source: "小册子",
  }),
  get_build_info: () => ({ version: "0.2.0", built_at: "2026-09-30 14:00:00", exe_path: "C:\\枫之助.exe" }),
  check_for_update: () => ({
    current: "0.2.0",
    latest: "0.2.1",
    has_update: true,
    download_url: "https://example.com",
    notes: null,
    source: "json",
    error: null,
  }),
  take_version_greeting: () => null,
  get_hotkey_status: () => [
    { action: "price_hud", label: "查价悬浮窗", hotkey: "Alt+F", registered: true, error: null, default_hotkey: "Alt+F" },
    { action: "lite_float", label: "经验窗", hotkey: "F10", registered: true, error: null, default_hotkey: "F10" },
    { action: "exp_toggle", label: "经验开始/暂停", hotkey: "Ctrl+Alt+P", registered: true, error: null, default_hotkey: "Ctrl+Alt+P" },
  ],
  query_market: () => ({ items, source: "live", cached_seconds_ago: null }),
  get_item_detail: ({ itemId }: any) => ({
    id: itemId,
    name: "锅盖",
    icon_url: "",
    equipment: true,
    reqs: [{ label: "需求等级", value: "30" }],
    jobs: ["全职业"],
    facts: [{ label: "分类", value: "盾牌" }],
    props: [{ label: "物理防御力", value: "+10", range: "(9-12)" }],
    upgrade: "可升级 5 次",
    sell_price: "1,200 金币",
  }),
  search_drops: ({ keyword }: any) => ({
    ok: true,
    meta: { keyword, page: 1, pageSize: 12, total: 1, totalPages: 1, dropTotal: 2, boss: false, source: "", sourceLabel: "" },
    matches: { items: [], mobs: [{}] },
    results: [
      {
        mob: { mobId: 1, name: "无魂猴", level: 47, boss: false, categoryLabel: "", icon: "", hp: "3,200", exp: "210", pad: "", mad: "", pageUrl: "" },
        drops: [
          {
            item: { itemId: 1, name: "锅盖", icon: "", reqLevel: 30, mainCategory: "", subCategory: "", pageUrl: "https://mxdc.dvg.cn/item_info.php?id=1" },
            chance: 0.00007,
            chanceText: "0.007%",
            min: 1,
            max: 1,
            matched: true,
            questid: 0,
          },
          {
            // 任务道具：行上不该出现「查价」按钮（没有任何拍卖意义）
            item: { itemId: 4032379, name: "海盗冒险家表彰状", icon: "", reqLevel: 0, mainCategory: "", subCategory: "", pageUrl: "https://mxdc.dvg.cn/item_info.php?id=4032379" },
            chance: 40000,
            chanceText: "4%",
            min: 1,
            max: 1,
            matched: false,
            questid: 2409,
          },
        ],
        maps: [{ mapId: 1, name: "智慧森林（30只）", street: "", icon: "", pageUrl: "" }],
        matchedDropCount: 1,
        reasons: [],
      },
    ],
  }),
  check_blacklist: ({ name }: any) => ({
    status: "clear",
    checked_name: name,
    checked_server: "蘑菇仔",
    total_local: 3482,
    entry: null,
    related: [],
    note: "没有找到同名记录。",
  }),
  get_blacklist: () => [],
  known_exp_maps: () => [],
  get_exp_history: ({ character }: any = {}) => {
    const rows = historyRows
      .filter((row) => character == null || characterOf(row.id) === character)
      .map((row) => ({
        ...row,
        character_id: characterOf(row.id),
        character_name: characterRows.find((item) => item.id === characterOf(row.id))?.name ?? null,
      }));
    return {
      rows,
      totals: {
        sessions: rows.length,
        active_secs: rows.reduce((sum, row) => sum + row.active_secs, 0),
        gained_exp: rows.reduce((sum, row) => sum + row.gained_exp, 0),
      },
    };
  },
  list_exp_characters: () =>
    characterRows.map((item) => {
      const mine = historyRows.filter((row) => characterOf(row.id) === item.id);
      return {
        ...item,
        sessions: mine.length,
        active_secs: mine.reduce((sum, row) => sum + row.active_secs, 0),
        gained_exp: mine.reduce((sum, row) => sum + row.gained_exp, 0),
      };
    }),
  rename_exp_character: ({ id, name }: any) => {
    characterRows = characterRows.map((item) => (item.id === id ? { ...item, name } : item));
    return null;
  },
  delete_exp_character: ({ id }: any) => {
    characterRows = characterRows.filter((item) => item.id !== id);
    return null;
  },
  delete_exp_session: ({ id }: any) => {
    historyRows = historyRows.filter((row) => row.id !== id);
    return null;
  },
  get_exp_curve: () => Array.from({ length: 30 }, (_, i) => ({ captured_unix: 1790700000 + i * 300, exp: 1_100_000_000 + i * i * 400_000 + i * 2_000_000 })),
  get_latest_open_alert: () => null,
  get_update_page_url: () => "",
  get_float_window_visible: () => false,
};

w.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
w.__TAURI_INTERNALS__ = {
  metadata: { currentWindow: { label }, currentWebview: { windowLabel: label, label } },
  transformCallback(cb: (p: unknown) => void) {
    const id = nextId++;
    callbacks.set(id, cb);
    return id;
  },
  unregisterCallback(id: number) {
    callbacks.delete(id);
  },
  convertFileSrc: (p: string) => p,
  async invoke(cmd: string, args: any) {
    if (cmd === "plugin:event|listen") {
      const set = listeners.get(args.event) ?? new Set();
      set.add(args.handler);
      listeners.set(args.event, set);
      return args.handler;
    }
    if (cmd.startsWith("plugin:")) return null;
    const handler = handlers[cmd];
    if (handler) return handler(args ?? {});
    return null; // 其余命令（窗口开关 / 外部链接 …）在模拟里什么都不做
  },
};

// 经验每秒走字，看得出「在记录」
window.setInterval(() => {
  if (phase !== "running") return;
  activeSecs += 1;
  gained += 7_200;
  emit("exp-update", expStatus());
}, 1000);
