import React, { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { MXDC_HOME } from "../lib/links";
import { 
  Key, 
  ExternalLink, 
  CheckCircle2, 
  AlertCircle, 
  X, 
  Clipboard, 
  ShieldCheck,
  Globe,
  QrCode
} from "lucide-react";

export const LoginModalView: React.FC = () => {
  const [cookie, setCookie] = useState("");
  const [testing, setTesting] = useState(false);
  /**
   * `warn` = 「存下来了，但没能确认它到底能不能用」。
   *
   * 必须有这一档：后端以前只有「有效 / 无效」两种答案，而只有 401 算无效，
   * 于是网络不通、站点 5xx 全被当成**验证成功**，界面写着「已注入并生效」——
   * 用户接着就会在「验证通过了却查不到东西」里绕圈。
   */
  const [status, setStatus] = useState<"idle" | "success" | "warn" | "fail">("idle");
  const [statusMsg, setStatusMsg] = useState("");
  const [qrWaiting, setQrWaiting] = useState(false);

  useEffect(() => {
    // Load existing cookie if any
    invoke<{ auth_cookie: string }>("get_settings")
      .then((cfg) => {
        if (cfg.auth_cookie) {
          setCookie(cfg.auth_cookie);
        }
      })
      .catch(() => {});

    // 内嵌扫码窗口登录成功（Rust 侧已验活并落库）→ 自动填充并关闭本窗口
    const unlistenUpdated = listen<string>("login-cookie-updated", (event) => {
      setQrWaiting(false);
      setCookie(event.payload);
      setStatus("success");
      setStatusMsg("微信扫码登录成功！凭证已自动保存，窗口即将关闭...");
      setTimeout(() => {
        invoke("close_login_window");
      }, 1200);
    });

    // 自动获取超时：扫码窗口不会自己关，用户可以继续扫，也可以改用手动粘贴
    const unlistenTimeout = listen("login-cookie-timeout", () => {
      setQrWaiting(false);
      setStatus("fail");
      setStatusMsg(
        "自动获取超时：请确认已在扫码窗口中完成登录；若站点限制了内置浏览器，请改用下方手动粘贴 Cookie。"
      );
    });

    // Esc key listener to close
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        invoke("close_login_window");
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => {
      window.removeEventListener("keydown", handleKeyDown);
      unlistenUpdated.then((f) => f());
      unlistenTimeout.then((f) => f());
    };
  }, []);

  const handleQrLogin = async () => {
    setStatusMsg("");
    try {
      await invoke("open_qr_login_window");
      setQrWaiting(true);
    } catch (err: unknown) {
      setStatus("fail");
      setStatusMsg(typeof err === "string" ? err : "打开扫码窗口失败");
    }
  };

  const handleOpenBrowser = () => {
    invoke("open_external_url", { url: MXDC_HOME });
  };

  const handlePasteClipboard = async () => {
    try {
      const text = await navigator.clipboard.readText();
      if (text) {
        setCookie(text.trim());
      }
    } catch {
      // ignore
    }
  };

  const handleVerifyAndSave = async () => {
    if (!cookie.trim()) {
      setStatus("fail");
      setStatusMsg("请输入或粘贴有效的 Cookie");
      return;
    }

    setTesting(true);
    setStatus("idle");
    setStatusMsg("");

    try {
      const verdict = await invoke<"valid" | "invalid" | "unverified">(
        "test_cookie_validity",
        { cookie: cookie.trim() }
      );

      if (verdict === "valid") {
        setStatus("success");
        setStatusMsg("验证成功！Cookie 已注入并生效，窗口即将自动关闭...");
        setTimeout(() => {
          invoke("close_login_window");
        }, 1200);
      } else if (verdict === "unverified") {
        // 这条消息不能写成「失败」，也不能写成「成功」：我们只是没问到
        setStatus("warn");
        setStatusMsg(
          "没能连上小册子确认这份 Cookie（网络或站点问题），它**已经保存**了。\n稍后查价时若提示需要登录，回来重新绑定即可。"
        );
      } else {
        setStatus("fail");
        setStatusMsg("Cookie 验证未通过（未登录或已过期），请在浏览器重新登录后提取！");
      }
    } catch (err: unknown) {
      setStatus("fail");
      setStatusMsg(typeof err === "string" ? err : "验证请求出错");
    } finally {
      setTesting(false);
    }
  };

  const handleClose = () => {
    invoke("close_login_window");
  };

  return (
    <div className="h-screen w-screen bg-[var(--app-bg)] text-slate-100 flex flex-col justify-between p-4 select-none font-sans">
      {/* Title / Drag Bar */}
      <div 
        data-tauri-drag-region 
        className="flex items-center justify-between pb-3 border-b border-white/10 cursor-move"
      >
        <div className="flex items-center gap-2 text-sm font-bold text-slate-100">
          <Key className="w-4 h-4 text-amber-400" />
          <span>小册子账号授权绑定</span>
        </div>
        <button
          onClick={handleClose}
          className="p-1 rounded-lg hover:bg-white/10 text-slate-400 hover:text-white transition-colors cursor-pointer"
          title="关闭窗口 (Esc)"
        >
          <X className="w-4 h-4" />
        </button>
      </div>

      {/* Main Body Steps */}
      <div className="flex-1 py-4 space-y-4 text-xs overflow-y-auto">

        {/* Primary: 内嵌扫码 + 自动提取 Cookie */}
        <div className="p-3.5 rounded-xl bg-amber-500/10 border border-amber-500/30 space-y-2.5">
          <div className="flex items-center justify-between">
            <span className="font-bold text-amber-300 flex items-center gap-1.5">
              <QrCode className="w-3.5 h-3.5" />
              推荐：微信扫码登录（自动获取 Cookie）
            </span>
            {qrWaiting && (
              <span className="text-[10px] text-amber-300/80 animate-pulse">
                等待扫码结果...
              </span>
            )}
          </div>

          <p className="text-slate-300/90 leading-relaxed text-[11px]">
            在本工具内打开 <strong className="text-slate-100">mxdc.dvg.cn</strong> 微信扫码：
            Cookie 自动读取、验证并保存，成功后窗口自动关闭。
          </p>

          <button
            onClick={handleQrLogin}
            disabled={qrWaiting}
            className="w-full py-2 rounded-lg bg-amber-500 hover:bg-amber-400 active:bg-amber-600 text-slate-950 font-bold text-xs flex items-center justify-center gap-1.5 transition-all cursor-pointer shadow disabled:opacity-60"
          >
            <QrCode className="w-3.5 h-3.5" />
            <span>{qrWaiting ? "已打开扫码窗口，请扫码..." : "打开微信扫码登录"}</span>
          </button>
        </div>

        <div className="text-[11px] text-slate-500 pt-1">
          —— 以下为备用方案（内置浏览器被站点风控拦住时使用）——
        </div>

        {/* Step 1 */}
        <div className="p-3.5 rounded-xl bg-slate-900/80 border border-white/10 space-y-2">
          <div className="flex items-center justify-between">
            <span className="font-bold text-slate-200 flex items-center gap-1.5">
              <Globe className="w-3.5 h-3.5 text-amber-400" />
              备用 1：在浏览器登录小册子
            </span>
            <button
              onClick={handleOpenBrowser}
              className="px-3 py-1 rounded-lg bg-amber-500 hover:bg-amber-400 text-slate-950 font-bold text-xs flex items-center gap-1 transition-all cursor-pointer shadow"
            >
              <span>在浏览器打开</span>
              <ExternalLink className="w-3 h-3" />
            </button>
          </div>
          <p className="text-slate-400 leading-relaxed text-[11px]">
            用默认浏览器打开 <strong className="text-slate-200">mxdc.dvg.cn</strong> 登录（不受代理影响）。
          </p>
        </div>

        {/* Step 2 */}
        <div className="p-3.5 rounded-xl bg-slate-900/80 border border-white/10 space-y-2.5">
          <div className="flex items-center justify-between">
            <span className="font-bold text-slate-200 flex items-center gap-1.5">
              <ShieldCheck className="w-3.5 h-3.5 text-emerald-400" />
              备用 2：粘贴登录 Cookie
            </span>
            <button
              onClick={handlePasteClipboard}
              className="px-2.5 py-1 rounded bg-slate-800 hover:bg-slate-700 text-slate-300 border border-white/10 text-[11px] flex items-center gap-1 transition-colors cursor-pointer"
            >
              <Clipboard className="w-3 h-3" />
              <span>从剪贴板粘贴</span>
            </button>
          </div>

          <textarea
            rows={3}
            value={cookie}
            onChange={(e) => setCookie(e.target.value)}
            placeholder="粘贴小册子的 Cookie 字符串（例如 PHPSESSID=... 或全量 Cookie）"
            className="w-full bg-slate-800 border border-white/10 rounded-lg p-2.5 text-slate-100 font-mono text-[11px] focus:outline-none focus:border-amber-400/80 leading-relaxed"
          />

          <div className="text-[11px] text-slate-500">
            💡 提示：在浏览器按 <kbd className="px-1 py-0.5 rounded bg-slate-800 text-slate-300 font-mono">F12</kbd> ➔【应用程序 (Application)】➔【Cookie】中复制值。
          </div>
        </div>

        {/* Status Prompt */}
        {statusMsg && (
          <div className={`p-3 rounded-xl border text-xs flex items-start gap-2 ${
            status === "success"
              ? "bg-emerald-950/50 border-emerald-500/40 text-emerald-300"
              : status === "warn"
                ? "bg-amber-950/50 border-amber-500/40 text-amber-200"
                : "bg-red-950/50 border-red-500/40 text-red-300"
          }`}>
            {status === "success" ? (
              <CheckCircle2 className="w-4 h-4 text-emerald-400 shrink-0 mt-0.5" />
            ) : (
              <AlertCircle
                className={`w-4 h-4 shrink-0 mt-0.5 ${
                  status === "warn" ? "text-amber-400" : "text-red-400"
                }`}
              />
            )}
            <span className="whitespace-pre-wrap">{statusMsg}</span>
          </div>
        )}

      </div>

      {/* Footer Actions */}
      <div className="pt-3 border-t border-white/10 flex items-center justify-end gap-3">
        <button
          type="button"
          onClick={handleClose}
          className="px-4 py-1.5 rounded-lg bg-slate-800 hover:bg-slate-700 text-slate-300 text-xs font-semibold cursor-pointer"
        >
          关闭窗口
        </button>
        <button
          type="button"
          onClick={handleVerifyAndSave}
          disabled={testing}
          className="px-5 py-1.5 rounded-lg bg-amber-500 hover:bg-amber-400 active:bg-amber-600 text-slate-950 text-xs font-bold transition-all shadow-md shadow-amber-500/10 cursor-pointer disabled:opacity-50"
        >
          {testing ? "验证有效性中..." : "验证并保存"}
        </button>
      </div>
    </div>
  );
};
