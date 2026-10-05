import React, { useState } from "react";
import { Check, Pencil, Trash2, Users, X } from "lucide-react";
import { Panel } from "./ui/kit";
import { cn } from "../lib/utils";
import { formatDuration, formatNumber, formatShortDate } from "../lib/format";
import type { ExpCharacter } from "../types";

/**
 * 角色列表：名字、职业、等级、经验进度，和各自名下的历史汇总。
 *
 * 角色是程序自己从状态栏上认的（`LV.` 右边那两行字，见 `exp::character`）：
 * 认的是名字那一块的像素，名字和职业是读出来给人看的 —— 所以名字偶尔会认错一个字，
 * 这里给了改名；改过之后程序不会再拿读出来的名字盖回去。
 *
 * 点一个角色 = 下面的历史只看它的；再点一次回到全部。
 */
export const ExpCharacters: React.FC<{
  characters: ExpCharacter[];
  /** 现在登录的那个（没认出来是 null） */
  currentId: number | null;
  /** 历史正在只看谁（null = 全部） */
  selectedId: number | null;
  busy: boolean;
  onSelect: (id: number | null) => void;
  onRename: (id: number, name: string) => void;
  onDelete: (character: ExpCharacter) => void;
}> = ({ characters, currentId, selectedId, busy, onSelect, onRename, onDelete }) => {
  const [editing, setEditing] = useState<{ id: number; name: string } | null>(null);

  const commit = () => {
    if (!editing) return;
    const name = editing.name.trim();
    if (name) onRename(editing.id, name);
    setEditing(null);
  };

  return (
    <Panel
      className="col-span-12"
      title={
        <>
          <Users className="w-4 h-4 text-amber-400" />
          角色
        </>
      }
      meta={
        characters.length > 0 && (
          <span>点一个角色，下面的历史只看它的</span>
        )
      }
    >
      {characters.length === 0 ? (
        <p className="text-xs text-slate-500">
          还没认到角色。进游戏、状态栏能看到的时候会自动记下来，不用手动添加。
        </p>
      ) : (
        <ul className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-2">
          {characters.map((character) => {
            const selected = character.id === selectedId;
            const current = character.id === currentId;
            const percent = Math.max(0, Math.min(100, character.percent ?? 0));
            const renaming = editing?.id === character.id;
            return (
              <li
                key={character.id}
                className={cn(
                  "group rounded-xl border bg-slate-950/40 px-3 py-2.5 transition-colors",
                  selected
                    ? "border-amber-500/60 bg-amber-500/5"
                    : "border-white/10 hover:border-white/20",
                )}
              >
                <div className="flex items-center gap-2 min-w-0">
                  {renaming ? (
                    <form
                      className="flex items-center gap-1 flex-1 min-w-0"
                      onSubmit={(event) => {
                        event.preventDefault();
                        commit();
                      }}
                    >
                      <input
                        autoFocus
                        value={editing.name}
                        maxLength={24}
                        aria-label="角色名"
                        onChange={(event) =>
                          setEditing({ id: character.id, name: event.target.value })
                        }
                        onKeyDown={(event) => {
                          if (event.key === "Escape") setEditing(null);
                        }}
                        className="flex-1 min-w-0 bg-slate-900 border border-white/15 rounded px-2 py-0.5 text-sm text-slate-100 outline-none focus:border-amber-500/60"
                      />
                      <button
                        type="submit"
                        title="保存"
                        className="p-1 rounded text-emerald-300 hover:bg-white/10 cursor-pointer"
                      >
                        <Check className="w-3.5 h-3.5" />
                      </button>
                      <button
                        type="button"
                        title="取消"
                        onClick={() => setEditing(null)}
                        className="p-1 rounded text-slate-400 hover:bg-white/10 cursor-pointer"
                      >
                        <X className="w-3.5 h-3.5" />
                      </button>
                    </form>
                  ) : (
                    <>
                      <button
                        type="button"
                        aria-pressed={selected}
                        onClick={() => onSelect(selected ? null : character.id)}
                        title={selected ? "回到全部角色的历史" : "历史只看这个角色的"}
                        className="flex items-baseline gap-2 flex-1 min-w-0 text-left cursor-pointer"
                      >
                        <span className="font-bold text-sm text-slate-100 truncate">
                          {character.name}
                        </span>
                        {character.job && (
                          <span className="text-[11px] text-slate-400 shrink-0">
                            {character.job}
                          </span>
                        )}
                      </button>
                      {current && (
                        <span className="shrink-0 text-[10px] text-emerald-200 bg-emerald-500/10 border border-emerald-500/25 rounded px-1.5 py-0.5">
                          在线
                        </span>
                      )}
                      <button
                        type="button"
                        disabled={busy}
                        onClick={() => setEditing({ id: character.id, name: character.name })}
                        title="改名（名字是程序读出来的，认错了字可以自己改）"
                        className="p-1 rounded text-slate-500 hover:text-amber-300 hover:bg-white/10 disabled:opacity-40 cursor-pointer"
                      >
                        <Pencil className="w-3 h-3" />
                      </button>
                      <button
                        type="button"
                        disabled={busy}
                        onClick={() => onDelete(character)}
                        title="从列表里拿掉这个角色"
                        className="p-1 rounded text-slate-500 hover:text-red-300 hover:bg-white/10 disabled:opacity-40 cursor-pointer"
                      >
                        <Trash2 className="w-3 h-3" />
                      </button>
                    </>
                  )}
                </div>

                <div className="mt-2 flex items-center gap-2 text-xs tabular-nums">
                  <span className="text-slate-200 shrink-0">
                    {character.level != null ? `Lv.${character.level}` : "Lv.—"}
                  </span>
                  <div
                    role="progressbar"
                    aria-label={`${character.name} 的经验进度`}
                    aria-valuemin={0}
                    aria-valuemax={100}
                    aria-valuenow={Math.round(percent)}
                    className="flex-1 h-1.5 rounded-full bg-slate-800 overflow-hidden"
                  >
                    <div
                      className="h-full rounded-full bg-amber-400"
                      style={{ width: `${percent}%` }}
                    />
                  </div>
                  <span className="text-slate-400 shrink-0 w-14 text-right">
                    {character.percent != null ? `${character.percent.toFixed(2)}%` : "—"}
                  </span>
                </div>

                <p className="mt-1.5 text-[11px] text-slate-500 tabular-nums truncate">
                  {character.sessions > 0
                    ? `${character.sessions} 段 · ${formatDuration(character.active_secs)} · +${formatNumber(character.gained_exp)}`
                    : "还没有计入历史的记录"}
                  {" · "}
                  {current ? "现在" : `上次 ${formatShortDate(character.last_seen_unix)}`}
                </p>
              </li>
            );
          })}
        </ul>
      )}
    </Panel>
  );
};
