import React, { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { Crop, RefreshCw, X } from "lucide-react";
import { cn } from "../lib/utils";
import type { BuffConfig, BuffSnapshot } from "../types";

interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
}

/**
 * 「添加 BUFF」：截一张游戏画面顶部（BUFF 栏那一条），在上面拖一个框框住要盯的那个图标。
 *
 * 框下来的就是模板，所以：
 * * 先在游戏里把这个 BUFF 放出来，图标亮着的时候截；
 * * **只框图标本身**（游戏里一般 32×32），别带上旁边的图标或空白 —— 读秒数时按框的
 *   位置找左下角的数字，框歪了数字就找不准。
 *
 * 截图按原生像素显示（可放大到 2 / 3 倍），框的坐标也是原生像素，直接交给后端裁。
 */
export const BuffCropDialog: React.FC<{
  onClose: () => void;
  onAdded: (config: BuffConfig) => void;
}> = ({ onClose, onAdded }) => {
  const [snapshot, setSnapshot] = useState<BuffSnapshot | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [zoom, setZoom] = useState(2);
  const [box, setBox] = useState<Box | null>(null);
  const [name, setName] = useState("");
  const [saving, setSaving] = useState(false);
  const drag = useRef<{ x: number; y: number } | null>(null);
  const imageRef = useRef<HTMLImageElement>(null);

  const capture = () => {
    setLoading(true);
    setError("");
    setBox(null);
    invoke<BuffSnapshot>("buff_capture_frame")
      .then(setSnapshot)
      .catch((err: unknown) => setError(String(err)))
      .finally(() => setLoading(false));
  };

  useEffect(capture, []);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);

  /** 鼠标位置 → 截图上的原生像素。 */
  const pointAt = (event: React.PointerEvent) => {
    const rect = imageRef.current!.getBoundingClientRect();
    const clamp = (v: number, max: number) => Math.max(0, Math.min(max, v));
    return {
      x: clamp(Math.floor((event.clientX - rect.left) / zoom), snapshot!.width),
      y: clamp(Math.floor((event.clientY - rect.top) / zoom), snapshot!.height),
    };
  };

  const onPointerDown = (event: React.PointerEvent) => {
    if (!snapshot) return;
    event.currentTarget.setPointerCapture(event.pointerId);
    const p = pointAt(event);
    drag.current = p;
    setBox({ x: p.x, y: p.y, w: 0, h: 0 });
  };

  const onPointerMove = (event: React.PointerEvent) => {
    if (!drag.current || !snapshot) return;
    const p = pointAt(event);
    const start = drag.current;
    setBox({
      x: Math.min(start.x, p.x),
      y: Math.min(start.y, p.y),
      w: Math.abs(p.x - start.x),
      h: Math.abs(p.y - start.y),
    });
  };

  const onPointerUp = () => {
    drag.current = null;
  };

  const sizeOk = box != null && box.w >= 12 && box.h >= 12 && box.w <= 128 && box.h <= 128;
  const canSave = sizeOk && name.trim() !== "" && !saving;

  const save = () => {
    if (!box || !canSave) return;
    setSaving(true);
    setError("");
    invoke<BuffConfig>("add_buff_from_frame", {
      name: name.trim(),
      x: box.x,
      y: box.y,
      width: box.w,
      height: box.h,
    })
      .then((config) => {
        onAdded(config);
        onClose();
      })
      .catch((err: unknown) => setError(String(err)))
      .finally(() => setSaving(false));
  };

  return createPortal(
    <div className="anim-fade fixed inset-0 z-[999] flex items-center justify-center bg-slate-950/70 p-4">
      <div className="anim-pop w-full max-w-[960px] max-h-full flex flex-col rounded-2xl border border-white/10 bg-slate-900 shadow-2xl overflow-hidden">
        <div className="flex items-center justify-between gap-3 px-4 pt-3.5 pb-2.5">
          <div className="flex items-center gap-2 text-sm font-bold text-slate-100">
            <Crop className="w-4 h-4 text-amber-400" />
            添加 BUFF：在截图上框住它的图标
          </div>
          <button
            type="button"
            onClick={onClose}
            title="关闭 (Esc)"
            aria-label="关闭"
            className="p-1 rounded-lg text-slate-400 hover:text-slate-100 hover:bg-slate-800 cursor-pointer"
          >
            <X className="w-4 h-4" />
          </button>
        </div>

        <p className="px-4 pb-2 text-[11px] text-slate-400 leading-relaxed">
          先在游戏里把这个 BUFF 放出来，再点「重新截图」。拖一个框<b className="text-slate-200">只框住图标本身</b>
          （一般是个小方块），别带上旁边的图标 —— 秒数是按框的位置去图标左下角找的。
        </p>

        <div className="flex items-center gap-2 px-4 pb-2 text-xs">
          <button
            type="button"
            onClick={capture}
            disabled={loading}
            className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-slate-800 border border-white/10 text-slate-200 hover:border-amber-500/40 cursor-pointer disabled:opacity-50"
          >
            <RefreshCw className={cn("w-3.5 h-3.5", loading && "animate-spin")} />
            重新截图
          </button>
          <span className="text-slate-500 ml-2">放大</span>
          {[1, 2, 3].map((z) => (
            <button
              key={z}
              type="button"
              onClick={() => setZoom(z)}
              className={cn(
                "px-2 py-1 rounded-md border cursor-pointer tabular-nums",
                z === zoom
                  ? "bg-amber-500 text-slate-950 border-amber-500"
                  : "border-white/10 text-slate-300 hover:border-amber-500/40",
              )}
            >
              {z}×
            </button>
          ))}
          {box && (
            <span className={cn("ml-auto tabular-nums", sizeOk ? "text-slate-400" : "text-red-300")}>
              框：{box.w}×{box.h}
              {!sizeOk && box.w + box.h > 0 && "（要在 12–128 像素之间）"}
            </span>
          )}
        </div>

        <div className="mx-4 rounded-xl border border-white/10 bg-black overflow-auto min-h-[120px] max-h-[55vh]">
          {snapshot ? (
            <div
              className="relative select-none cursor-crosshair"
              style={{ width: snapshot.width * zoom, height: snapshot.height * zoom }}
              onPointerDown={onPointerDown}
              onPointerMove={onPointerMove}
              onPointerUp={onPointerUp}
            >
              <img
                ref={imageRef}
                src={`data:image/png;base64,${snapshot.png_base64}`}
                alt="游戏画面顶部"
                draggable={false}
                className="block"
                style={{
                  width: snapshot.width * zoom,
                  height: snapshot.height * zoom,
                  imageRendering: "pixelated",
                }}
              />
              {box && box.w > 0 && box.h > 0 && (
                <div
                  className="absolute border-2 border-amber-400 bg-amber-400/10 pointer-events-none"
                  style={{ left: box.x * zoom, top: box.y * zoom, width: box.w * zoom, height: box.h * zoom }}
                />
              )}
            </div>
          ) : (
            <div className="p-6 text-xs text-slate-500">{loading ? "正在截图…" : "还没有截图"}</div>
          )}
        </div>

        <div className="flex flex-wrap items-center gap-3 px-4 py-3">
          {snapshot && box && sizeOk && (
            <canvas
              aria-label="框住的图标"
              className="rounded border border-white/10"
              style={{ width: 48, height: 48, imageRendering: "pixelated" }}
              ref={(canvas) => {
                if (!canvas || !imageRef.current) return;
                canvas.width = box.w;
                canvas.height = box.h;
                canvas.getContext("2d")?.drawImage(imageRef.current, box.x, box.y, box.w, box.h, 0, 0, box.w, box.h);
              }}
            />
          )}
          <label className="flex items-center gap-2 text-xs text-slate-300">
            名字
            <input
              value={name}
              maxLength={20}
              onChange={(event) => setName(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter" && !event.nativeEvent.isComposing) save();
              }}
              placeholder="比如：圣盾"
              className="w-40 bg-slate-950/60 border border-white/10 rounded-lg px-2.5 py-1.5 text-xs text-slate-100 placeholder:text-slate-600 focus:border-amber-500/40 outline-none"
            />
          </label>
          <span className="text-[11px] text-slate-500">提醒时会念「{name.trim() || "名字"}即将结束」</span>
          <button
            type="button"
            onClick={save}
            disabled={!canSave}
            className="ml-auto px-4 py-1.5 rounded-lg bg-amber-500 text-slate-950 text-xs font-bold cursor-pointer disabled:opacity-40 disabled:cursor-default"
          >
            {saving ? "添加中…" : "添加"}
          </button>
        </div>

        {error && <div className="px-4 pb-3 text-[11px] text-red-300">{error}</div>}
      </div>
    </div>,
    document.body,
  );
};
