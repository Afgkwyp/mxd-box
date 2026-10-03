import React, { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Loader2, Target, X } from "lucide-react";
import type { CalibrationGeometry, CalibrationTest, NormRect, RegionProfile } from "../types";

/**
 * **校准蒙版窗**（`?window=calibration-overlay`）。
 *
 * 这是一层透明置顶窗，由 `open_calibration_overlay` 命令**按游戏客户区的
 * 物理像素**摆好位置和大小，正好盖在游戏上。用户看到的是：
 *
 * ```text
 * ┌───────────────────────────────┐
 * │  提示（放大）· 测试读数 / 保存 / 取消 │  ← 顶部工具条（不挡经验条）
 * │                               │
 * │        （游戏画面，正常可见）        │
 * │   ┌──────────────┐             │
 * │   │ 拖出来的框     │ ← 框外压暗     │
 * │   └──────────────┘             │
 * │                               │
 * └───────────────────────────────┘
 * ```
 *
 * 按钮**全部在顶部**：经验条就在画面最底下，底部工具条会正好压住它。
 *
 * ## 坐标
 *
 * 蒙版窗的**物理**尺寸 = 客户区尺寸；WebView 里的 CSS 像素可能被 Windows 缩放
 * 除过一次。所以换算只有一个式子：
 *
 * ```text
 * 客户区物理像素 = (事件 CSS 坐标 − 根节点左上角 CSS 坐标) × 客户区物理宽 ÷ 根节点渲染宽
 * ```
 *
 * 这个比值把系统缩放、窗口缩放全部吸收掉，**不显式乘 devicePixelRatio**
 * （本项目在 DPI 上栽过「整体偏移」）。
 *
 * ## 独占全屏怎么办
 *
 * 透明置顶窗在独占全屏下可能根本显示不出来。这一版没有在蒙版里留按钮：
 * 主窗口 / 悬浮窗点「手动校准」时会**先试蒙版**，命令失败（拿不到窗口等）
 * 自动退回截图浮层（`CalibrationDialog`）；Esc 也随时能关掉蒙版。
 */

const KIND_EXP_LINE = "exp_line";
const KIND_MAP_NAME = "map_name";
const KIND_LEVEL = "level";

/** 可校准的三样东西：经验条 / 地图名 / 等级（各自一条比例记录）。 */
const KIND_OPTIONS: Array<{ kind: string; label: string; hint: string }> = [
  {
    kind: KIND_EXP_LINE,
    label: "经验条",
    hint: "拖框套住「EXP xx(xx.xx%)」那一行",
  },
  {
    kind: KIND_MAP_NAME,
    label: "地图名",
    hint: "拖框套住小地图上的地图名那一行",
  },
  {
    kind: KIND_LEVEL,
    label: "等级",
    hint: "拖框套住 HUD 上的等级数字（如 Lv.55）",
  },
];
/** 框的最小边（客户区物理像素）。 */
const MIN_RECT_PX = 8;

interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

const clamp0 = (value: number, max: number) => Math.min(Math.max(value, 0), Math.max(0, max));

/** 把两个点归成一个正矩形，钳在客户区里并保证最小边。 */
function normalizeRect(
  anchor: { x: number; y: number },
  cursor: { x: number; y: number },
  clientW: number,
  clientH: number,
): Rect {
  const x = clamp0(Math.min(anchor.x, cursor.x), clientW - MIN_RECT_PX);
  const y = clamp0(Math.min(anchor.y, cursor.y), clientH - MIN_RECT_PX);
  const w = clamp0(cursor.x - x, clientW - x);
  const h = clamp0(cursor.y - y, clientH - y);
  return {
    x,
    y,
    w: Math.max(w, Math.min(MIN_RECT_PX, clientW - x)),
    h: Math.max(h, Math.min(MIN_RECT_PX, clientH - y)),
  };
}

function toNorm(rect: Rect, geometry: CalibrationGeometry): NormRect {
  return {
    x: rect.x / geometry.client_w,
    y: rect.y / geometry.client_h,
    w: rect.w / geometry.client_w,
    h: rect.h / geometry.client_h,
  };
}

function fromNorm(norm: NormRect, geometry: CalibrationGeometry): Rect {
  return {
    x: norm.x * geometry.client_w,
    y: norm.y * geometry.client_h,
    w: norm.w * geometry.client_w,
    h: norm.h * geometry.client_h,
  };
}

export const CalibrationOverlayView: React.FC = () => {
  const [geometry, setGeometry] = useState<CalibrationGeometry | null>(null);
  const [kind, setKind] = useState<string>(KIND_EXP_LINE);
  const [rect, setRect] = useState<Rect | null>(null);
  const [busy, setBusy] = useState(false);
  const [test, setTest] = useState<CalibrationTest | null>(null);
  const [message, setMessage] = useState<string | null>(null);

  const rootRef = useRef<HTMLDivElement>(null);
  const rectRef = useRef<Rect | null>(null);
  const anchorRef = useRef<{ x: number; y: number } | null>(null);
  const draggingRef = useRef(false);
  /** 自动测试/保存用的当前 kind（拖拽结束的回调里要读最新值） */
  const kindRef = useRef(kind);
  useEffect(() => {
    kindRef.current = kind;
  }, [kind]);

  const applyRect = useCallback((next: Rect | null) => {
    rectRef.current = next;
    setRect(next);
  }, []);

  // 几何 + 已有校准档：先问后端要一次（蒙版窗和游戏窗口是同一次快照）
  useEffect(() => {
    let alive = true;
    invoke<CalibrationGeometry>("calibration_overlay_geometry")
      .then((data) => {
        if (!alive) return;
        setGeometry(data);
        if (data.profile) applyRect(fromNorm(data.profile.rect, data));
      })
      .catch((err: unknown) => {
        if (alive) setMessage(typeof err === "string" ? err : `拿不到游戏窗口信息：${String(err)}`);
      });
    return () => {
      alive = false;
    };
  }, [applyRect]);

  const close = useCallback((restoreMain: boolean) => {
    void invoke("close_calibration_overlay", { restoreMain }).catch(() => {});
  }, []);

  const cancel = useCallback(() => close(true), [close]);

  // Esc = 取消（关掉蒙版并把主窗口抬回来）
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") cancel();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [cancel]);

  /** 坐标换算唯一入口（式子见文件头）。 */
  const toClient = (event: { clientX: number; clientY: number }) => {
    const el = rootRef.current;
    if (!el || !geometry) return null;
    const box = el.getBoundingClientRect();
    if (box.width <= 0 || box.height <= 0) return null;
    return {
      x: ((event.clientX - box.left) / box.width) * geometry.client_w,
      y: ((event.clientY - box.top) / box.height) * geometry.client_h,
    };
  };

  /** 切换校准类型：把这个 kind 已存的框画出来（没存过就清空重拖）。 */
  const switchKind = async (next: string) => {
    if (busy || next === kind) return;
    setKind(next);
    setTest(null);
    setMessage(null);
    if (!geometry) return;
    try {
      const profile = await invoke<RegionProfile | null>("get_region_profile", { kind: next });
      applyRect(profile ? fromNorm(profile.rect, geometry) : null);
    } catch {
      applyRect(null);
    }
  };

  const onPointerDown = (event: React.PointerEvent) => {
    if (event.button !== 0 || !geometry || busy) return;
    event.preventDefault();
    rootRef.current?.setPointerCapture(event.pointerId);
    const point = toClient(event);
    if (!point) return;
    draggingRef.current = true;
    anchorRef.current = point;
    setTest(null);
    setMessage(null);
    applyRect({ x: point.x, y: point.y, w: 0, h: 0 });
  };

  const onPointerMove = (event: React.PointerEvent) => {
    const anchor = anchorRef.current;
    if (!draggingRef.current || !anchor || !geometry) return;
    const point = toClient(event);
    if (!point) return;
    applyRect(normalizeRect(anchor, point, geometry.client_w, geometry.client_h));
  };

  const onPointerUp = (event: React.PointerEvent) => {
    if (!draggingRef.current) return;
    draggingRef.current = false;
    anchorRef.current = null;
    rootRef.current?.releasePointerCapture?.(event.pointerId);
    const current = rectRef.current;
    if (!current || current.w < MIN_RECT_PX || current.h < MIN_RECT_PX) {
      applyRect(null);
      return;
    }
    // 拖完**立刻自动测试**：过了就自动保存（蒙版不关，方便接着校准别的类型）；
    // 没过就提示重新拖。
    void autoVerify(current);
  };

  /**
   * 拖完自动测试：成功 → 落库并**留在蒙版里**（用户可能还要接着校准地图名 / 等级）；
   * 失败 → 留在蒙版上提示重新拖框。
   *
   * 失败时框**留着**（不清掉）：用户可以直接在原框上再拖一次覆盖，
   * 也可以点顶栏的「重试读数」再试一次（比如那一瞬画面正在加载）。
   */
  const autoVerify = async (current: Rect) => {
    if (!geometry) return;
    setBusy(true);
    setMessage(null);
    setTest(null);
    const kind = kindRef.current;
    try {
      const result = await invoke<CalibrationTest>("test_exp_region", {
        kind,
        rect: toNorm(current, geometry),
      });
      setTest(result);
      if (!result.ok) {
        setMessage("没认出来，请重新拖动框体");
        setBusy(false);
        return;
      }
      await invoke<RegionProfile>("save_region_profile", {
        kind,
        rect: toNorm(current, geometry),
        textHeight: result.text_height ?? null,
      });
      // 不自动关闭：一次打开可以连续校准多个类型（Esc / 取消 才退出）。
      setMessage("已保存。可以继续切换类型校准，或按 Esc 关闭");
      setBusy(false);
    } catch (err: unknown) {
      setMessage(
        typeof err === "string" ? `保存失败：${err}，请重新拖动框体` : "保存失败，请重新拖动框体",
      );
      setBusy(false);
    }
  };

  /** 手动重试当前框（自动测试没过、但想再试一次时用）。 */
  const runTest = async () => {
    const current = rectRef.current;
    if (busy || !current || !geometry) return;
    setBusy(true);
    setMessage(null);
    try {
      const result = await invoke<CalibrationTest>("test_exp_region", {
        kind,
        rect: toNorm(current, geometry),
      });
      setTest(result);
      if (!result.ok) setMessage("没认出来，请重新拖动框体");
    } catch (err: unknown) {
      setTest({
        ok: false,
        message: typeof err === "string" ? err : `测试读数失败：${String(err)}`,
        raw: null,
        exp: null,
        percent: null,
        level: null,
        text_height: null,
        profile: null,
      });
    } finally {
      setBusy(false);
    }
  };

  const save = async () => {
    const current = rectRef.current;
    if (busy || !current || !geometry) return;
    setBusy(true);
    try {
      await invoke<RegionProfile>("save_region_profile", {
        kind,
        rect: toNorm(current, geometry),
        textHeight: test?.text_height ?? null,
      });
      close(true);
    } catch (err: unknown) {
      setMessage(typeof err === "string" ? err : `保存校准失败：${String(err)}`);
      setBusy(false);
    }
  };

  const rectStyle = rect
    ? {
        left: `${(rect.x / (geometry?.client_w ?? 1)) * 100}%`,
        top: `${(rect.y / (geometry?.client_h ?? 1)) * 100}%`,
        width: `${(rect.w / (geometry?.client_w ?? 1)) * 100}%`,
        height: `${(rect.h / (geometry?.client_h ?? 1)) * 100}%`,
      }
    : undefined;

  return (
    <div
      ref={rootRef}
      className="fixed inset-0 select-none touch-none cursor-crosshair overflow-hidden"
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
    >
      {/* 没框时整屏压暗；有框时由框的 boxShadow 压暗框外 */}
      {!rect && <div className="absolute inset-0 bg-slate-950/45 pointer-events-none" />}

      {rect && (
        <div
          className="absolute border-2 border-amber-400 pointer-events-none"
          style={{ ...rectStyle, boxShadow: "0 0 0 9999px rgba(2, 6, 23, 0.55)" }}
        />
      )}

      {/* 顶部工具条：放大后的提示 + 测试 / 保存 / 取消。
          工具条上按下不能当成「开始拖框」，所以拦掉 pointer 事件
          （否则点按钮的瞬间会在按钮位置起一个框）。
          按钮全部放在**顶部**：底部那一层会正好压住经验条（它就在画面最下面）。 */}
      <div
        className="absolute top-3 left-1/2 -translate-x-1/2 w-[min(94vw,880px)] px-4 py-3 rounded-xl bg-slate-950/88 border border-white/15 shadow-xl space-y-2"
        onPointerDown={(event) => event.stopPropagation()}
        onPointerMove={(event) => event.stopPropagation()}
        onPointerUp={(event) => event.stopPropagation()}
      >
        <div className="flex items-baseline gap-3 flex-wrap justify-center">
          <span className="text-sm font-bold text-amber-300">校准</span>
          <span className="text-[13px] text-slate-200">
            {KIND_OPTIONS.find((option) => option.kind === kind)?.hint ?? "拖框套住目标那一行"}
            （拖完自动测试，认对了自动保存；Esc 取消）
          </span>
        </div>
        <div className="flex items-center gap-2 justify-center flex-wrap">
          {KIND_OPTIONS.map((option) => (
            <button
              key={option.kind}
              type="button"
              disabled={busy}
              onClick={() => switchKind(option.kind)}
              className={`px-3 py-1 rounded-lg text-xs font-bold border transition-colors disabled:opacity-40 cursor-pointer ${
                kind === option.kind
                  ? "bg-amber-500/20 border-amber-500/50 text-amber-200"
                  : "bg-slate-800/80 border-white/15 text-slate-300 hover:bg-slate-700"
              }`}
            >
              {option.label}
            </button>
          ))}
          <span className="w-px h-5 bg-white/15" />
          <button
            type="button"
            disabled={busy || !rect}
            onClick={runTest}
            className="px-3 py-1.5 rounded-lg border border-sky-500/40 text-sky-200 hover:bg-sky-500/10 text-xs font-bold flex items-center gap-1.5 disabled:opacity-40 cursor-pointer"
          >
            {busy ? <Loader2 className="w-3.5 h-3.5 animate-spin" /> : <Target className="w-3.5 h-3.5" />}
            重试读数
          </button>
          <button
            type="button"
            disabled={busy || !rect}
            onClick={save}
            className={`px-4 py-1.5 rounded-lg text-slate-950 text-xs font-bold flex items-center gap-1.5 disabled:opacity-40 cursor-pointer ${
              test?.ok ? "bg-emerald-400 hover:bg-emerald-300" : "bg-amber-500 hover:bg-amber-400"
            }`}
          >
            保存并关闭
          </button>
          <button
            type="button"
            onClick={cancel}
            className="px-3 py-1.5 rounded-lg bg-slate-800/80 hover:bg-slate-700 border border-white/15 text-slate-300 text-xs font-bold flex items-center gap-1.5 cursor-pointer"
          >
            <X className="w-3.5 h-3.5" />
            取消
          </button>
        </div>
        {test && (
          <div
            className={`text-xs leading-relaxed text-center ${
              test.ok ? "text-emerald-300" : "text-red-300"
            }`}
          >
            {test.ok ? "✓ " : "✗ "}
            {test.message}
          </div>
        )}
        {message && (
          <div
            className={`text-xs leading-relaxed text-center ${
              test?.ok ? "text-emerald-200/80" : "text-amber-200"
            }`}
          >
            {message}
          </div>
        )}
      </div>
    </div>
  );
};
