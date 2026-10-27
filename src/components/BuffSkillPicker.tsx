import React, { useEffect, useMemo, useState } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { Crop, Search, Sparkles, X } from "lucide-react";
import { cn } from "../lib/utils";
import type { BuffConfig, BuffSkill } from "../types";

const GROUPS = ["全部", "冒险家", "骑士团", "英雄职业", "通用"] as const;

/**
 * 「添加 BUFF」：从内置的技能图标里挑（小册子技能图鉴里的 BUFF 类技能，打包在程序里）。
 *
 * 图鉴里没有的（药水、道具给的 BUFF）点底下的「在截图上框」，走 `BuffCropDialog`。
 */
export const BuffSkillPicker: React.FC<{
  /** 已经加过的技能 ID（标成「已添加」，不让重复加） */
  added: string[];
  onClose: () => void;
  onAdded: (config: BuffConfig) => void;
  onCrop: () => void;
}> = ({ added, onClose, onAdded, onCrop }) => {
  const [skills, setSkills] = useState<BuffSkill[] | null>(null);
  const [query, setQuery] = useState("");
  const [group, setGroup] = useState<(typeof GROUPS)[number]>("全部");
  const [error, setError] = useState("");
  const [saving, setSaving] = useState<string | null>(null);

  useEffect(() => {
    invoke<BuffSkill[]>("list_buff_skills")
      .then(setSkills)
      .catch((err: unknown) => setError(String(err)));
  }, []);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);

  const shown = useMemo(() => {
    const q = query.trim();
    return (skills ?? []).filter(
      (s) => (group === "全部" || s.group === group) && (!q || s.name.includes(q) || s.job.includes(q)),
    );
  }, [skills, query, group]);

  const pick = (skill: BuffSkill) => {
    if (saving || added.includes(skill.id)) return;
    setSaving(skill.id);
    setError("");
    invoke<BuffConfig>("add_buff_from_skill", { skillId: skill.id })
      .then((config) => {
        onAdded(config);
        onClose();
      })
      .catch((err: unknown) => setError(String(err)))
      .finally(() => setSaving(null));
  };

  return createPortal(
    <div className="anim-fade fixed inset-0 z-[999] flex items-center justify-center bg-slate-950/70 p-4">
      <div className="anim-pop w-full max-w-[720px] max-h-full flex flex-col rounded-2xl border border-white/10 bg-slate-900 shadow-2xl overflow-hidden">
        <div className="flex items-center justify-between gap-3 px-4 pt-3.5 pb-2.5">
          <div className="flex items-center gap-2 text-sm font-bold text-slate-100">
            <Sparkles className="w-4 h-4 text-amber-400" />
            添加 BUFF：挑一个技能
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

        <div className="flex flex-wrap items-center gap-2 px-4 pb-3 text-xs">
          <label className="flex items-center gap-1.5 flex-1 min-w-[180px] bg-slate-950/60 border border-white/10 rounded-lg px-2.5 py-1.5 focus-within:border-amber-500/40">
            <Search className="w-3.5 h-3.5 text-slate-500" />
            <input
              autoFocus
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder="技能名或职业，比如：祝福、牧师"
              className="flex-1 bg-transparent outline-none text-slate-100 placeholder:text-slate-600"
            />
          </label>
          {GROUPS.map((g) => (
            <button
              key={g}
              type="button"
              onClick={() => setGroup(g)}
              className={cn(
                "px-2 py-1 rounded-md border cursor-pointer",
                g === group
                  ? "bg-amber-500 text-slate-950 border-amber-500"
                  : "border-white/10 text-slate-300 hover:border-amber-500/40",
              )}
            >
              {g}
            </button>
          ))}
        </div>

        <div className="mx-4 rounded-xl border border-white/10 bg-slate-950/40 overflow-auto min-h-[160px] max-h-[55vh] p-2">
          {skills == null ? (
            <div className="p-6 text-xs text-slate-500">{error ? "" : "正在读取…"}</div>
          ) : shown.length === 0 ? (
            <div className="p-6 text-xs text-slate-500">没有对得上的技能 —— 换个词，或者用下面的「在截图上框」</div>
          ) : (
            <div className="grid grid-cols-[repeat(auto-fill,minmax(150px,1fr))] gap-1.5">
              {shown.map((skill) => {
                const done = added.includes(skill.id);
                return (
                  <button
                    key={skill.id}
                    type="button"
                    onClick={() => pick(skill)}
                    disabled={done || saving != null}
                    title={skill.job}
                    className={cn(
                      "flex items-center gap-2 rounded-lg border px-2 py-1.5 text-left cursor-pointer disabled:cursor-default",
                      done
                        ? "border-white/5 opacity-50"
                        : "border-white/5 bg-slate-800/60 hover:border-amber-500/50",
                    )}
                  >
                    <img
                      src={`data:image/png;base64,${skill.png}`}
                      alt=""
                      className="w-8 h-8 shrink-0"
                      style={{ imageRendering: "pixelated" }}
                    />
                    <span className="min-w-0">
                      <span className="block truncate text-xs font-semibold text-slate-100">{skill.name}</span>
                      <span className="block truncate text-[10px] text-slate-500">
                        {done ? "已添加" : saving === skill.id ? "添加中…" : skill.job}
                      </span>
                    </span>
                  </button>
                );
              })}
            </div>
          )}
        </div>

        <div className="flex flex-wrap items-center gap-3 px-4 py-3 text-[11px] text-slate-500">
          <span>图标来自小册子「技能图鉴」，已打包在程序里。</span>
          <button
            type="button"
            onClick={onCrop}
            className="ml-auto inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-slate-800 border border-white/10 text-xs text-slate-200 hover:border-amber-500/40 cursor-pointer"
          >
            <Crop className="w-3.5 h-3.5" />
            图鉴里没有？在截图上框
          </button>
        </div>

        {error && <div className="px-4 pb-3 text-[11px] text-red-300">{error}</div>}
      </div>
    </div>,
    document.body,
  );
};
