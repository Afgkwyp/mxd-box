import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { Camera, Copy, Download, ImagePlus, RefreshCw, Trash2, X } from "lucide-react";
import { InlineNotice, type Notice } from "./Notice";
import {
  ART_ASPECT,
  RARITY_STYLE,
  cardFlair,
  copyShareImage,
  drawExpCard,
  hasTransparency,
  loadAvatar,
  loadCrop,
  renderShareImage,
  saveShareImage,
  sparkleTexture,
  storeAvatar,
  type AvatarCrop,
  type CardAvatar,
  type ExpCardData,
} from "../lib/shareCard";
import { useMotion } from "../lib/motion";
import type { CalibrationFrame } from "../types";
import "../styles/shareCard.css";

/** 形象最长边存多大：再大卡面上也用不到，还白占本机存储 */
const AVATAR_MAX_SIDE = 720;

/** 图片地址 → 解好的图（解不开就 reject）。 */
function loadImage(url: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const image = new Image();
    image.onload = () => resolve(image);
    image.onerror = () => reject(new Error("图片打不开"));
    image.src = url;
  });
}
/** 裁剪框最小占画面宽的比例（再小框出来只剩几十个像素） */
const CROP_MIN = 0.06;

const errorText = (err: unknown) => (typeof err === "string" ? err : String(err));

/**
 * 小结卡片的分享弹窗：先看一眼这张卡，再决定复制还是存成图片。
 *
 * 卡是可以「拿在手里晃」的：鼠标在卡上移动时卡跟着倾斜，镭射和高光跟着走。
 * 这些只在预览里有 —— 发出去的是一张静态图，镭射定格在一个好看的角度（见 renderShareImage）。
 *
 * 分享到微信 / QQ 走**剪贴板**：桌面版的微信和 QQ 没有给别的程序用的「分享」接口，
 * 最短的路就是复制图片，到聊天框里粘贴。
 */
export const ShareCardDialog: React.FC<{
  data: ExpCardData;
  /** 别的历史记录的时速（评稀有度用，不含这一段自己） */
  pastRates: number[];
  /** 第几段；不知道就不画编号 */
  serial: number | null;
  onClose: () => void;
}> = ({ data, pastRates, serial, onClose }) => {
  const flair = useMemo(() => cardFlair(data, pastRates), [data, pastRates]);
  const style = RARITY_STYLE[flair.rarity];

  const [avatarUrl, setAvatarUrl] = useState<string | null>(loadAvatar);
  /** 解好的形象图，连同它是哪个地址解出来的（换了 / 删了形象时旧图立刻作废） */
  const [loaded, setLoaded] = useState<(CardAvatar & { url: string }) | null>(null);
  const avatar = loaded && loaded.url === avatarUrl ? loaded : null;
  /** 正在框形象时拿到的那张游戏画面；null = 在看卡片 */
  const [frame, setFrame] = useState<CalibrationFrame | null>(null);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<Notice | null>(null);
  /** 点一下卡片再翻一次（换 key 让入场动画重放） */
  const [flip, setFlip] = useState(0);

  const canvasRef = useRef<HTMLCanvasElement>(null);
  const cardRef = useRef<HTMLDivElement>(null);
  /** 鼠标在卡上的位置（0~1）；不在卡上时为 null，卡自己慢慢晃 */
  const pointer = useRef<{ x: number; y: number } | null>(null);
  const fileRef = useRef<HTMLInputElement>(null);
  /** 把倾斜循环叫醒（它在「关了动效 + 鼠标不在卡上」时会停） */
  const wake = useRef<(() => void) | null>(null);
  /** 设置里关了「界面动效」：不翻面、不自己晃，只跟鼠标 */
  const [motion] = useMotion();
  const still = !motion;

  useEffect(() => {
    if (!avatarUrl) return;
    let alive = true;
    loadImage(avatarUrl)
      .then((image) => {
        if (alive) setLoaded({ url: avatarUrl, image, cutout: hasTransparency(image) });
      })
      // 存的图坏了就当没有形象（`avatar` 保持 null），不让一张破图挡住整张卡
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [avatarUrl]);

  useEffect(() => {
    if (frame || !canvasRef.current) return;
    drawExpCard(canvasRef.current, data, flair, avatar, serial);
  }, [data, flair, avatar, serial, frame, flip]);

  /**
   * 倾斜与镭射：每帧把「现在的姿态」往「目标姿态」靠一点，再写进 CSS 变量。
   * 目标 = 鼠标位置；鼠标不在卡上时是一条慢慢画圈的轨迹（卡放着也在反光）。
   *
   * 关了「界面动效」时卡不自己晃 —— 但**鼠标移上去照样跟手**：那是用户自己在动，
   * 不是我们在放动画。所以这个循环归位后会停，由 `wake` 在鼠标进来时重新叫醒。
   * （第一版归位后就再也不醒，表现是「得先点一下卡片才会动」。）
   */
  useEffect(() => {
    if (frame) return;
    let x = 0.5;
    let y = 0.5;
    let raf = 0;
    const tick = (now: number) => {
      raf = 0;
      const hover = pointer.current;
      const targetX = hover ? hover.x : still ? 0.35 : 0.5 + 0.34 * Math.sin(now / 1900);
      const targetY = hover ? hover.y : still ? 0.3 : 0.5 + 0.26 * Math.cos(now / 2600);
      x += (targetX - x) * 0.12;
      y += (targetY - y) * 0.12;
      const element = cardRef.current;
      if (element) {
        // 自己晃的时候幅度收一半：一直大幅度摆会晕
        const swing = hover ? 1 : 0.45;
        element.style.setProperty("--px", (x * 100).toFixed(2));
        element.style.setProperty("--py", (y * 100).toFixed(2));
        element.style.setProperty("--rx", `${((0.5 - y) * 18 * swing).toFixed(2)}deg`);
        element.style.setProperty("--ry", `${((x - 0.5) * 22 * swing).toFixed(2)}deg`);
      }
      const settled = !hover && Math.abs(targetX - x) < 0.002 && Math.abs(targetY - y) < 0.002;
      if (!still || !settled) raf = window.requestAnimationFrame(tick);
    };
    const start = () => {
      if (!raf) raf = window.requestAnimationFrame(tick);
    };
    wake.current = start;
    start();
    return () => {
      window.cancelAnimationFrame(raf);
      wake.current = null;
    };
  }, [frame, flip, still]);

  // Esc：在框形象就退回卡片，在看卡片就关掉
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      if (frame) setFrame(null);
      else onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [frame, onClose]);

  const run = async (task: () => Promise<void>) => {
    setBusy(true);
    try {
      await task();
    } catch (err: unknown) {
      setNotice({ kind: "error", message: errorText(err) });
    } finally {
      setBusy(false);
    }
  };

  const exportPng = () => {
    if (!canvasRef.current) throw new Error("卡片还没画好");
    return renderShareImage(canvasRef.current, flair, data.endedAt || data.gainedExp);
  };

  const copy = () =>
    run(async () => {
      await copyShareImage(exportPng());
      setNotice({ kind: "success", message: "已复制。到微信 / QQ 的聊天框里按 Ctrl+V 就能发" });
    });

  const save = () =>
    run(async () => {
      const path = await saveShareImage(exportPng(), data.mapName);
      setNotice({ kind: "success", message: `已保存：${path}` });
    });

  /** 抓一张当前游戏画面，进入框选 */
  const grab = () =>
    run(async () => {
      setNotice(null);
      const shot = await invoke<CalibrationFrame>("calibration_frame", { maxWidth: 2048 });
      setFrame(shot);
    });

  /** 换形象：存进本机；存不下（图太大）也照用，只是下次打开要重新选 */
  const applyAvatar = (dataUrl: string, crop?: AvatarCrop) => {
    const kept = storeAvatar(dataUrl, crop);
    setAvatarUrl(dataUrl);
    setNotice(
      kept ? null : { kind: "info", message: "这张图太大，没能记住；这次照用，下次打开要重新选" },
    );
  };

  /**
   * 导入一张现成的图当形象 —— 比如纸娃娃模拟器导出的透明 PNG，或者自己修过的截图。
   * 带透明的整个摆在卡面自己的光里，不带透明的铺满形象窗。
   */
  const importImage = (file: File) =>
    run(async () => {
      const url = URL.createObjectURL(file);
      try {
        const image = await loadImage(url);
        const longest = Math.max(image.naturalWidth, image.naturalHeight);
        const scale = Math.min(1, AVATAR_MAX_SIDE / longest);
        const canvas = document.createElement("canvas");
        canvas.width = Math.max(1, Math.round(image.naturalWidth * scale));
        canvas.height = Math.max(1, Math.round(image.naturalHeight * scale));
        canvas.getContext("2d")?.drawImage(image, 0, 0, canvas.width, canvas.height);
        applyAvatar(canvas.toDataURL("image/png"));
      } catch {
        throw new Error("这个文件不是能打开的图片（支持 PNG / JPG / WebP）");
      } finally {
        URL.revokeObjectURL(url);
      }
    });

  const onPointerMove = useCallback((event: React.PointerEvent<HTMLDivElement>) => {
    const box = event.currentTarget.getBoundingClientRect();
    pointer.current = {
      x: Math.min(1, Math.max(0, (event.clientX - box.left) / box.width)),
      y: Math.min(1, Math.max(0, (event.clientY - box.top) / box.height)),
    };
    wake.current?.();
  }, []);

  const cardVars = {
    "--foil": style.foil,
    "--holo-accent": style.accent,
  } as React.CSSProperties;

  // 挂到 body 上：练级页外层有切页动画（transform），留在原地的话 `fixed` 会被它圈住，
  // 弹窗只盖得住内容区、卡片下半截还会被裁掉。
  return createPortal(
    <div
      className="fixed inset-0 z-50 flex items-center justify-center p-6 bg-black/75 backdrop-blur-sm anim-fade"
      onClick={onClose}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label="小结卡片"
        className="relative flex items-stretch gap-6 max-w-full max-h-full"
        onClick={(event) => event.stopPropagation()}
      >
        {frame ? (
          <div className="flex flex-col gap-2 min-w-0">
            <AvatarCropper
              frame={frame}
              busy={busy}
              onRegrab={grab}
              onCancel={() => setFrame(null)}
              onConfirm={(dataUrl, crop) => {
                applyAvatar(dataUrl, crop);
                setFrame(null);
              }}
            />
            {notice && <InlineNotice notice={notice} onClose={() => setNotice(null)} />}
          </div>
        ) : (
          <>
            {/* 卡片本体 */}
            <div
              className="holo-stage relative shrink-0 h-[min(78vh,640px)] flex items-center"
              style={cardVars}
            >
              {style.foil > 0.6 && <div className="holo-rays" />}
              <div
                key={flip}
                ref={cardRef}
                className="holo-card"
                title={still ? undefined : "点一下再翻一次"}
                onPointerEnter={onPointerMove}
                onPointerMove={onPointerMove}
                onPointerLeave={() => {
                  pointer.current = null;
                  wake.current?.();
                }}
                onClick={still ? undefined : () => setFlip((value) => value + 1)}
              >
                <div className="holo-card__tilt">
                  <div className="holo-card__face">
                    <canvas ref={canvasRef} />
                    <div className="holo-card__layer holo-card__foil" />
                    <div
                      className="holo-card__layer holo-card__spark"
                      style={{ backgroundImage: `url(${sparkleTexture()})` }}
                    />
                    <div className="holo-card__layer holo-card__glare" />
                    <div className="holo-card__layer holo-card__flash" />
                  </div>
                  <div className="holo-card__face holo-card__back">
                    <div className="holo-card__seal">枫</div>
                  </div>
                </div>
              </div>
            </div>

            {/* 右侧：这张卡是什么、怎么发出去 */}
            <div className="w-72 shrink-0 flex flex-col gap-4 self-center rounded-2xl border border-white/10 bg-slate-900 p-4 text-slate-100 shadow-2xl anim-pop">
              <div className="space-y-1.5">
                <div className="flex items-center gap-2">
                  <span
                    className="px-2 py-0.5 rounded-md text-sm font-extrabold italic text-slate-950"
                    style={{ background: style.accent }}
                  >
                    {flair.rarity}
                  </span>
                  <span className="text-lg font-extrabold">{flair.title}</span>
                </div>
                <p className="text-[11px] text-slate-400 leading-relaxed">{flair.reason}。</p>
              </div>

              <div className="space-y-2">
                <button
                  type="button"
                  disabled={busy}
                  onClick={copy}
                  className="w-full px-4 py-2.5 rounded-xl bg-amber-500 hover:bg-amber-400 disabled:opacity-50 text-sm font-bold text-slate-950 flex items-center justify-center gap-2 cursor-pointer"
                >
                  <Copy className="w-4 h-4" />
                  复制图片
                </button>
                <button
                  type="button"
                  disabled={busy}
                  onClick={save}
                  className="w-full px-4 py-2.5 rounded-xl bg-slate-800 hover:bg-slate-700 disabled:opacity-50 border border-white/10 text-sm font-bold text-slate-100 flex items-center justify-center gap-2 cursor-pointer"
                >
                  <Download className="w-4 h-4" />
                  保存为图片
                </button>
                <p className="text-[11px] text-slate-500 leading-relaxed">
                  发微信 / QQ：点「复制图片」，到聊天框里 Ctrl+V。保存的图在「图片\枫之助」。
                </p>
              </div>

              {/* 角色形象：自己框，不靠识别 */}
              <div className="rounded-xl border border-white/10 bg-slate-950/50 p-3 space-y-2">
                <div className="text-xs font-bold text-slate-200">角色形象</div>
                <div className="flex items-center gap-2">
                  {avatarUrl && (
                    <img
                      src={avatarUrl}
                      alt="当前用的角色形象"
                      className="w-16 rounded-md border border-white/10 bg-slate-950 object-contain [image-rendering:pixelated]"
                      style={{ aspectRatio: ART_ASPECT }}
                    />
                  )}
                  <div className="flex-1 min-w-0 flex flex-col gap-1.5">
                    <button
                      type="button"
                      disabled={busy}
                      onClick={grab}
                      className="px-2.5 py-1.5 rounded-lg bg-amber-500/20 hover:bg-amber-500/30 border border-amber-500/40 disabled:opacity-50 text-[11px] font-bold text-amber-200 flex items-center justify-center gap-1 cursor-pointer"
                    >
                      <Camera className="w-3.5 h-3.5" />
                      {avatarUrl ? "重新截取" : "从游戏画面截取"}
                    </button>
                    <button
                      type="button"
                      disabled={busy}
                      onClick={() => fileRef.current?.click()}
                      title="用一张现成的图：纸娃娃模拟器导出的透明 PNG、自己修过的截图都行"
                      className="px-2.5 py-1.5 rounded-lg bg-slate-800 hover:bg-slate-700 border border-white/10 disabled:opacity-50 text-[11px] text-slate-200 flex items-center justify-center gap-1 cursor-pointer"
                    >
                      <ImagePlus className="w-3.5 h-3.5" />
                      导入图片
                    </button>
                    <input
                      ref={fileRef}
                      type="file"
                      accept="image/png,image/jpeg,image/webp"
                      className="hidden"
                      onChange={(event) => {
                        const file = event.target.files?.[0];
                        // 清空选择：同一个文件改完再选一次，也要能触发
                        event.target.value = "";
                        if (file) importImage(file);
                      }}
                    />
                    {avatarUrl && (
                      <button
                        type="button"
                        disabled={busy}
                        onClick={() => {
                          storeAvatar(null);
                          setAvatarUrl(null);
                        }}
                        className="px-2.5 py-1 rounded-lg text-[11px] text-slate-400 hover:text-red-300 flex items-center justify-center gap-1 cursor-pointer"
                      >
                        <Trash2 className="w-3 h-3" />
                        不放形象
                      </button>
                    )}
                  </div>
                </div>
                <p className="text-[11px] text-slate-500 leading-relaxed">
                  {avatarUrl
                    ? "选过一次就一直用这张，换了装扮再重新来。"
                    : "游戏开着、角色在画面里时点「截取」，把框拖到角色身上就行。"}
                </p>
              </div>

              {notice && <InlineNotice notice={notice} onClose={() => setNotice(null)} />}
            </div>
          </>
        )}

        <button
          type="button"
          onClick={onClose}
          title="关闭（Esc）"
          className="absolute -top-3 -right-3 p-1.5 rounded-full bg-slate-800 hover:bg-slate-700 border border-white/10 text-slate-300 cursor-pointer"
        >
          <X className="w-4 h-4" />
        </button>
      </div>
    </div>,
    document.body,
  );
};

/** 把裁剪框限制在画面里（宽高比固定，高由宽推出来）。 */
function clampCrop(crop: AvatarCrop, frameAspect: number): AvatarCrop {
  // 框高占画面高的比例 = 宽比例 × 画面宽高比 ÷ 形象窗宽高比；不能超过 1
  const maxWidth = Math.min(1, ART_ASPECT / frameAspect);
  const width = Math.min(maxWidth, Math.max(CROP_MIN, crop.width));
  const height = (width * frameAspect) / ART_ASPECT;
  return {
    width,
    cx: Math.min(1 - width / 2, Math.max(width / 2, crop.cx)),
    cy: Math.min(1 - height / 2, Math.max(height / 2, crop.cy)),
  };
}

/**
 * 在一张游戏截图上框出角色。
 *
 * 为什么让用户自己框而不是自动找：角色不一定在画面正中（到地图边上镜头就不跟了），
 * 旁边还有别的玩家、宠物、召唤兽 —— 自动找错一次，发出去的卡上就是别人。
 * 框的位置按比例记住，下次打开框还在原来那里，多数时候点一下「用这个形象」就完事。
 *
 * 框出来的就是卡上的画面，**不去背景**：试过从四边漫水抠图，游戏里的背景多数太花，
 * 抠出来毛边、缺块，还不如一块干净的矩形画面（用户看过效果后定的）。
 */
const AvatarCropper: React.FC<{
  frame: CalibrationFrame;
  busy: boolean;
  onRegrab: () => void;
  onCancel: () => void;
  onConfirm: (dataUrl: string, crop: AvatarCrop) => void;
}> = ({ frame, busy, onRegrab, onCancel, onConfirm }) => {
  const frameAspect = frame.image_w / frame.image_h;
  const [crop, setCrop] = useState<AvatarCrop>(() => clampCrop(loadCrop(), frameAspect));
  const imageRef = useRef<HTMLImageElement>(null);
  /** 拖动中：按下时鼠标离框中心的偏移（比例） */
  const drag = useRef<{ dx: number; dy: number } | null>(null);
  const src = `data:image/png;base64,${frame.png_base64}`;
  const height = (crop.width * frameAspect) / ART_ASPECT;

  const pointAt = (event: React.PointerEvent | React.WheelEvent) => {
    const box = event.currentTarget.getBoundingClientRect();
    return {
      x: (event.clientX - box.left) / box.width,
      y: (event.clientY - box.top) / box.height,
    };
  };

  const onPointerDown = (event: React.PointerEvent<HTMLDivElement>) => {
    const point = pointAt(event);
    const inside =
      Math.abs(point.x - crop.cx) <= crop.width / 2 && Math.abs(point.y - crop.cy) <= height / 2;
    // 点在框外：框直接跳过来（比把框从老远拖过来快）
    drag.current = inside ? { dx: point.x - crop.cx, dy: point.y - crop.cy } : { dx: 0, dy: 0 };
    if (!inside) setCrop((prev) => clampCrop({ ...prev, cx: point.x, cy: point.y }, frameAspect));
    event.currentTarget.setPointerCapture(event.pointerId);
  };

  const onPointerMove = (event: React.PointerEvent<HTMLDivElement>) => {
    const offset = drag.current;
    if (!offset) return;
    const point = pointAt(event);
    setCrop((prev) =>
      clampCrop({ ...prev, cx: point.x - offset.dx, cy: point.y - offset.dy }, frameAspect),
    );
  };

  const resize = (width: number) =>
    setCrop((prev) => clampCrop({ ...prev, width }, frameAspect));

  const confirm = () => {
    const image = imageRef.current;
    if (!image || !image.naturalWidth) return;
    const sourceW = crop.width * image.naturalWidth;
    const sourceH = height * image.naturalHeight;
    const out = document.createElement("canvas");
    out.width = Math.max(1, Math.round(Math.min(AVATAR_MAX_SIDE, sourceW)));
    out.height = Math.max(1, Math.round(out.width / ART_ASPECT));
    const ctx = out.getContext("2d");
    if (!ctx) return;
    // 缩小存的时候才平滑；原样大小保持像素的硬边
    ctx.imageSmoothingEnabled = sourceW > out.width;
    ctx.drawImage(
      image,
      (crop.cx - crop.width / 2) * image.naturalWidth,
      (crop.cy - height / 2) * image.naturalHeight,
      sourceW,
      sourceH,
      0,
      0,
      out.width,
      out.height,
    );
    onConfirm(out.toDataURL("image/png"), crop);
  };

  return (
    <div className="flex flex-col gap-3 rounded-2xl border border-white/10 bg-slate-900 p-4 text-slate-100 shadow-2xl anim-pop">
      <div className="flex items-baseline gap-3">
        <h3 className="text-sm font-bold">把框拖到你的角色身上</h3>
        <span className="text-[11px] text-slate-400">
          拖动移位，滚轮或下面的滑块调大小；框里有什么，卡上就是什么
        </span>
      </div>
      <div
        className="relative w-fit mx-auto overflow-hidden rounded-xl border border-white/10 cursor-move select-none touch-none"
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={() => (drag.current = null)}
        onPointerCancel={() => (drag.current = null)}
        onWheel={(event) => resize(crop.width * (event.deltaY > 0 ? 1.08 : 1 / 1.08))}
      >
        <img
          ref={imageRef}
          src={src}
          alt="当前游戏画面"
          draggable={false}
          className="block max-w-[min(86vw,960px)] max-h-[62vh] [image-rendering:pixelated]"
        />
        {/* 框外压暗：一圈巨大的投影，比四块遮罩省事 */}
        <div
          className="absolute rounded-lg border-2 border-amber-400 pointer-events-none"
          style={{
            left: `${(crop.cx - crop.width / 2) * 100}%`,
            top: `${(crop.cy - height / 2) * 100}%`,
            width: `${crop.width * 100}%`,
            height: `${height * 100}%`,
            boxShadow: "0 0 0 9999px rgba(0,0,0,0.6)",
          }}
        />
      </div>
      <div className="flex items-center gap-3 flex-wrap">
        <label className="flex items-center gap-2 text-[11px] text-slate-400">
          取景大小
          <input
            type="range"
            min={CROP_MIN}
            max={Math.min(1, ART_ASPECT / frameAspect)}
            step={0.005}
            value={crop.width}
            onChange={(event) => resize(Number(event.target.value))}
            className="w-44 accent-amber-500"
          />
        </label>
        <button
          type="button"
          disabled={busy}
          onClick={onRegrab}
          title="角色被挡住了 / 姿势不好看 / 换了个地方：再抓一张"
          className="px-3 py-1.5 rounded-lg bg-slate-800 hover:bg-slate-700 disabled:opacity-50 border border-white/10 text-xs text-slate-200 flex items-center gap-1.5 cursor-pointer"
        >
          <RefreshCw className="w-3.5 h-3.5" />
          再抓一张
        </button>
        <div className="ml-auto flex gap-2">
          <button
            type="button"
            onClick={onCancel}
            className="px-3 py-1.5 rounded-lg bg-slate-800 hover:bg-slate-700 border border-white/10 text-xs text-slate-200 cursor-pointer"
          >
            取消
          </button>
          <button
            type="button"
            onClick={confirm}
            className="px-4 py-1.5 rounded-lg bg-amber-500 hover:bg-amber-400 text-xs font-bold text-slate-950 cursor-pointer"
          >
            用这个形象
          </button>
        </div>
      </div>
    </div>
  );
};
