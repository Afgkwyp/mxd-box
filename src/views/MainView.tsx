import React, { Suspense, lazy, useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  Activity,
  BellRing,
  DownloadCloud,
  ExternalLink,
  Info,
  LayoutDashboard,
  MessagesSquare,
  Search,
  SlidersHorizontal,
  TrendingUp,
} from "lucide-react";
import { AlertSettings } from "../components/AlertSettings";
import { CalibrationDialog } from "../components/CalibrationDialog";
import { ChannelBadge } from "../components/ChannelBadge";
import { ThemeToggle } from "../components/ThemeToggle";
import { UpdateNotesGreeting } from "../components/VersionGreeting";
import { SubTabs, useSlider, type TabDef } from "../components/ui/kit";
import { OverviewPage, type PageId } from "../pages/OverviewPage";
import { QueryPage } from "../pages/QueryPage";
import { SettingsPage } from "../pages/SettingsPage";
import { AboutPage } from "../pages/AboutPage";
import { EMPTY_MONITOR_STATUS, summarizeMonitor } from "../lib/monitor";
import { useMemorySample } from "../lib/memory";
import { usePresetServer } from "../lib/presetServer";
import { MXDC_HOME, QQ_GROUP_INVITE, QQ_GROUP_NUMBER } from "../lib/links";
import type { BuildInfo, ServerMonitorStatus, UpdateInfo } from "../types";
import mushroomLogo from "../assets/mushroom.png";

/**
 * 主窗口外壳：左侧导航（7 页）+ 常驻状态区 + 内容区。
 *
 * 重设计前是 9 个页签，外加顶栏 6 个每页都挂着的胶囊（频道 / QQ 群 / 开服 / 新版本 / 主题 / 内存）。
 * 现在：
 *  * 页面合并成 7 个：总览 · 行情 · 查询 · 练级 · 提醒 · 设置 · 关于；
 *  * 频道 / 开服 / 内存 / 新版本 / 主题 挪进侧栏底部的「状态区」，不再占顶栏；
 *  * QQ 群、数据来源、更新说明、赞赏码在「关于」页。
 *
 * 窗口窄（< 1000px）时侧栏收成只有图标的窄条，内容区拿走多出来的宽度。
 */

// 「行情」和「练级」两页带图表库（recharts，几百 KB）：点进去才加载，
// 主窗口启动时只画总览，不用先等它们。
const MesoTrends = lazy(() =>
  import("../components/MesoTrends").then((module) => ({ default: module.MesoTrends })),
);
const ExpStats = lazy(() =>
  import("../components/ExpStats").then((module) => ({ default: module.ExpStats })),
);

interface NavItem {
  id: PageId;
  label: string;
  icon: React.ReactNode;
}

const ICON = "w-[17px] h-[17px] shrink-0";
const NAV: NavItem[] = [
  { id: "overview", label: "总览", icon: <LayoutDashboard className={ICON} /> },
  { id: "meso", label: "行情", icon: <TrendingUp className={ICON} /> },
  { id: "query", label: "查询", icon: <Search className={ICON} /> },
  { id: "exp", label: "练级", icon: <Activity className={ICON} /> },
  { id: "alert", label: "提醒", icon: <BellRing className={ICON} /> },
  { id: "settings", label: "设置", icon: <SlidersHorizontal className={ICON} /> },
  { id: "about", label: "关于与更新说明", icon: <Info className={ICON} /> },
];

const TITLES: Record<PageId, string> = {
  overview: "总览",
  meso: "行情",
  query: "查询",
  exp: "练级",
  alert: "提醒",
  settings: "设置",
  about: "关于与更新说明",
};

/** 开服状态的小圆点颜色。 */
const TONE_DOT: Record<string, string> = {
  open: "bg-emerald-400",
  maintenance: "bg-amber-400",
  error: "bg-red-400",
  pending: "bg-sky-400",
  unknown: "bg-slate-500",
};
/** 状态区里只放一个词（完整说法在悬停提示里）—— 站点文案很长，塞进窄侧栏会折行。 */
const TONE_WORD: Record<string, string> = {
  open: "开服中",
  maintenance: "维护中",
  error: "取不到数据",
  pending: "确认中",
  unknown: "未知",
};
const TONE_TEXT: Record<string, string> = {
  open: "text-emerald-400",
  maintenance: "text-amber-400",
  error: "text-red-400",
  pending: "text-sky-400",
  unknown: "text-slate-400",
};

export const MainView: React.FC = () => {
  const [page, setPage] = useState<PageId>("overview");
  /** 总览「最近查价」点词跳转：查询页据此重查（nonce 变了才算新的一次） */
  const [seed, setSeed] = useState<{ term: string; nonce: number } | undefined>();
  const memory = useMemorySample();
  /** 右上角的预设区服：全局设置，每一页都用得到，所以挂在外壳上 */
  const { serverId, servers, chooseServer, loading: serverLoading } = usePresetServer();
  const serverTabs = useMemo<TabDef<string>[]>(
    () => servers.map((server) => ({ key: String(server.id), label: server.name })),
    [servers],
  );
  const [monitorStatus, setMonitorStatus] = useState<ServerMonitorStatus>(EMPTY_MONITOR_STATUS);
  const monitor = summarizeMonitor(monitorStatus);
  /** 当前在跑的二进制身份（界面上那个版本号要显示真值） */
  const [build, setBuild] = useState<BuildInfo | null>(null);
  /**
   * 版本自检：**只在真的有新版本时才显示任何东西**。没有新版本 / 没配更新页 / 连不上，
   * 界面上一律不出现 —— 一个只在真有更新时冒出来的小胶囊比天天挂着的「已是最新」有用。
   */
  const [update, setUpdate] = useState<UpdateInfo | null>(null);
  /**
   * 校准浮层挂在这一层（而不是 ExpStats 里）：① 切走页面时它不该跟着卸载；
   * ② 经验窗（F10）点「校准位置」时，主窗口可能停在别的页 —— 由这里统一「切到练级 + 打开浮层」。
   */
  const [calibrating, setCalibrating] = useState(false);

  useEffect(() => {
    invoke<ServerMonitorStatus>("get_server_monitor_status")
      .then(setMonitorStatus)
      .catch(() => {});
    const unlistenMonitor = listen<ServerMonitorStatus>("server-monitor-update", (event) => {
      setMonitorStatus(event.payload);
    });

    // 经验窗里的「校准位置」：切到练级页并打开校准浮层。
    // 悬浮窗优先直接开蒙版；开不出来才广播这个事件退回截图浮层。
    const unlistenCalibration = listen("open-exp-calibration", () => {
      setPage("exp");
      setCalibrating(true);
    });

    invoke<BuildInfo>("get_build_info")
      .then(setBuild)
      .catch(() => {});

    // 启动时顺手看一眼有没有新版本（出错就安静地算了）
    invoke<UpdateInfo>("check_for_update")
      .then((info) => setUpdate(info.has_update ? info : null))
      .catch(() => {});

    return () => {
      unlistenMonitor.then((f) => f());
      unlistenCalibration.then((f) => f());
    };
  }, []);

  /** 打开手动校准：**先试游戏画面蒙版**，开不出来（游戏没开 / 独占全屏 / 建窗失败）就退回截图浮层。 */
  const openCalibration = useCallback(async () => {
    try {
      await invoke("open_calibration_overlay");
    } catch {
      setPage("exp");
      setCalibrating(true);
    }
  }, []);

  const navigate = useCallback((next: PageId, opts?: { keyword?: string }) => {
    if (next === "query" && opts?.keyword) {
      setSeed({ term: opts.keyword, nonce: Date.now() });
    }
    setPage(next);
  }, []);

  const { ref: navRef, style: navStyle } = useSlider<HTMLElement>(page);

  const openExternal = (url: string) => invoke("open_external_url", { url }).catch(() => {});

  return (
    <div className="flex h-screen w-screen bg-[var(--app-bg)] text-slate-100 overflow-hidden font-sans">
      {/* ------------------------------------------------------------ 侧栏 */}
      <aside className="w-[68px] min-[1000px]:w-56 shrink-0 border-r border-white/10 bg-slate-900/60 flex flex-col p-3 min-[1000px]:p-4">
        <div className="flex items-center gap-3 px-1 pb-5 justify-center min-[1000px]:justify-start">
          <div className="w-10 h-10 shrink-0 rounded-xl bg-amber-500/15 flex items-center justify-center">
            {/* 像素画小蘑菇，用 pixelated 保持硬边不发糊 */}
            <img src={mushroomLogo} alt="枫之助" className="w-7 h-7" style={{ imageRendering: "pixelated" }} />
          </div>
          <div className="hidden min-[1000px]:block min-w-0">
            <div className="font-bold text-[15px] tracking-wide leading-tight">枫之助</div>
            {/* 版本号来自正在跑的那个二进制（`get_build_info`），拿不到就什么都不显示 —— 绝不先摆一个写死的版本号 */}
            <div className="text-[10px] text-slate-500 font-mono truncate" title={build ? `构建于 ${build.built_at}\n${build.exe_path}` : undefined}>
              {build ? `v${build.version} · ` : ""}怀旧服小助手
            </div>
          </div>
        </div>

        <nav ref={navRef} className="relative space-y-1" aria-label="主导航">
          <span aria-hidden className="rounded-xl bg-slate-800" style={navStyle} />
          {NAV.map((item) => {
            const on = page === item.id;
            return (
              <button
                key={item.id}
                type="button"
                onClick={() => setPage(item.id)}
                title={item.label}
                aria-current={on ? "page" : undefined}
                data-active={on}
                className={`relative z-10 w-full flex items-center gap-3 rounded-xl px-3 py-2.5 text-[13px] font-medium cursor-pointer justify-center min-[1000px]:justify-start ${
                  on ? "text-amber-400" : "text-slate-400 hover:text-slate-100"
                }`}
              >
                {item.icon}
                <span className="hidden min-[1000px]:inline">{item.label}</span>
              </button>
            );
          })}
        </nav>

        <div className="flex-1" />

        {/* 新版本：只在真有新版本时出现 */}
        {update?.download_url && (
          <button
            type="button"
            onClick={() => openExternal(update.download_url as string)}
            title={
              (update.notes ? `更新说明：${update.notes}\n\n` : "") +
              `当前 ${update.current} → 新版本 ${update.latest}\n\n` +
              "下载后解压覆盖旧的 exe 即可，设置与黑名单都存在系统目录里、不会丢。"
            }
            className="mb-2.5 flex items-center justify-center min-[1000px]:justify-start gap-2 rounded-xl bg-emerald-500/12 text-emerald-300 px-3 py-2 text-xs font-medium hover:bg-emerald-500/20 cursor-pointer"
          >
            <DownloadCloud className="w-4 h-4 shrink-0" />
            <span className="hidden min-[1000px]:inline truncate">有新版本 {update.latest}</span>
          </button>
        )}

        {/* 常驻状态区：频道 / 开服 / 内存 —— 不属于任何页面，所以放在导航之外 */}
        <div className="rounded-2xl border border-white/10 bg-slate-900/70 p-2.5 min-[1000px]:p-3 space-y-2.5 text-xs">
          {/* 侧栏贴着窗口底边：浮层往上开；「上次」胶囊放不下就折到第二行 */}
          <div className="hidden min-[1000px]:flex items-start justify-between gap-2">
            <span className="text-slate-400 whitespace-nowrap leading-[22px]">当前频道</span>
            <ChannelBadge compact align="left" side="top" wrap />
          </div>

          <div
            className="flex items-center justify-center min-[1000px]:justify-between gap-2 cursor-help"
            title={monitor.tooltip}
          >
            <span className="hidden min-[1000px]:inline text-slate-400 whitespace-nowrap">官方开服</span>
            <span className={`flex items-center gap-1.5 font-medium ${TONE_TEXT[monitor.tone] ?? TONE_TEXT.unknown}`}>
              <span className={`w-1.5 h-1.5 rounded-full ${TONE_DOT[monitor.tone] ?? TONE_DOT.unknown}`} />
              <span className="hidden min-[1000px]:inline whitespace-nowrap">{TONE_WORD[monitor.tone] ?? TONE_WORD.unknown}</span>
            </span>
          </div>

          <div
            title={
              memory
                ? `已用 ${memory.used_mb}MB / 共 ${memory.total_mb}MB，警戒线 ${memory.threshold}%`
                : "正在读取内存占用…"
            }
          >
            <div className="hidden min-[1000px]:flex items-center justify-between mb-1.5">
              <span className="text-slate-400">内存</span>
              <span className={`tabular-nums ${memory?.is_warning ? "text-red-400 font-bold" : ""}`}>
                {memory ? `${memory.percent.toFixed(0)}%` : "读取中"}
                {memory && (
                  <span className="text-slate-500"> · {(memory.used_mb / 1024).toFixed(1)} / {(memory.total_mb / 1024).toFixed(0)} GB</span>
                )}
              </span>
            </div>
            <div className="h-1.5 rounded-full bg-slate-800 overflow-hidden">
              <div
                className={`h-full rounded-full transition-[width] duration-700 ease-out ${memory?.is_warning ? "bg-red-400" : "bg-emerald-400"}`}
                style={{ width: `${Math.min(100, memory?.percent ?? 0)}%` }}
              />
            </div>
            {memory?.is_warning && (
              <div className="mt-1.5 hidden min-[1000px]:block text-[10px] text-red-400">内存偏高，建议进商城刷新</div>
            )}
          </div>

          {/* 数据来源：物价、金价、掉落、开服监控都是小册子的数据，每一页都看得到出处 */}
          <button
            type="button"
            onClick={() => openExternal(MXDC_HOME)}
            title="物价、金价、掉落、开服监控的数据都来自小册子 mxdc.dvg.cn（点击用系统浏览器打开）"
            className="hidden min-[1000px]:flex w-full items-center justify-between gap-2 border-t border-white/10 pt-2.5 cursor-pointer group"
          >
            <span className="text-slate-400 whitespace-nowrap">数据来源</span>
            <span className="flex items-center gap-1 font-medium whitespace-nowrap text-amber-400 group-hover:text-amber-300">
              小册子
              <ExternalLink className="w-3 h-3 shrink-0" />
            </span>
          </button>
        </div>

        <div className="mt-2.5 flex flex-col min-[1000px]:flex-row gap-1.5">
          <ThemeToggle className="flex-1 !py-2 justify-center" />
          <button
            type="button"
            onClick={() => openExternal(QQ_GROUP_INVITE)}
            title={`点击加入 QQ 群 ${QQ_GROUP_NUMBER}（更新、问题反馈都发在群里）`}
            aria-label="QQ 群"
            className="flex-1 inline-flex items-center justify-center gap-1.5 rounded-lg border border-white/10 bg-slate-800/80 py-2 text-sky-300 hover:border-sky-400/50 cursor-pointer"
          >
            <MessagesSquare className="w-3.5 h-3.5" />
            <span className="hidden min-[1000px]:inline text-[11px] font-mono">{QQ_GROUP_NUMBER}</span>
          </button>
        </div>
      </aside>

      {/* ------------------------------------------------------------ 内容区 */}
      <div className="flex-1 min-w-0 flex flex-col h-full overflow-hidden">
        <header className="h-16 shrink-0 px-5 xl:px-7 flex items-center justify-between gap-4">
          <h1 className="text-xl font-bold">{TITLES[page]}</h1>
          {/* 预设区服：查价、行情、悬浮窗、黑名单都用它；切了所有窗口一起跟着切。
              预设还没读回来时先不画，免得「先亮蓝蜗牛再跳到你那个服」。 */}
          <div
            className="flex items-center gap-2.5 shrink-0"
            title="查价、行情、悬浮窗、黑名单都用这个区服；切了所有窗口一起跟着切"
          >
            <span className="text-xs text-slate-400 whitespace-nowrap">预设区服</span>
            {!serverLoading && serverTabs.length > 0 && (
              <SubTabs
                tabs={serverTabs}
                value={String(serverId)}
                onChange={(key) => chooseServer(Number(key))}
              />
            )}
          </div>
        </header>

        {/* min-w-0 是让内部长内容能正确收缩、而不是把布局撑破的关键；
            窗口最大化到大屏时用 max-w 封顶，避免内容被拉成超宽一条 */}
        <main className="flex-1 min-w-0 overflow-y-auto px-5 xl:px-7 pb-6">
          <div key={page} className="anim-page mx-auto w-full max-w-[1400px]">
            <Suspense fallback={null}>
              {page === "overview" && <OverviewPage onNavigate={navigate} monitor={monitorStatus} />}
              {page === "meso" && <MesoTrends />}
              {page === "query" && <QueryPage seed={seed} />}
              {page === "exp" && <ExpStats onOpenCalibration={openCalibration} />}
            </Suspense>
            {page === "alert" && <AlertSettings />}
            {page === "settings" && <SettingsPage />}
            {page === "about" && <AboutPage build={build} />}
          </div>
        </main>
      </div>

      {/* 换版本时弹一次「这一版更新了什么」（判断在后端，见 take_version_greeting） */}
      <UpdateNotesGreeting />

      {/* 校准浮层（自动识别 + 手动框选）。入口：练级页的按钮、失败横幅、经验窗的「校准位置」。 */}
      {calibrating && <CalibrationDialog onClose={() => setCalibrating(false)} />}
    </div>
  );
};
