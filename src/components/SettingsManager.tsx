import React, { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { COOLDOWN, guard } from "../lib/throttle";
import { listen } from "@tauri-apps/api/event";
import {
  Key,
  ExternalLink,
  Sparkles,
  Monitor,
  Wand2,
  DownloadCloud,
  RefreshCw,
} from "lucide-react";
import { InlineNotice, type Notice } from "./Notice";
import { MXDC_HOME } from "../lib/links";
import { useMotion } from "../lib/motion";
import { Switch } from "./ui/kit";
import type { AppSettings, BuildInfo, UpdateInfo } from "../types";

/**
 * 基础配置：**配一次就不用再动的那几项**（关闭行为、小册子登录凭证）。
 * 预设区服不在这里 —— 它在主窗口右上角，每一页都能切。
 *
 * 以前的偏好设置页把三组东西挤在一屏里靠切换卡片选（悬浮窗与内存 / 开服提醒 /
 * 基础配置）。那两组已经各自搬成一个左侧导航页了，所以这里只剩基础配置，
 * 而它也**不再需要「保存全部配置」按钮**：关闭行为点了就写库，
 * 登录凭证有自己的保存按钮。「全部」这个词在这一页本来也没意义 —— 它只看得见
 * 自己这一页的字段。
 *
 * 这里有一张「版本与更新」卡片，但它**没有输入框**：更新页地址编译在程序里
 * （`update::DEFAULT_UPDATE_URL`，一个夸克网盘分享页），程序启动时自己看一眼，
 * 真有新版本才在顶栏冒一个小胶囊。卡片只是把同一个地址**打开**出来 ——
 * 用户想自己去看一眼有没有新版，总得有个去处，不用去问人要链接；
 * 「检查更新」按钮和启动时那次检查走的是同一个命令，同一个地址。
 */
export const SettingsManager: React.FC = () => {
  const [settings, setSettings] = useState<AppSettings | null>(null);
  /** 当前在跑的二进制版本（不写死，免得界面显示的和文件属性里的不一样） */
  const [build, setBuild] = useState<BuildInfo | null>(null);
  const [notice, setNotice] = useState<Notice | null>(null);
  /** 界面动效开关（存在本机，三个窗口一起生效；见 lib/motion.ts） */
  const [motion, setMotion] = useMotion();
  /** 手动粘贴的 Cookie 草稿：只有点「保存凭证」才写库，避免边打字边写 */
  const [cookieDraft, setCookieDraft] = useState("");
  /** 内置更新页地址（来自 Rust 侧常量，界面不自己写死） */
  const [updatePageUrl, setUpdatePageUrl] = useState("");
  const [checking, setChecking] = useState(false);
  /** 上次「检查更新」的结果；null = 还没查过 */
  const [updateResult, setUpdateResult] = useState<UpdateInfo | null>(null);

  useEffect(() => {
    invoke<AppSettings>("get_settings")
      .then((data) => {
        setSettings(data);
        setCookieDraft(data.auth_cookie);
      })
      .catch((err: unknown) => setNotice({ kind: "error", message: `读取设置失败：${err}` }));

    invoke<string>("get_update_page_url")
      .then(setUpdatePageUrl)
      .catch(() => {});

    invoke<BuildInfo>("get_build_info")
      .then(setBuild)
      .catch(() => {});

    // 进这一页就顺手看一眼有没有新版本（和启动时那次同一个命令）。
    // 出错不弹提示：这里没查到不代表有问题，点按钮重试即可。
    // 版本自检要打站点，而且结果本来就不会秒变
    if (!guard("check_for_update", "版本自检", COOLDOWN.heavy)) return;
    invoke<UpdateInfo>("check_for_update")
      .then(setUpdateResult)
      .catch(() => {});

    // 扫码登录成功后 Cookie 已经由 Rust 侧写进数据库，这里同步一下内存状态，
    // 否则紧接着点「保存凭证」会把刚拿到的 Cookie 覆盖回旧值。
    const unlistenAuth = listen<string>("login-cookie-updated", (event) => {
      setSettings((prev) => (prev ? { ...prev, auth_cookie: event.payload } : prev));
      setCookieDraft(event.payload);
    });

    return () => {
      unlistenAuth.then((f) => f());
    };
  }, []);

  /** 关窗行为：**立刻生效**（patch_settings 只改这一个字段），不用等保存。 */
  const handleCloseToTrayChange = async (value: boolean) => {
    setSettings((prev) => (prev ? { ...prev, close_to_tray: value } : prev));
    try {
      await invoke("patch_settings", { patch: { close_to_tray: value } });
    } catch (err: unknown) {
      setNotice({ kind: "error", message: `保存关闭行为失败：${err}` });
    }
  };

  /** 手动再检查一次（和启动时那次同一个命令、同一个地址）。 */
  const handleCheckUpdate = async () => {
    setChecking(true);
    try {
      if (!guard("check_for_update", "版本自检", COOLDOWN.heavy)) return;
      setUpdateResult(await invoke<UpdateInfo>("check_for_update"));
    } catch (err: unknown) {
      setNotice({ kind: "error", message: `检查更新失败：${err}` });
    } finally {
      setChecking(false);
    }
  };

  /** 更新页一律交给系统浏览器打开（webview 里 `target="_blank"` 是死链）。 */
  const openUrl = (url: string) => {
    if (url) invoke("open_external_url", { url }).catch(() => {});
  };

  const handleSaveCookie = async () => {
    try {
      await invoke("save_auth_cookie", { cookie: cookieDraft.trim() });
      setSettings((prev) => (prev ? { ...prev, auth_cookie: cookieDraft.trim() } : prev));
      setNotice({ kind: "success", message: "凭证已保存。想确认它到底能不能用，去查一次价。" });
    } catch (err: unknown) {
      setNotice({ kind: "error", message: `保存 Cookie 失败：${err}` });
    }
  };

  if (!settings) {
    return (
      <div className="space-y-5 max-w-4xl">
        <div className="p-5 rounded-xl bg-slate-900/40 border border-white/5 text-xs text-slate-500">
          正在读取当前设置…
        </div>
      </div>
    );
  }

  return (
    <div className="space-y-5 max-w-4xl">
      <p className="text-xs text-slate-400 px-1">配一次就不用再动的几项</p>

      <InlineNotice notice={notice} onClose={() => setNotice(null)} />

      {/* 关闭行为与系统托盘 */}
      <div className="p-5 rounded-2xl bg-slate-900/70 border border-white/10 space-y-4">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2 text-sm font-bold text-slate-200">
            <Monitor className="w-4 h-4 text-sky-400" />
            关闭窗口与系统托盘
          </div>
          <label className="relative inline-flex items-center cursor-pointer">
            <input
              type="checkbox"
              checked={settings.close_to_tray}
              onChange={(e) => handleCloseToTrayChange(e.target.checked)}
              className="sr-only peer"
            />
            <div className="w-9 h-5 bg-slate-700 peer-focus:outline-none rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-4 after:w-4 after:transition-all peer-checked:bg-sky-500"></div>
          </label>
        </div>

        <p className="text-xs text-slate-400 leading-relaxed">
          {settings.close_to_tray
            ? "已开启（默认）：点 X 只把窗口收进托盘，开服提醒、剪贴板嗅探、快捷键都继续跑；要真正退出用托盘菜单的「退出程序」。"
            : "已关闭：点 X 直接结束进程，后台的开服提醒、剪贴板嗅探、快捷键也会一起停。"}
        </p>

        <div className="text-[11px] text-slate-500 leading-relaxed bg-slate-950/40 border border-white/5 rounded-lg p-3">
          托盘图标（任务栏右下角）：左键单击抬回主窗口，右键是菜单。
        </div>
      </div>

      {/* 界面动效 */}
      <div className="p-5 rounded-2xl bg-slate-900/70 border border-white/10 space-y-4">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2 text-sm font-bold text-slate-200">
            <Wand2 className="w-4 h-4 text-amber-400" />
            界面动效
          </div>
          <Switch checked={motion} onChange={setMotion} label="界面动效" />
        </div>

        <p className="text-xs text-slate-400 leading-relaxed">
          {motion
            ? "已开启（默认）：切页过渡、数字滚动、小结卡片的翻面和镭射都会播 —— 即使 Windows 里关了「动画效果」也照播。"
            : "已关闭：界面不再播放过渡和动画（小结卡片仍然会跟着鼠标倾斜）。"}
        </p>
      </div>

      {/* 小册子登录凭证 */}
      <div className="p-5 rounded-2xl bg-slate-900/70 border border-white/10 space-y-4">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2 text-sm font-bold text-slate-200">
            <Key className="w-4 h-4 text-amber-400" />
            小册子 (mxdc.dvg.cn) 登录凭证
          </div>
          {settings.auth_cookie ? (
            <span className="text-[10px] px-2 py-0.5 rounded-full bg-emerald-500/15 text-emerald-400 border border-emerald-500/30">
              已注入 Cookie
            </span>
          ) : (
            <span className="text-[10px] px-2 py-0.5 rounded-full bg-amber-500/15 text-amber-400 border border-amber-500/30">
              未授权 (部分行情受限)
            </span>
          )}
        </div>

        <p className="text-xs text-slate-400 leading-relaxed">
          完整物价需要小册子账号。推荐微信扫码，也可以手动粘贴 Cookie。
        </p>

        <div className="flex items-center gap-3 pt-1">
          <button
            type="button"
            onClick={() => invoke("open_login_window")}
            className="flex-1 py-2 rounded-lg bg-amber-500/20 hover:bg-amber-500/30 text-amber-300 border border-amber-500/40 text-xs font-bold flex items-center justify-center gap-1.5 transition-all cursor-pointer"
          >
            <ExternalLink className="w-3.5 h-3.5" />
            打开账号授权助手 (微信扫码/提取Cookie)
          </button>
        </div>

        <div className="space-y-1.5">
          <label className="block text-xs text-slate-400">手动粘贴 Cookie (可选备用)</label>
          <div className="flex flex-col sm:flex-row gap-2">
            <input
              type="password"
              value={cookieDraft}
              onChange={(e) => setCookieDraft(e.target.value)}
              placeholder="PHPSESSID=... 或 auth_token=..."
              className="flex-1 bg-slate-800 border border-white/10 rounded-lg px-3 py-1.5 text-xs text-slate-100 tabular-nums focus:outline-none focus:border-amber-400"
            />
            <button
              type="button"
              onClick={handleSaveCookie}
              className="px-4 py-1.5 rounded-lg bg-slate-800 hover:bg-slate-700 text-xs font-semibold text-slate-200 border border-white/10 cursor-pointer shrink-0"
            >
              保存凭证
            </button>
          </div>
          <p className="text-[11px] text-slate-500">
            扫码会自动验证有效性；手动粘贴的只保存、不验证。
          </p>
        </div>
      </div>

      {/* 版本与更新：地址编译在程序里，这里只显示 + 打开，不能改 */}
      <div className="p-5 rounded-2xl bg-slate-900/70 border border-white/10 space-y-3">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2 text-sm font-bold text-slate-200">
            <DownloadCloud className="w-4 h-4 text-sky-400" />
            版本与更新
          </div>
          <span className="text-[10px] px-2 py-0.5 rounded-full bg-slate-800 text-slate-400 border border-white/10 tabular-nums">
            {build ? `当前 v${build.version}` : "当前版本读取中…"}
          </span>
        </div>

        <div className="flex flex-wrap items-center gap-2">
          <button
            type="button"
            onClick={handleCheckUpdate}
            disabled={checking}
            className="px-3 py-1.5 rounded-lg bg-slate-800 hover:bg-slate-700 disabled:opacity-50 text-xs font-semibold text-slate-200 border border-white/10 cursor-pointer flex items-center gap-1.5"
          >
            <RefreshCw className={`w-3.5 h-3.5 ${checking ? "animate-spin" : ""}`} />
            {checking ? "正在检查…" : "检查更新"}
          </button>

          {updatePageUrl && (
            <button
              type="button"
              onClick={() => openUrl(updatePageUrl)}
              title={updatePageUrl}
              className="px-3 py-1.5 rounded-lg bg-amber-500/10 hover:bg-amber-500/20 text-xs font-semibold text-amber-300 border border-amber-500/30 cursor-pointer flex items-center gap-1.5"
            >
              打开更新页（网盘）
              <ExternalLink className="w-3 h-3" />
            </button>
          )}
        </div>

        {/* 把地址本身也摆出来（可选中复制）：想手抄、想在手机上打开时用得上 */}
        {updatePageUrl && (
          <p className="text-[10px] tabular-nums text-slate-500 break-all select-all">
            {updatePageUrl}
          </p>
        )}

        {/* 三种结果分开说：有新版本 / 已是最新 / 没能确认（后者绝不能说成前者） */}
        {updateResult && (
          <p
            className={`text-xs ${
              updateResult.has_update
                ? "text-emerald-300"
                : updateResult.error
                  ? "text-amber-300"
                  : "text-slate-400"
            }`}
          >
            {updateResult.has_update
              ? `有新版本 ${updateResult.latest}（当前 ${updateResult.current}）`
              : updateResult.error
                ? `没能确认有没有新版本：${updateResult.error}`
                : `已是最新（${updateResult.current}）`}
          </p>
        )}

        {updateResult?.notes && (
          <p className="text-[11px] text-slate-500">更新说明：{updateResult.notes}</p>
        )}

        {updateResult?.has_update && (
          <button
            type="button"
            onClick={() => openUrl(updateResult.download_url ?? updatePageUrl)}
            className="w-full py-2 rounded-lg bg-emerald-500/20 hover:bg-emerald-500/30 text-emerald-300 border border-emerald-500/40 text-xs font-bold flex items-center justify-center gap-1.5 cursor-pointer"
          >
            <DownloadCloud className="w-3.5 h-3.5" />
            去下载 {updateResult.latest}
          </button>
        )}
      </div>

      {/* Source Attribution Card */}
      <div className="p-4 rounded-xl bg-slate-900/40 border border-white/5 flex items-center justify-between text-xs text-slate-400">
        <div className="flex items-center gap-2">
          <Sparkles className="w-4 h-4 text-amber-400" />
          <span>
            本项目所有怀旧服装备图鉴、物价与走势数据均来源于{" "}
            <strong className="text-slate-200">冒险岛怀旧服小册子 (mxdc.dvg.cn)</strong>
          </span>
        </div>
        <button
          type="button"
          onClick={() => invoke("open_external_url", { url: MXDC_HOME }).catch(() => {})}
          title="用系统浏览器打开小册子"
          className="text-amber-400 hover:underline flex items-center gap-1 cursor-pointer"
        >
          访问源站
          <ExternalLink className="w-3 h-3" />
        </button>
      </div>
    </div>
  );
};
