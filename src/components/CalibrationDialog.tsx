import React, { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Crop, Loader2, RotateCcw, Sparkles, Target, X } from "lucide-react";
import type { CalibrationFrame, CalibrationTest, NormRect, RegionProfile } from "../types";
import { formatShortDate } from "../lib/format";
import { InlineNotice, type Notice } from "./Notice";

/**
 * 经验行校准浮层：**自动识别 + 手动框选**。
 *
 * ## 为什么是主窗内的浮层而不是新窗口
 *
 * 本项目在同步命令里建 WebView2 控制器死锁过（登录窗白屏那回），
 * 开窗必须 async + 线程投递 —— 浮层零风险，还省一个 WebView 实例。
 *
 * ## 坐标的模型：比例才是那个不变量
 *
 * 经验行会跟着游戏分辨率 / 界面缩放走（1080p 字高 7px、1440p 约 9~10、4K 约 14）。
 * 所以落库的是**比例矩形**（x/y/w/h 相对客户区），不是像素 —— 换分辨率不用重框。
 * 前端只做「鼠标 CSS 坐标 → 客户区物理像素」这一件事，比例换算交给保存命令；
 * 后端读的时候再把比例乘回当前客户区。
 *
 * ## 坐标换算的三层等式（全组件唯一入口是 `toClient`，别处不要再换算）
 *
 *   客户区物理像素 = (事件 CSS 坐标 − 图像左上角 CSS 坐标) × host_w ÷ 渲染 CSS 宽 + host 原点
 *
 * 三层是：① 鼠标事件给的 **CSS 像素**（受 Windows 缩放 × Tauri 逻辑坐标影响）
 * → ② **图像像素**（后端把客户区按 max_width 等比缩小后的 image_w × image_h）
 * → ③ **客户区物理像素**（框的暂存单位，保存时再换算成比例）。
 * `host` 是「这个元素显示的是客户区的哪一块」：select 相 = 整幅 {0,0,client_w,client_h}，
 * refine 相 = 外扩后的视图（只显示一个子区域）。渲染 CSS 宽 = host 宽 × k
 * （k = 任意 CSS/窗口缩放），host 又是从 client 等比来的 —— 代入后 k 与 s
 * 全部约掉，式子与 devicePixelRatio 无关，所以这里**绝不显式乘/除 dpr**。
 * 本项目在 DPI 缩放上栽过「坐标整体偏移」：多乘一个系数就错一层。
 *
 * ## 「测试读数」不落库
 *
 * 读数链抓屏用的是**已保存**的校准。所以测试走独立命令 `test_exp_region`：
 * 后端拿这个临时框立刻读一次，**不改**用户已有的校准；「保存」是另一个动作。
 * 旧实现「先保存再等下一轮采样」的问题：点一次测试就已经改了用户的选择，
 * 测失败时想恢复都没有退路。
 */

/** 框的最小边（客户区像素）：再小就是误触，交给后端裁剪只会出乱子。 */
const MIN_RECT_PX = 8;
/** 抓屏请求的图像宽：对 4K 也只差 2.4×，精调放大后认边足够；再大 IPC 体积不划算。 */
const FRAME_MAX_WIDTH = 1600;
/** 精调视图四周多露的比例（最少 16px）：框外必须可见，边缘才有东西可拉。 */
const REFINE_VIEW_PAD_RATIO = 0.35;
/** 经验行这个 kind（后端 `exp::region::KIND_EXP_LINE`）。 */
const KIND_EXP_LINE = "exp_line";
/**
 * 粗框视图的放大档位（1 = 适应窗口）。
 *
 * 1080p 整幅画面缩到对话框里时，经验那一行只有 ~50 像素宽 —— 直接在上面拖框
 * 纯靠手稳。放大到 2~3 倍、用滚动条看底部 HUD，再拖就轻松得多；
 * 拖完还有「放大微调」那一相兜底。
 */
const ZOOM_LEVELS = [1, 1.5, 2, 3] as const;

/** 客户区物理像素下的矩形（浮点暂存，保存时才换算成比例）。 */
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

type Edge = "top" | "bottom" | "left" | "right";

/** 单边拖拽后的矩形：只动被拖的那条边，另一侧钉住，并保住最小边。 */
function rectWithEdge(
  rect: Rect,
  edge: Edge,
  value: number,
  clientW: number,
  clientH: number,
): Rect {
  const bottom = rect.y + rect.h;
  const right = rect.x + rect.w;
  switch (edge) {
    case "top": {
      const y = clamp0(value, bottom - MIN_RECT_PX);
      return { ...rect, y, h: bottom - y };
    }
    case "bottom":
      return { ...rect, h: clamp0(value - rect.y, clientH - rect.y) };
    case "left": {
      const x = clamp0(value, right - MIN_RECT_PX);
      return { ...rect, x, w: right - x };
    }
    case "right":
      return { ...rect, w: clamp0(value - rect.x, clientW - rect.x) };
  }
}

/** 精调视图 = 矩形四周外扩一圈（框外必须可见，边缘才有东西可拉）。 */
function refineView(rect: Rect, frame: CalibrationFrame): Rect {
  const pad = Math.max(16, Math.round(Math.max(rect.w, rect.h) * REFINE_VIEW_PAD_RATIO));
  const x = clamp0(rect.x - pad, frame.client_w);
  const y = clamp0(rect.y - pad, frame.client_h);
  return {
    x,
    y,
    w: Math.min(rect.w + (rect.x - x) + pad, frame.client_w - x),
    h: Math.min(rect.h + (rect.y - y) + pad, frame.client_h - y),
  };
}

/** 矩形 → 百分比定位（host 是承载它的那层视图）。百分比天生跟随窗口缩放，
 * 不用 ResizeObserver 量 DOM —— 量了反而是新的一层坐标要维护。 */
function percentBox(r: Rect, host: Rect) {
  return {
    left: `${((r.x - host.x) / host.w) * 100}%`,
    top: `${((r.y - host.y) / host.h) * 100}%`,
    width: `${(r.w / host.w) * 100}%`,
    height: `${(r.h / host.h) * 100}%`,
  };
}

/** 落库 / 测试用的比例矩形（客户区物理像素 ÷ 客户区尺寸）。 */
function toNorm(rect: Rect, frame: CalibrationFrame): NormRect {
  return {
    x: rect.x / frame.client_w,
    y: rect.y / frame.client_h,
    w: rect.w / frame.client_w,
    h: rect.h / frame.client_h,
  };
}

/** 已有的校准档换算回像素，显示在画面上。 */
function fromNorm(norm: NormRect, frame: CalibrationFrame): Rect {
  return {
    x: norm.x * frame.client_w,
    y: norm.y * frame.client_h,
    w: norm.w * frame.client_w,
    h: norm.h * frame.client_h,
  };
}

const SCREEN_MODE_LABEL: Record<string, string> = {
  Windowed: "窗口化",
  Framed: "带边框全屏",
  Borderless: "无边框全屏",
};

export const CalibrationDialog: React.FC<{ onClose: () => void }> = ({ onClose }) => {
  const [frame, setFrame] = useState<CalibrationFrame | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [profile, setProfile] = useState<RegionProfile | null>(null);
  /** 'select' = 整幅画面拖粗框 / 'refine' = 放大视图微调四边 */
  const [phase, setPhase] = useState<"select" | "refine">("select");
  /** 粗框视图的放大倍率（1 = 适应窗口，可滚动） */
  const [zoom, setZoom] = useState<number>(2);
  /** 框，单位是**客户区物理像素**（浮点暂存，保存时才换算成比例） */
  const [rect, setRect] = useState<Rect | null>(null);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [testing, setTesting] = useState(false);
  const [testingKind, setTestingKind] = useState<"manual" | "auto">("manual");
  const [testResult, setTestResult] = useState<CalibrationTest | null>(null);
  const [busyGuard, setBusyGuard] = useState(false);
  const [saving, setSaving] = useState(false);

  const stageRef = useRef<HTMLDivElement>(null);
  /** 放大视图里真正承载图像与选框的那一层（滚动容器里面的内容层）。
   *  鼠标坐标换算要用它 —— 滚动偏移都在它的 boundingRect 里。 */
  const stageWrapRef = useRef<HTMLDivElement>(null);
  const refineBoxRef = useRef<HTMLDivElement>(null);
  const cropCanvasRef = useRef<HTMLCanvasElement>(null);
  /** 解码好的整幅图像：粗框背景与精调裁剪共用（避免 base64 反复解码）。
   *  imgKeyRef 记这个对象对应哪张图 —— 绘制前先配对，防止画成上一帧。
   *  decodeTick 只在解码回调里递增（回调里 setState 不受「effect 同步
   *  setState」检查约束），它负责在解码完成时把画布重画出来。 */
  const imgObjRef = useRef<HTMLImageElement | null>(null);
  const imgKeyRef = useRef<string>("");
  const [decodeTick, setDecodeTick] = useState(0);
  /** 粗框拖拽的锚点（客户区像素） */
  const dragAnchorRef = useRef<{ x: number; y: number } | null>(null);
  /** rect 的同步镜像：拖拽事件里要拿「当前矩形」推 host，state 可能落后一拍 */
  const rectRef = useRef<Rect | null>(null);
  /** 切换放大倍率时，记录视口中心在图像上的相对位置（切完还原，视野不跳） */
  const zoomAnchorRef = useRef<{ x: number; y: number } | null>(null);
  /** 正在拖的边 */
  const edgeRef = useRef<Edge | null>(null);
  /** 写 rect 的唯一入口：state 给渲染，ref 给事件处理器同步读（见上条注释） */
  const applyRect = useCallback((next: Rect | null) => {
    rectRef.current = next;
    setRect(next);
  }, []);

  // 抓屏是十几毫秒的阻塞调用，必须走 async 命令（同步命令在 UI 线程上跑，
  // 抓屏会卡住整个界面）。
  useEffect(() => {
    let alive = true;
    invoke<CalibrationFrame>("calibration_frame", { maxWidth: FRAME_MAX_WIDTH })
      .then((data) => {
        if (!alive) return;
        setFrame(data);
        return invoke<RegionProfile | null>("get_region_profile", {
          kind: KIND_EXP_LINE,
        }).then((existing) => {
          if (!alive) return;
          setProfile(existing);
          // 校准过就直接从上次的框开始精调 —— 重校准多数是「挪一点点」，
          // 从空白重拖是惩罚。
          if (existing) {
            const fromProfile = fromNorm(existing.rect, data);
            rectRef.current = fromProfile;
            setRect(fromProfile);
            setPhase("refine");
          }
        });
      })
      .catch((err: unknown) => {
        if (alive)
          setLoadError(typeof err === "string" ? err : `抓取游戏画面失败：${String(err)}`);
      });
    return () => {
      alive = false;
    };
  }, []);

  // 整幅图解码一份存 ref：粗框背景与精调裁剪都从它来。
  // 不在 effect 同步段 setState —— 解码完成是在 onload 回调里通知的。
  useEffect(() => {
    if (!frame) return;
    const img = new Image();
    img.onload = () => {
      imgObjRef.current = img;
      imgKeyRef.current = frame.png_base64;
      setDecodeTick((tick) => tick + 1);
    };
    img.src = `data:image/png;base64,${frame.png_base64}`;
  }, [frame]);

  // Esc 关浮层：主窗没有别的全局 Esc 监听（那是悬浮窗的事），这里独占安全
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  // 进粗框视图时把滚动条拉到底部：经验那一行在 HUD 上（画面最底下），
  // 放大之后先看到底部，省得每次都要自己滚。没有校准档时才走这个相位。
  useEffect(() => {
    if (phase !== "select" || !frame) return;
    const stage = stageRef.current;
    if (!stage) return;
    stage.scrollTop = stage.scrollHeight;
    // 只在进入粗框视图 / 换图时拉到底；换倍率时由 zoomAnchorRef 那一条接管视野。
  }, [phase, frame]);

  // 切换放大倍率：按切换前视口中心的相对位置还原滚动，视野不跳。
  useLayoutEffect(() => {
    const anchor = zoomAnchorRef.current;
    zoomAnchorRef.current = null;
    if (!anchor) return;
    const stage = stageRef.current;
    const wrap = stageWrapRef.current;
    if (!stage || !wrap) return;
    stage.scrollLeft = anchor.x * wrap.clientWidth - stage.clientWidth / 2;
    stage.scrollTop = anchor.y * wrap.clientHeight - stage.clientHeight / 2;
  }, [zoom]);

  /** 切放大倍率：先记下当前视口中心在图像上的相对位置（见上面的还原 effect）。 */
  const changeZoom = (level: number) => {
    const stage = stageRef.current;
    const wrap = stageWrapRef.current;
    if (stage && wrap && wrap.clientWidth > 0 && wrap.clientHeight > 0) {
      zoomAnchorRef.current = {
        x: (stage.scrollLeft + stage.clientWidth / 2) / wrap.clientWidth,
        y: (stage.scrollTop + stage.clientHeight / 2) / wrap.clientHeight,
      };
    }
    setZoom(level);
  };

  /** 三层换算唯一入口（等式见文件头）。`el` 是图像承载元素，
   *  `host` 是「它显示的是客户区的哪一块」—— 两边必须一致，否则坐标偏移。 */
  const toClient = (e: { clientX: number; clientY: number }, el: HTMLElement | null, host: Rect) => {
    if (!el || !frame) return null;
    const box = el.getBoundingClientRect();
    if (box.width <= 0 || host.w <= 0 || host.h <= 0) return null;
    return {
      x: (e.clientX - box.left) * (host.w / box.width) + host.x,
      y: (e.clientY - box.top) * (host.h / box.height) + host.y,
    };
  };

  const onStagePointerDown = (e: React.PointerEvent) => {
    if (e.button !== 0 || !frame) return;
    e.preventDefault();
    stageWrapRef.current?.setPointerCapture(e.pointerId);
    // select 相显示整幅客户区，host 就是它本身（stageWrapRef 的 rect 含滚动偏移）
    const p = toClient(e, stageWrapRef.current, { x: 0, y: 0, w: frame.client_w, h: frame.client_h });
    if (!p) return;
    dragAnchorRef.current = p;
    applyRect({ x: p.x, y: p.y, w: 0, h: 0 });
  };

  const onStagePointerMove = (e: React.PointerEvent) => {
    const anchor = dragAnchorRef.current;
    if (!anchor || !frame) return;
    const p = toClient(e, stageWrapRef.current, { x: 0, y: 0, w: frame.client_w, h: frame.client_h });
    if (!p) return;
    applyRect(normalizeRect(anchor, p, frame.client_w, frame.client_h));
  };

  const onStagePointerUp = (e: React.PointerEvent) => {
    const anchor = dragAnchorRef.current;
    if (!anchor || !frame) return;
    dragAnchorRef.current = null;
    stageWrapRef.current?.releasePointerCapture?.(e.pointerId);
    const p = toClient(e, stageWrapRef.current, { x: 0, y: 0, w: frame.client_w, h: frame.client_h });
    const done = p ? normalizeRect(anchor, p, frame.client_w, frame.client_h) : null;
    if (!done || done.w < MIN_RECT_PX || done.h < MIN_RECT_PX) {
      applyRect(null);
      return;
    }
    applyRect(done);
    setPhase("refine");
  };

  const onEdgePointerDown = (edge: Edge) => (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    refineBoxRef.current?.setPointerCapture(e.pointerId);
    edgeRef.current = edge;
  };

  const onRefinePointerMove = (e: React.PointerEvent) => {
    const edge = edgeRef.current;
    const current = rectRef.current;
    if (!edge || !frame || !current) return;
    // host 从**最新矩形**推（ref 镜像）：放大视图跟着矩形外扩，
    // 用旧视图换算会把拖出去的边折回来（拖拽自我漂移）。
    const p = toClient(e, refineBoxRef.current, refineView(current, frame));
    if (!p) return;
    applyRect(
      rectWithEdge(
        current,
        edge,
        edge === "top" || edge === "bottom" ? p.y : p.x,
        frame.client_w,
        frame.client_h,
      ),
    );
  };

  const onRefinePointerUp = () => {
    edgeRef.current = null;
  };

  /** 方向键整体微挪（Shift = 4px）：粗框常差一两像素，键盘比拖边稳。 */
  const onRefineKeyDown = (e: React.KeyboardEvent) => {
    if (!rect || !frame) return;
    const step = e.shiftKey ? 4 : 1;
    const moves: Record<string, [number, number]> = {
      ArrowUp: [0, -step],
      ArrowDown: [0, step],
      ArrowLeft: [-step, 0],
      ArrowRight: [step, 0],
    };
    const move = moves[e.key];
    if (!move) return;
    e.preventDefault();
    applyRect({
      x: clamp0(rect.x + move[0], frame.client_w - rect.w),
      y: clamp0(rect.y + move[1], frame.client_h - rect.h),
      w: rect.w,
      h: rect.h,
    });
  };

  /** 手动保存当前框（比例坐标）。 */
  const saveRect = useCallback(async (): Promise<RegionProfile | null> => {
    if (!rect || !frame) return null;
    setSaving(true);
    try {
      const saved = await invoke<RegionProfile>("save_region_profile", {
        kind: KIND_EXP_LINE,
        rect: toNorm(rect, frame),
        textHeight: profile?.text_height ?? null,
      });
      setProfile(saved);
      setNotice(null);
      return saved;
    } catch (err: unknown) {
      setNotice({
        kind: "error",
        message: typeof err === "string" ? err : `保存校准失败：${String(err)}`,
      });
      return null;
    } finally {
      setSaving(false);
    }
  }, [rect, frame, profile]);

  /** 测试当前框：**不落库**，后端拿临时框立刻读一次。 */
  const testRect = async () => {
    if (!rect || !frame || testing) return;
    setTesting(true);
    setTestingKind("manual");
    setTestResult(null);
    try {
      const result = await invoke<CalibrationTest>("test_exp_region", {
        kind: KIND_EXP_LINE,
        rect: toNorm(rect, frame),
      });
      setTestResult(result);
      if (result.ok && result.text_height) {
        // 实测字高顺手记下来（诊断 / 下次抓框留的余量都用它）
        setProfile((current) => (current ? { ...current, text_height: result.text_height } : current));
      }
    } catch (err: unknown) {
      setTestResult({
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
      setTesting(false);
    }
  };

  /** 自动识别：抓一整帧定位，成功即以它为准（显式动作，覆盖手动档）。 */
  const autoDetect = async () => {
    if (!frame || testing) return;
    setTesting(true);
    setTestingKind("auto");
    setTestResult(null);
    try {
      const result = await invoke<CalibrationTest>("auto_calibrate_exp");
      setTestResult(result);
      if (result.ok && result.profile) {
        setProfile(result.profile);
        const found = fromNorm(result.profile.rect, frame);
        applyRect(found);
        setPhase("refine");
      }
    } catch (err: unknown) {
      setTestResult({
        ok: false,
        message: typeof err === "string" ? err : `自动识别失败：${String(err)}`,
        raw: null,
        exp: null,
        percent: null,
        level: null,
        text_height: null,
        profile: null,
      });
    } finally {
      setTesting(false);
    }
  };

  /** 恢复自动 = 清掉这个 kind 的校准（所有分辨率共一条记录）。 */
  const restoreAuto = async () => {
    setBusyGuard(true);
    try {
      await invoke("clear_region_profiles", { kind: KIND_EXP_LINE });
      setProfile(null);
      setTestResult(null);
      setRect(null);
      rectRef.current = null;
      setPhase("select");
      setNotice({ kind: "success", message: "已恢复自动识别（删除校准记录，下次读数整幅自动定位）" });
    } catch (err: unknown) {
      setNotice({
        kind: "error",
        message: typeof err === "string" ? err : `恢复自动失败：${String(err)}`,
      });
    } finally {
      setBusyGuard(false);
    }
  };

  // 精调裁剪：rect 按后端降采样比换回图像像素，从整幅图切出来；canvas 里
  // 1 图像像素 = 1 backing 像素，不插值，放大交给 CSS pixelated。
  useEffect(() => {
    // imgKey 配对：还没解码完（或 frame 已换）时不画，等 decodeTick 重画，
    // 避免把上一帧的像素画进新框。
    if (
      phase !== "refine" ||
      !frame ||
      !rect ||
      !imgObjRef.current ||
      imgKeyRef.current !== frame.png_base64
    )
      return;
    const canvas = cropCanvasRef.current;
    if (!canvas) return;
    const s = frame.image_w / frame.client_w;
    const view = refineView(rect, frame);
    const vw = Math.max(1, Math.round(view.w * s));
    const vh = Math.max(1, Math.round(view.h * s));
    canvas.width = vw;
    canvas.height = vh;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.imageSmoothingEnabled = false;
    ctx.drawImage(imgObjRef.current, view.x * s, view.y * s, view.w * s, view.h * s, 0, 0, vw, vh);
  }, [phase, frame, rect, decodeTick]);

  const dataUrl = frame ? `data:image/png;base64,${frame.png_base64}` : "";
  const refineNow = frame && rect ? refineView(rect, frame) : null;
  const testPassed = testResult?.ok === true && testingKind === "manual";
  const edges: Array<{ edge: Edge; className: string }> = [
    { edge: "top", className: "left-0 right-0 top-0 h-2 -translate-y-1/2 cursor-ns-resize" },
    { edge: "bottom", className: "left-0 right-0 bottom-0 h-2 translate-y-1/2 cursor-ns-resize" },
    { edge: "left", className: "top-0 bottom-0 left-0 w-2 -translate-x-1/2 cursor-ew-resize" },
    { edge: "right", className: "top-0 bottom-0 right-0 w-2 translate-x-1/2 cursor-ew-resize" },
  ];

  return (
    <div
      className="anim-fade fixed inset-0 z-[999] flex items-center justify-center bg-slate-950/70 p-4"
      onClick={onClose}
    >
      <div
        className="anim-pop w-full max-w-[min(96vw,1440px)] max-h-[92vh] overflow-y-auto rounded-2xl border border-amber-500/30 bg-slate-900 shadow-2xl"
        onClick={(event) => event.stopPropagation()}
      >
        {/* 标题：状态胶囊说明「我们抓的是哪个窗口、多大、校准没有」 */}
        <div className="flex items-start justify-between gap-3 px-5 pt-4 pb-2">
          <div className="min-w-0">
            <div className="flex items-center gap-2 flex-wrap">
              <Crop className="w-4 h-4 text-amber-400 shrink-0" />
              <span className="text-sm font-bold text-slate-100">校准 · 经验行位置</span>
              {frame && (
                <span className="text-[10px] px-1.5 py-0.5 rounded-full bg-slate-800 text-slate-300 border border-white/10 tabular-nums">
                  {frame.client_w}×{frame.client_h} ·{" "}
                  {SCREEN_MODE_LABEL[frame.screen_mode] ?? frame.screen_mode}
                </span>
              )}
              {frame && (
                <span
                  className={`text-[10px] px-1.5 py-0.5 rounded-full border font-bold ${
                    profile
                      ? "bg-emerald-500/15 text-emerald-300 border-emerald-500/30"
                      : "bg-slate-800 text-slate-400 border-white/10"
                  }`}
                >
                  {profile
                    ? `已校准（${profile.source === "manual" ? "手动" : "自动"}）`
                    : "未校准（整幅自动定位）"}
                </span>
              )}
            </div>
            <p className="text-[11px] text-slate-400 mt-1 leading-relaxed">
              校准存的是比例：2560×1440 框出来的位置，换到别的分辨率（改 Windows 缩放、
              游戏改分辨率都算）照样落在这行字上；读不准时会自动退回整幅画面重新找。
              自动定位学到的位置连续两帧稳定才落库，手动框选的位置永不被它覆盖。
            </p>
          </div>
          <button
            type="button"
            onClick={onClose}
            title="关闭（Esc）"
            className="p-0.5 rounded text-slate-500 hover:text-slate-200 hover:bg-white/10 cursor-pointer shrink-0"
          >
            <X className="w-4 h-4" />
          </button>
        </div>

        <div className="px-5 pb-5 space-y-3">
          <InlineNotice notice={notice} onClose={() => setNotice(null)} />

          {/* 自动识别：一键把当前这帧里的经验行框出来（不用手拖） */}
          <div className="flex items-center gap-2 flex-wrap">
            <button
              type="button"
              onClick={autoDetect}
              disabled={!frame || testing || busyGuard}
              className="px-3 py-1.5 rounded-lg bg-sky-500/15 hover:bg-sky-500/25 border border-sky-500/40 text-sky-200 text-xs font-semibold flex items-center gap-1.5 disabled:opacity-50 cursor-pointer transition-colors"
            >
              {testing && testingKind === "auto" ? (
                <Loader2 className="w-3.5 h-3.5 animate-spin" />
              ) : (
                <Sparkles className="w-3.5 h-3.5" />
              )}
              自动识别并定位
            </button>
            <span className="text-[11px] text-slate-500">
              抓当前这帧整幅画面，多尺度找「EXP xx(xx.xx%)」那一行；找不到就手动画框。
            </span>
          </div>

          {loadError && (
            <div className="p-3 rounded-lg bg-red-950/40 border border-red-500/30 text-xs text-red-300">
              {loadError}
            </div>
          )}

          {!frame && !loadError && (
            <div className="py-10 text-center text-xs text-slate-400 flex items-center justify-center gap-2">
              <Loader2 className="w-4 h-4 animate-spin" />
              正在抓取游戏画面…
            </div>
          )}

          {frame && (
            <>
              <p className="text-[11px] text-slate-500">
                {phase === "select"
                  ? "第 1 步：画面已放大，滚轮滚到经验那一行，按住鼠标拖出一个大致的框（套住它即可）。"
                  : "第 2 步：拖琥珀色边条微调，方向键移动整个框（Shift = 4px），认准了再测试。"}
              </p>

              {phase === "select" && (
                <div className="space-y-1.5">
                  {/* 放大倍率：整幅缩在对话框里时那一行只有几十像素宽，放大后再拖 */}
                  <div className="flex items-center justify-between gap-2 flex-wrap">
                    <span className="text-[11px] text-slate-500">
                      画面已放大 {zoom}×（可滚动查看）；拖完会自动进入放大微调。
                    </span>
                    <div className="flex items-center gap-1">
                      {ZOOM_LEVELS.map((level) => (
                        <button
                          key={level}
                          type="button"
                          onClick={() => changeZoom(level)}
                          className={`px-2 py-0.5 rounded text-[11px] border cursor-pointer transition-colors ${
                            zoom === level
                              ? "bg-amber-500/20 border-amber-500/40 text-amber-200"
                              : "border-white/10 text-slate-400 hover:bg-slate-800"
                          }`}
                        >
                          {level === 1 ? "适应" : `${level}×`}
                        </button>
                      ))}
                    </div>
                  </div>
                  <div
                    ref={stageRef}
                    className="relative overflow-auto overscroll-contain select-none touch-none cursor-crosshair rounded-lg border border-white/10"
                    style={{ maxHeight: "58vh" }}
                    onPointerDown={onStagePointerDown}
                    onPointerMove={onStagePointerMove}
                    onPointerUp={onStagePointerUp}
                  >
                    {/* 内容层：宽度 = 倍率 × 容器宽，比例锁客户区 —— 滚动偏移
                        全在它的 boundingRect 里，坐标换算不用另算滚动量。 */}
                    <div
                      ref={stageWrapRef}
                      className="relative"
                      style={{
                        width: `${zoom * 100}%`,
                        aspectRatio: `${frame.client_w} / ${frame.client_h}`,
                      }}
                    >
                      <img
                        src={dataUrl}
                        alt="游戏客户区画面"
                        draggable={false}
                        className="absolute inset-0 w-full h-full pointer-events-none"
                      />
                      {rect && (
                        <div
                          className="absolute border-2 border-amber-400 pointer-events-none"
                          style={{
                            ...percentBox(rect, { x: 0, y: 0, w: frame.client_w, h: frame.client_h }),
                            // 外圈压暗：一眼看出框外的部分不算数
                            boxShadow: "0 0 0 9999px rgba(0, 0, 0, 0.45)",
                          }}
                        />
                      )}
                    </div>
                  </div>
                </div>
              )}

              {phase === "refine" && refineNow && (
                <div className="space-y-2" tabIndex={0} onKeyDown={onRefineKeyDown}>
                  <div
                    ref={refineBoxRef}
                    className="relative mx-auto select-none touch-none rounded-lg overflow-hidden border border-amber-400/40"
                    // 宽度按裁剪比例反推，保证高度不超过 58vh；canvas 始终铺满这一层，
                    // 所以框/把手的百分比定位不受影响。
                    style={{ maxWidth: `min(100%, ${((58 * refineNow.w) / refineNow.h).toFixed(0)}vh)` }}
                    onPointerMove={onRefinePointerMove}
                    onPointerUp={onRefinePointerUp}
                  >
                    <canvas
                      ref={cropCanvasRef}
                      className="block w-full"
                      style={{ imageRendering: "pixelated" }}
                    />
                    {/* 矩形边界画在放大视图上（百分比定位）：把手拖的就是它们，
                        canvas 只负责显示像素 */}
                    <div
                      className="absolute border-2 border-amber-400 pointer-events-none"
                      style={percentBox(rect!, refineNow)}
                    />
                    {edges.map(({ edge, className }) => {
                      // 把手是**贴在边上的细条**（跨过边线，一半里一半外），
                      // 不是盖住整块矩形 —— 写错成整块的话鼠标到处都变拖拽。
                      // 百分比 + calc 混合定位，不用另量像素尺寸。
                      const b = percentBox(rect!, refineNow);
                      const strip = "8px";
                      const style =
                        edge === "top"
                          ? { left: b.left, width: b.width, top: `calc(${b.top} - 4px)`, height: strip }
                          : edge === "bottom"
                            ? { left: b.left, width: b.width, top: `calc(${b.top} + ${b.height} - 4px)`, height: strip }
                            : edge === "left"
                              ? { top: b.top, height: b.height, left: `calc(${b.left} - 4px)`, width: strip }
                              : { top: b.top, height: b.height, left: `calc(${b.left} + ${b.width} - 4px)`, width: strip };
                      return (
                        <div
                          key={edge}
                          className={`absolute bg-amber-400/70 hover:bg-amber-300 transition-colors ${className}`}
                          style={style}
                          onPointerDown={onEdgePointerDown(edge)}
                        />
                      );
                    })}
                  </div>
                  <p className="text-[11px] text-slate-500">
                    拖琥珀色边条调边；方向键 / Shift+方向键移动整个框（先点一下这个视图获得焦点）。
                  </p>
                </div>
              )}

              {/* 测试读数：显示真实读取链的结论（临时框，不落库） */}
              {testResult && (
                <div
                  role="status"
                  className={`text-xs rounded-lg px-3 py-2 border leading-relaxed ${
                    testResult.ok
                      ? "bg-emerald-500/10 border-emerald-500/30 text-emerald-300"
                      : "bg-red-950/40 border-red-500/30 text-red-300"
                  }`}
                >
                  {testResult.ok ? "✓ " : "✗ "}
                  {testResult.message}
                  {testResult.ok && testingKind === "manual" && (
                    <span className="text-emerald-300/70"> —— 框对了，点「保存并关闭」记住它。</span>
                  )}
                  {!testResult.ok && (
                    <span className="text-red-300/70"> —— 可以继续微调再测。</span>
                  )}
                </div>
              )}

              <div className="flex items-center justify-between gap-2 flex-wrap">
                <button
                  type="button"
                  onClick={restoreAuto}
                  disabled={busyGuard}
                  title="删掉这条校准记录，回到整幅画面自动定位"
                  className="px-3 py-1.5 rounded-lg border border-white/10 text-xs text-slate-300 hover:bg-slate-800 disabled:opacity-50 flex items-center gap-1.5 cursor-pointer"
                >
                  <RotateCcw className="w-3.5 h-3.5" />
                  恢复自动
                </button>

                <div className="flex items-center gap-2">
                  <button
                    type="button"
                    onClick={onClose}
                    className="px-3 py-1.5 rounded-lg border border-white/10 text-xs text-slate-300 hover:bg-slate-800 cursor-pointer"
                  >
                    关闭
                  </button>
                  <button
                    type="button"
                    onClick={testRect}
                    disabled={!rect || testing}
                    title="用这个框立刻读一次，不修改已保存的校准"
                    className="px-3 py-1.5 rounded-lg border border-sky-500/40 text-sky-200 hover:bg-sky-500/10 text-xs font-semibold disabled:opacity-50 flex items-center gap-1.5 cursor-pointer"
                  >
                    {testing && testingKind === "manual" ? (
                      <Loader2 className="w-3.5 h-3.5 animate-spin" />
                    ) : (
                      <Target className="w-3.5 h-3.5" />
                    )}
                    测试读数
                  </button>
                  <button
                    type="button"
                    onClick={async () => {
                      if (await saveRect()) onClose();
                    }}
                    disabled={!rect || saving || busyGuard}
                    title="按当前框保存（比例坐标），下一轮采样起生效"
                    className={`px-4 py-1.5 rounded-lg text-slate-950 text-xs font-bold flex items-center gap-1.5 disabled:opacity-50 cursor-pointer ${
                      testPassed
                        ? "bg-emerald-400 hover:bg-emerald-300"
                        : "bg-amber-500 hover:bg-amber-400"
                    }`}
                  >
                    {saving ? (
                      <Loader2 className="w-3.5 h-3.5 animate-spin" />
                    ) : (
                      <Crop className="w-3.5 h-3.5" />
                    )}
                    {testPassed ? "保存并关闭（已测通）" : "保存并关闭"}
                  </button>
                </div>
              </div>

              {profile && (
                <p className="text-[10px] text-slate-500 tabular-nums">
                  当前校准：比例 ({profile.rect.x.toFixed(4)}, {profile.rect.y.toFixed(4)}){" "}
                  {profile.rect.w.toFixed(4)}×{profile.rect.h.toFixed(4)}
                  {profile.text_height ? ` · 字高 ${profile.text_height}px` : ""}（
                  {profile.source === "manual" ? "手动" : "自动"}，
                  {formatShortDate(profile.learned_unix)} 记录）
                </p>
              )}
            </>
          )}
        </div>
      </div>
    </div>
  );
};
