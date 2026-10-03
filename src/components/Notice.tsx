import React from "react";
import { AlertTriangle, CheckCircle2, Info, X } from "lucide-react";

/**
 * 界面里自己画的提示条与确认框。
 *
 * 为什么不用浏览器原生的 `alert()` / `confirm()`：原生弹窗是**另一套视觉**，
 * 和这个深色界面完全不搭；而且它是阻塞的、必须点掉才能继续，连着两次失败
 * 就是两个必须挨个确认的弹窗。更要紧的是它挂不住状态 —— 排查时用户截图过来，
 * 上面的错误信息早就关掉了。
 *
 * 所以失败/成功信息用页内的提示条（可关闭、留在原地），危险操作用自绘确认框。
 */

export type NoticeKind = "error" | "success" | "info";

const NOTICE_STYLE: Record<NoticeKind, string> = {
  error: "bg-red-500/10 border-red-500/40 text-red-200",
  success: "bg-emerald-500/10 border-emerald-500/40 text-emerald-200",
  info: "bg-sky-500/10 border-sky-500/40 text-sky-200",
};

const NOTICE_ICON: Record<NoticeKind, React.ReactNode> = {
  error: <AlertTriangle className="w-4 h-4 shrink-0" />,
  success: <CheckCircle2 className="w-4 h-4 shrink-0" />,
  info: <Info className="w-4 h-4 shrink-0" />,
};

export interface Notice {
  kind: NoticeKind;
  message: string;
}

export const InlineNotice: React.FC<{
  notice: Notice | null;
  onClose?: () => void;
  className?: string;
}> = ({ notice, onClose, className = "" }) => {
  if (!notice) return null;

  return (
    <div
      role={notice.kind === "error" ? "alert" : "status"}
      className={`flex items-start gap-2 px-3 py-2 rounded-lg border text-xs leading-relaxed ${NOTICE_STYLE[notice.kind]} ${className}`}
    >
      {NOTICE_ICON[notice.kind]}
      <span className="flex-1 whitespace-pre-wrap break-words">{notice.message}</span>
      {onClose && (
        <button
          type="button"
          onClick={onClose}
          title="关掉这条提示"
          className="shrink-0 opacity-60 hover:opacity-100 cursor-pointer"
        >
          <X className="w-3.5 h-3.5" />
        </button>
      )}
    </div>
  );
};

/**
 * 危险操作的确认框（删除之类）。
 *
 * `confirm()` 的按钮文案是浏览器定的（「确定 / 取消」），说不清要删的是什么；
 * 这里把对象和后果写进正文，按钮上也直接写动作。
 */
export const ConfirmDialog: React.FC<{
  open: boolean;
  title: string;
  body?: React.ReactNode;
  confirmLabel?: string;
  onConfirm: () => void;
  onCancel: () => void;
}> = ({ open, title, body, confirmLabel = "确认", onConfirm, onCancel }) => {
  if (!open) return null;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/60"
      onClick={onCancel}
    >
      <div
        role="dialog"
        aria-modal="true"
        className="w-full max-w-sm rounded-xl border border-white/10 bg-slate-900 shadow-2xl p-5 space-y-4 text-slate-100"
        onClick={(event) => event.stopPropagation()}
      >
        <div className="flex items-start gap-2">
          <AlertTriangle className="w-4 h-4 mt-0.5 text-amber-400 shrink-0" />
          <h3 className="text-sm font-bold">{title}</h3>
        </div>

        {body && <div className="text-xs text-slate-300 leading-relaxed">{body}</div>}

        <div className="flex justify-end gap-2 pt-1">
          <button
            type="button"
            onClick={onCancel}
            className="px-3 py-1.5 rounded-lg bg-slate-800 hover:bg-slate-700 border border-white/10 text-xs text-slate-200 cursor-pointer"
          >
            取消
          </button>
          <button
            type="button"
            onClick={onConfirm}
            className="px-3 py-1.5 rounded-lg bg-red-500 hover:bg-red-400 text-xs font-bold text-white cursor-pointer"
          >
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
};
