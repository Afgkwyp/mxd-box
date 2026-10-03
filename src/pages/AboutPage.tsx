import React, { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { UpdateNotes } from "../components/UpdateNotes";
import { TipDialog } from "../components/TipDialog";
import { Panel } from "../components/ui/kit";
import { APACHE_LICENSE, GITHUB_REPO, MXDC_HOME, QQ_GROUP_INVITE, QQ_GROUP_NUMBER } from "../lib/links";
import type { BuildInfo } from "../types";

const open = (url: string) => invoke("open_external_url", { url }).catch(() => {});

/**
 * 关于与更新说明：开源地址、数据来源、交流群、赞赏码、每一版更新了什么。
 * 原来是设置页里的第三个页签，现在单独成页（侧栏「设置」下面）。
 */
export const AboutPage: React.FC<{ build: BuildInfo | null }> = ({ build }) => {
  const [tipOpen, setTipOpen] = useState(false);

  return (
    <div className="space-y-4">
      <Panel title="枫之助 · 冒险岛怀旧服小助手" meta={build ? <span className="font-mono">v{build.version}</span> : undefined}>
        <div className="grid grid-cols-1 sm:grid-cols-2 xl:grid-cols-4 gap-3 text-xs">
          <button
            type="button"
            onClick={() => open(GITHUB_REPO)}
            className="text-left rounded-xl bg-slate-800/70 border border-white/5 p-3 hover:border-emerald-500/40 cursor-pointer"
          >
            <div className="font-bold text-slate-100 mb-0.5">
              已开源 <span className="font-mono text-emerald-400">MIT</span>
            </div>
            <div className="text-slate-400 break-all">
              源码在 GitHub：github.com/Afgkwyp/mxd-box（点击用系统浏览器打开，欢迎提 Issue）
            </div>
          </button>
          <button
            type="button"
            onClick={() => open(MXDC_HOME)}
            className="text-left rounded-xl bg-slate-800/70 border border-white/5 p-3 hover:border-amber-500/40 cursor-pointer"
          >
            <div className="font-bold text-slate-100 mb-0.5">数据来源：小册子</div>
            <div className="text-slate-400">物价、金价、掉落、开服监控都来自 mxdc.dvg.cn（点击用系统浏览器打开）</div>
          </button>
          <button
            type="button"
            onClick={() => open(QQ_GROUP_INVITE)}
            className="text-left rounded-xl bg-slate-800/70 border border-white/5 p-3 hover:border-sky-500/40 cursor-pointer"
          >
            <div className="font-bold text-slate-100 mb-0.5">
              玩家交流群 <span className="font-mono text-sky-400">{QQ_GROUP_NUMBER}</span>
            </div>
            <div className="text-slate-400">更新、问题反馈都发在群里（点击直接拉起 QQ 加群）</div>
          </button>
          {/* 赞赏：只在这里放一个入口，点了才出码（见 TipDialog 的说明） */}
          <button
            type="button"
            onClick={() => setTipOpen(true)}
            className="text-left rounded-xl bg-slate-800/70 border border-white/5 p-3 hover:border-amber-500/40 cursor-pointer"
          >
            <div className="font-bold text-slate-100 mb-0.5">请作者喝杯咖灰</div>
            <div className="text-slate-400">枫之助一直免费，赞不赞赏用起来都一样（点击看微信赞赏码）</div>
          </button>
        </div>
        {build && (
          <div className="mt-3 text-[11px] text-slate-500 font-mono break-all" title={build.exe_path}>
            构建于 {build.built_at} · {build.exe_path}
          </div>
        )}
        {/* 开源组件署名：随包分发了别人的模型和库，出处要写明（Apache-2.0 等许可的要求） */}
        <p className="mt-2 text-[11px] leading-relaxed text-slate-500">
          开源组件：地图名识别使用 PaddleOCR 的 PP-OCR 识别模型（Apache-2.0）；程序基于 Tauri、React、
          tract、Recharts、Lucide 等开源项目构建（MIT / Apache-2.0 / ISC）。
          经验统计里定位游戏 HUD 的思路和模板数据借鉴了网页版枫记（fj.need.run），本项目与枫记无隶属关系。
          <button
            type="button"
            onClick={() => open(APACHE_LICENSE)}
            className="ml-1 underline underline-offset-2 hover:text-slate-300 cursor-pointer"
          >
            Apache-2.0 许可全文
          </button>
        </p>
      </Panel>
      <UpdateNotes />
      {tipOpen && <TipDialog onClose={() => setTipOpen(false)} />}
    </div>
  );
};
