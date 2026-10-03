import React from "react";
import { Sparkles } from "lucide-react";
import { RELEASES } from "../lib/changelog";
import { Card } from "./ui/card";

/**
 * 「更新说明」页：按版本倒序列出每次更新了什么。
 *
 * 数据在 `lib/changelog.ts`（发版时往最前面加一条）。
 * 这一页和启动时弹的那个窗看的是同一份数据 —— 一处维护，两处一致。
 */
export const UpdateNotes: React.FC = () => (
  <div className="space-y-4">
    {RELEASES.map((release, index) => (
      <Card key={release.version} className="p-5 space-y-3">
        <div className="flex items-baseline justify-between gap-3 flex-wrap">
          <div className="flex items-baseline gap-2">
            <span className="text-lg font-bold tabular-nums text-amber-400">
              v{release.version}
            </span>
            {index === 0 && (
              <span className="text-[10px] px-1.5 py-0.5 rounded-full bg-emerald-500/15 text-emerald-300 ring-1 ring-inset ring-emerald-500/30">
                当前版本
              </span>
            )}
          </div>
          <span className="text-xs tabular-nums text-slate-500">{release.date}</span>
        </div>

        <p className="text-xs text-slate-400">{release.summary}</p>

        <ul className="space-y-1.5">
          {release.items.map((item) => (
            <li key={item} className="flex items-start gap-2 text-[13px] text-slate-300">
              <Sparkles className="w-3.5 h-3.5 mt-0.5 shrink-0 text-amber-400/70" />
              <span>{item}</span>
            </li>
          ))}
        </ul>
      </Card>
    ))}
  </div>
);
