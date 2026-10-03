import React, { useEffect } from "react";
import { createPortal } from "react-dom";
import { Coffee, X } from "lucide-react";
import tipQr from "../assets/tip-qr.jpg";

/**
 * 赞赏码弹窗。
 *
 * 入口只有「关于与更新说明」页里的一张卡片，**用户自己点才会出现**：不自动弹、不挂红点、
 * 不在别的页面提醒 —— 这是个免费工具，赞赏纯属自愿，不该让任何人觉得被催。
 * 文案也守这一条：只说「随缘」，不说「支持一下」「求打赏」之类的话。
 *
 * 挂到 body 上（portal）：页面容器带入场动画的 transform，`fixed` 放在它里面会被
 * 当成相对那个容器定位，弹窗就跑到页面底下去了。
 */
export const TipDialog: React.FC<{ onClose: () => void }> = ({ onClose }) => {
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);

  return createPortal(
    <div
      className="anim-fade fixed inset-0 z-[999] flex items-center justify-center bg-slate-950/60 p-4"
      onClick={onClose}
    >
      <div
        className="anim-pop w-full max-w-[320px] rounded-2xl border border-white/10 bg-slate-900 shadow-2xl overflow-hidden"
        onClick={(event) => event.stopPropagation()}
      >
        <div className="flex items-center justify-between gap-3 px-4 pt-3.5 pb-2.5">
          <div className="flex items-center gap-2 text-sm font-bold text-slate-100">
            <Coffee className="w-4 h-4 text-amber-400" />
            请作者喝杯咖灰
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
        <div className="px-4">
          <img
            src={tipQr}
            alt="微信赞赏码"
            draggable={false}
            className="block w-full rounded-xl border border-white/10"
          />
        </div>
        <p className="px-4 pt-2.5 pb-4 text-center text-[11px] leading-relaxed text-slate-400">
          微信扫一扫。随缘就好 —— 枫之助一直免费，
          <br />
          赞不赞赏用起来都一样。
        </p>
      </div>
    </div>,
    document.body,
  );
};
