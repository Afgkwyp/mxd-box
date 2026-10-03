import React, { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  ShieldAlert,
  ShieldCheck,
  Plus,
  Trash2,
  Search,
  AlertTriangle,
  Check,
  Clipboard,
  UserX,
  UserSearch,
  Info,
} from "lucide-react";
import type { BlacklistEntry, BlacklistVerdict } from "../types";
import { usePresetServer } from "../lib/presetServer";
import { useHotkey } from "../lib/hotkeys";
import { ConfirmDialog, InlineNotice, type Notice } from "./Notice";

/** 四档结论的配色与图标，一个地方定，避免各写一份。 */
const VERDICT_STYLE: Record<
  BlacklistVerdict["status"],
  { tone: string; label: string; hint: string }
> = {
  hit: {
    tone: "bg-red-950/60 border-red-500/60 text-red-200",
    label: "确定命中：同名 + 同服",
    hint: "交易请走担保，或直接不做这笔。",
  },
  other_server: {
    tone: "bg-amber-950/50 border-amber-500/50 text-amber-200",
    label: "注意：只在别的服被记过",
    hint: "同名很常见，不能就此认定是同一人。",
  },
  similar: {
    tone: "bg-amber-950/40 border-amber-500/40 text-amber-200",
    label: "疑似：有名字很接近的记录",
    hint: "可能是小号式改名，对照一下区服和原因。",
  },
  clear: {
    tone: "bg-emerald-950/40 border-emerald-500/40 text-emerald-200",
    label: "本地库没有记录",
    hint: "只说明你自己的库里没有，不代表他干净。",
  },
};

/**
 * 避坑黑名单库。
 *
 * 这次重做的重点不在表格，而在**「什么时候用得上」**：
 *
 * 1. 顶部加了「查这个人」—— 想查一个名字时，不再需要先翻表格再肉眼比对；
 * 2. 剪贴板嗅探搬到了 Rust 侧（开关存在设置里），**与这个页面开不开无关**：
 *    复制的名字看起来像角色名时自动比对，确定命中就响铃 + 右下角弹提醒窗。
 *    原来那版在前端定时读剪贴板，只有停在这一页才跑，而你要查人的时候
 *    恰恰是在游戏里交易 —— 也就是最不可能停在这一页的时候。
 * 3. 结论分四档而不是「安全 / 危险」：游戏里同名太常见，二值判断要么误伤好人，
 *    要么反过来给骗子背书。
 */
export const BlacklistManager: React.FC = () => {
  const { serverId, servers } = usePresetServer();
  /**
   * 查价悬浮窗的快捷键要按**实际在用**的键显示。
   *
   * 这里以前写死了「按 Alt+F」—— 用户把键改成别的之后，这行提示就在教一个按了
   * 没反应的键。凡是界面上提到快捷键的地方都必须读实时值（见 lib/hotkeys.ts）。
   */
  const priceHudKey = useHotkey("price_hud", "Alt+F");
  const serverName = servers.find((server) => server.id === serverId)?.name ?? "";

  const [list, setList] = useState<BlacklistEntry[]>([]);
  const [loading, setLoading] = useState(false);
  const [searchFilter, setSearchFilter] = useState("");

  // 「查这个人」
  const [query, setQuery] = useState("");
  const [verdict, setVerdict] = useState<BlacklistVerdict | null>(null);
  const [checking, setChecking] = useState(false);
  const [checkError, setCheckError] = useState("");

  // 剪贴板嗅探（开关在 Rust 侧，这里只是它的镜像）
  const [watch, setWatch] = useState(false);
  /** 嗅探命中时由 Rust 推过来，页面开着就能立刻看到细节 */
  const [hit, setHit] = useState<BlacklistVerdict | null>(null);

  // Add modal state
  const [showAddModal, setShowAddModal] = useState(false);
  const [newServer, setNewServer] = useState("");
  const [newName, setNewName] = useState("");
  const [newCategory, setNewCategory] = useState("骗子");
  const [newReason, setNewReason] = useState("");

  const [actionNotice, setActionNotice] = useState("");
  /** 失败信息用页内提示条，不用原生 `alert()`（它的视觉、阻塞行为和这个界面都不搭） */
  const [notice, setNotice] = useState<Notice | null>(null);
  /** 待删除的记录：先问一句再删，而不是 `confirm()` 一个说不清对象的弹窗 */
  const [pendingDelete, setPendingDelete] = useState<BlacklistEntry | null>(null);

  const fetchList = useCallback(async () => {
    setLoading(true);
    try {
      const data = await invoke<BlacklistEntry[]>("get_blacklist");
      setList(data);
    } catch {
      // ignore
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchList();

    invoke<{ blacklist_watch: boolean }>("get_settings")
      .then((settings) => setWatch(settings.blacklist_watch))
      .catch(() => {});

    // 嗅探命中：Rust 那边弹提醒窗，这里挂一条横幅（页面开着时顺手看细节）
    const unlisten = listen<BlacklistVerdict>("blacklist-hit", (event) => {
      setHit(event.payload);
      fetchList();
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, [fetchList]);


  const handleCheck = async (name?: string) => {
    const target = (name ?? query).trim();
    if (!target) return setCheckError("先输入要查的角色名");
    setChecking(true);
    setCheckError("");
    try {
      const result = await invoke<BlacklistVerdict>("check_blacklist", { name: target });
      setVerdict(result);
      setQuery(target);
    } catch (err: unknown) {
      setCheckError(typeof err === "string" ? err : "比对失败");
    } finally {
      setChecking(false);
    }
  };

  const toggleWatch = async () => {
    const next = !watch;
    setWatch(next);
    try {
      await invoke("set_blacklist_watch", { enabled: next });
      setActionNotice(
        next
          ? "剪贴板嗅探已开启：复制到像角色名的文本时会自动比对，命中就提醒你（关掉这个页面也生效）。"
          : "剪贴板嗅探已关闭。"
      );
      setTimeout(() => setActionNotice(""), 5000);
    } catch (err: unknown) {
      setWatch(!next);
      setNotice({ kind: "error", message: `保存失败：${err}` });
    }
  };

  const handleAdd = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!newName.trim()) return;

    const entry: BlacklistEntry = {
      server: newServer || serverName || "全服",
      player_name: newName.trim(),
      category: newCategory,
      reason: newReason.trim() || "无详细原因记录",
      created_at: new Date().toLocaleDateString(),
    };

    try {
      await invoke("add_blacklist", { entry });
      setShowAddModal(false);
      setNewName("");
      setNewReason("");
      fetchList();
      setActionNotice("已录入（同名同服会更新原有记录，不会多出重复行）。");
      setTimeout(() => setActionNotice(""), 4000);
    } catch (err: unknown) {
      setNotice({ kind: "error", message: `添加失败：${err}` });
    }
  };

  const handleDelete = async (id?: number) => {
    if (!id) return;
    try {
      await invoke("remove_blacklist", { id });
      fetchList();
    } catch (err: unknown) {
      setNotice({ kind: "error", message: `删除失败：${err}` });
    }
  };

  const filtered = list.filter((item) => {
    const q = searchFilter.toLowerCase();
    return (
      item.player_name.toLowerCase().includes(q) ||
      item.server.toLowerCase().includes(q) ||
      item.category.toLowerCase().includes(q) ||
      item.reason.toLowerCase().includes(q)
    );
  });

  const style = verdict ? VERDICT_STYLE[verdict.status] : null;

  return (
    <div className="space-y-5">
      {/* Header Bar */}
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-4 p-4 rounded-2xl bg-slate-900/70 border border-white/10">
        <div>
          <span className="text-[11px] px-2 py-0.5 rounded-full bg-slate-800 text-slate-300 border border-white/10">
            本地存储 · 共 {list.length} 条
          </span>
          <p className="text-xs text-slate-400 mt-2">记录骗子、抢怪、跑单、黑金惯犯</p>
        </div>

        {/*
          这里原本有「导出 JSON / 导入 JSON」两个按钮（分享黑名单给好友或公会）。
          用户这轮要求先撤掉，后期再评估要不要加回来 —— 所以**只删界面入口**，
          Rust 侧的 `export_blacklist` / `import_blacklist` 命令和它们的单测都留着，
          以后要恢复时不用重写逻辑（但那时要把「别人发来的名单能不能信」想清楚：
          黑名单是照着名字比对的，别人塞进来的名单可以直接毁掉一个人的风评）。
        */}
        <div className="flex flex-wrap items-center gap-2.5">
          {/* Add */}
          <button
            onClick={() => {
              // 默认区服跟着全局预设走（以前这里写死蘑菇仔，而且选项里还漏了蓝蜗牛）
              setNewServer(serverName || servers[0]?.name || "全服");
              setShowAddModal(true);
            }}
            className="px-3.5 py-1.5 rounded-lg bg-red-500/20 hover:bg-red-500/30 border border-red-500/40 text-xs font-bold text-red-300 flex items-center gap-1.5 transition-all cursor-pointer"
          >
            <Plus className="w-4 h-4" />
            录入黑名单
          </button>
        </div>
      </div>

      {/* 查这个人 —— 这一块是这次改动的核心 */}
      <div className="p-5 rounded-2xl bg-slate-900/70 border border-white/10 space-y-3">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div className="flex items-center gap-2 text-sm font-bold text-slate-200">
            <UserSearch className="w-4 h-4 text-amber-400" />
            查这个人
          </div>
          <span className="text-[11px] text-slate-500">
            按当前预设区服「{serverName || "—"}」比对（可在设置里改）
          </span>
        </div>

        <div className="flex flex-col sm:flex-row gap-2">
          <div className="relative flex-1">
            <Search className="w-4 h-4 absolute left-3 top-1/2 -translate-y-1/2 text-slate-400 pointer-events-none" />
            <input
              type="text"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={(e) => {
                // 按钮有 disabled，但回车绕过它；isComposing 是中文输入法选词
                // 的回车，不挡的话每选一次词都会比对一次（与查价页同一约定）
                if (e.key !== "Enter" || checking || e.nativeEvent.isComposing) return;
                handleCheck();
              }}
              placeholder="输入或粘贴角色名，回车立刻出结论（例如：盗号小贼）"
              className="w-full bg-slate-800/80 border border-white/15 rounded-xl pl-9 pr-3 py-2.5 text-sm text-slate-100 placeholder:text-slate-500 focus:outline-none focus:border-amber-400/80 transition-all"
            />
          </div>
          <button
            onClick={() => handleCheck()}
            disabled={checking}
            className="px-5 py-2.5 rounded-xl bg-amber-500 hover:bg-amber-400 active:bg-amber-600 text-slate-950 text-sm font-bold cursor-pointer disabled:opacity-50"
          >
            {checking ? "比对中…" : "查一下"}
          </button>
        </div>

        {checkError && (
          <p className="text-[11px] text-red-300 flex items-center gap-1.5">
            <AlertTriangle className="w-3.5 h-3.5" />
            {checkError}
          </p>
        )}

        {verdict && style && (
          <div className={`rounded-xl border p-4 space-y-2 ${style.tone}`}>
            <div className="flex flex-wrap items-center gap-2">
              {verdict.status === "clear" ? (
                <ShieldCheck className="w-4 h-4 shrink-0" />
              ) : (
                <ShieldAlert className="w-4 h-4 shrink-0" />
              )}
              <span className="font-bold text-sm">{style.label}</span>
              <span className="text-xs tabular-nums px-2 py-0.5 rounded bg-black/20">
                {verdict.checked_name}
              </span>
              <span className="text-[11px] opacity-80">
                {verdict.checked_server ? `${verdict.checked_server} · ` : ""}
                本地库 {verdict.total_local} 条
              </span>
            </div>

            <p className="text-xs leading-relaxed opacity-90">{verdict.note}</p>

            {verdict.entry && (
              <div className="rounded-lg bg-black/25 border border-white/10 p-3 text-xs space-y-1">
                <div className="flex flex-wrap items-center gap-2">
                  <span className="font-bold">
                    {verdict.entry.player_name}
                  </span>
                  <span className="px-1.5 py-0.5 rounded bg-black/30 text-[11px]">
                    {verdict.entry.server}
                  </span>
                  <span className="px-1.5 py-0.5 rounded-full bg-red-500/25 text-[11px]">
                    {verdict.entry.category}
                  </span>
                  <span className="text-[11px] opacity-70 tabular-nums">
                    {verdict.entry.created_at}
                  </span>
                </div>
                <p className="opacity-90">原因：{verdict.entry.reason}</p>
              </div>
            )}

            {verdict.related.length > 0 && verdict.status !== "hit" && (
              <ul className="space-y-1 text-[11px]">
                {verdict.related.slice(0, 5).map((item) => (
                  <li key={item.id ?? item.player_name} className="flex items-center gap-2">
                    <span className="font-semibold">{item.player_name}</span>
                    <span className="opacity-70">{item.server}</span>
                    <span className="opacity-70">{item.category}</span>
                    <span className="opacity-60 truncate">{item.reason}</span>
                  </li>
                ))}
              </ul>
            )}

            <p className="text-[11px] opacity-75">{style.hint}</p>
          </div>
        )}

        <div className="flex items-center gap-2 text-[11px] text-slate-500">
          <Info className="w-3.5 h-3.5 shrink-0" />
          游戏里交易时不想切窗口？按 {" "}
          <kbd className="px-1 rounded bg-slate-800 border border-white/10">{priceHudKey}</kbd>{" "}
          打开查价悬浮窗，切到「查人」那一栏输入名字即可。
        </div>
      </div>

      {/* 剪贴板嗅探开关 */}
      <div className="p-4 rounded-2xl bg-slate-900/70 border border-white/10 flex flex-col sm:flex-row sm:items-center justify-between gap-3">
        <div className="flex items-start gap-3">
          <Clipboard
            className={`w-4 h-4 mt-0.5 shrink-0 ${watch ? "text-emerald-400" : "text-slate-500"}`}
          />
          <div>
            <div className="text-sm font-bold text-slate-200 flex items-center gap-2">
              剪贴板自动嗅探
              <span
                className={`text-[10px] px-2 py-0.5 rounded-full border ${
                  watch
                    ? "bg-emerald-500/15 text-emerald-300 border-emerald-500/30"
                    : "bg-slate-800 text-slate-400 border-white/10"
                }`}
              >
                {watch ? "已开启" : "已关闭"}
              </span>
            </div>
            <p className="text-xs text-slate-400 mt-1 leading-relaxed">
              复制到像角色名的文本时自动比对，同名同服就响铃 + 弹提醒窗；只在本地比对。
            </p>
          </div>
        </div>

        <label className="relative inline-flex items-center cursor-pointer shrink-0">
          <input type="checkbox" checked={watch} onChange={toggleWatch} className="sr-only peer" />
          <div className="w-9 h-5 bg-slate-700 peer-focus:outline-none rounded-full peer peer-checked:after:translate-x-full after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-4 after:w-4 after:transition-all peer-checked:bg-emerald-500"></div>
        </label>
      </div>

      {/* Action Notice */}
      {actionNotice && (
        <div className="p-3 rounded-xl bg-emerald-500/15 border border-emerald-500/40 text-emerald-300 text-xs flex items-center gap-2">
          <Check className="w-4 h-4 text-emerald-400 shrink-0" />
          <span>{actionNotice}</span>
        </div>
      )}

      {/* 失败提示（页内、可关闭、不会像原生弹窗那样随手就没） */}
      <InlineNotice notice={notice} onClose={() => setNotice(null)} />

      {/* 嗅探命中横幅（页面开着时） */}
      {hit && (
        <div className="p-4 rounded-xl bg-red-950/70 border-2 border-red-500 text-red-200 flex items-start justify-between gap-3">
          <div className="flex items-start gap-3">
            <AlertTriangle className="w-6 h-6 text-red-400 shrink-0 mt-0.5" />
            <div>
              <div className="font-bold text-sm text-red-100 flex flex-wrap items-center gap-2">
                剪贴板命中黑名单！
                <span className="px-2 py-0.5 rounded bg-red-500/30 text-xs tabular-nums text-red-200">
                  {hit.entry?.player_name ?? hit.checked_name}
                  {hit.entry ? ` (${hit.entry.server})` : ""}
                </span>
                {hit.entry && (
                  <span className="text-xs text-red-300">标签: {hit.entry.category}</span>
                )}
              </div>
              {hit.entry && (
                <p className="text-xs text-red-300/90 mt-1">记录原因: {hit.entry.reason}</p>
              )}
            </div>
          </div>
          <button
            onClick={() => setHit(null)}
            className="px-2.5 py-1 rounded bg-red-900/50 hover:bg-red-800 text-xs text-red-200 border border-red-500/30 cursor-pointer"
          >
            关闭提醒
          </button>
        </div>
      )}

      {/* Search Filter */}
      <div className="relative">
        <Search className="w-4 h-4 absolute left-3 top-1/2 -translate-y-1/2 text-slate-400 pointer-events-none" />
        <input
          type="text"
          value={searchFilter}
          onChange={(e) => setSearchFilter(e.target.value)}
          placeholder="在本地黑名单表格里检索昵称、区服或违规原因..."
          className="w-full bg-slate-900/50 border border-white/10 rounded-xl pl-9 pr-4 py-2.5 text-sm text-slate-100 placeholder:text-slate-500 focus:outline-none focus:border-red-400/80 transition-all"
        />
      </div>

      {/* Blacklist Table */}
      <div className="rounded-2xl bg-slate-900/70 border border-white/10 overflow-hidden">
        <table className="w-full text-left text-xs text-slate-300">
          <thead className="bg-slate-800/80 text-slate-400 uppercase font-semibold border-b border-white/10">
            <tr>
              <th className="py-3 px-4">区服</th>
              <th className="py-3 px-4">玩家昵称</th>
              <th className="py-3 px-4">违规分类</th>
              <th className="py-3 px-4">原因 / 备忘</th>
              <th className="py-3 px-4">记录日期</th>
              <th className="py-3 px-4 text-right">操作</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-white/5">
            {filtered.map((item) => (
              <tr key={item.id} className="hover:bg-slate-800/40 transition-colors">
                <td className="py-3 px-4">
                  <span className="px-2 py-0.5 rounded bg-slate-800 text-slate-300 border border-white/10 font-medium">
                    {item.server}
                  </span>
                </td>
                <td className="py-3 px-4 font-bold text-slate-100 text-sm">{item.player_name}</td>
                <td className="py-3 px-4">
                  <span className="px-2 py-0.5 rounded-full bg-red-500/15 text-red-400 border border-red-500/30">
                    {item.category}
                  </span>
                </td>
                <td className="py-3 px-4 text-slate-400 max-w-xs truncate">{item.reason}</td>
                <td className="py-3 px-4 text-slate-500 tabular-nums">{item.created_at}</td>
                <td className="py-3 px-4 text-right">
                  <button
                    onClick={() => handleCheck(item.player_name)}
                    title="用这个人再查一次（看别的服有没有记录）"
                    className="p-1 mr-1 rounded hover:bg-amber-500/20 text-slate-400 hover:text-amber-300 transition-colors cursor-pointer"
                  >
                    <UserSearch className="w-4 h-4" />
                  </button>
                  <button
                    onClick={() => setPendingDelete(item)}
                    className="p-1 rounded hover:bg-red-500/20 text-slate-400 hover:text-red-400 transition-colors cursor-pointer"
                    title="移除"
                  >
                    <Trash2 className="w-4 h-4" />
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>

        {filtered.length === 0 && !loading && (
          <div className="p-10 text-center text-slate-500 space-y-2">
            <UserX className="w-10 h-10 mx-auto text-slate-600" />
            <p className="text-sm">暂无匹配的黑名单数据</p>
            <p className="text-xs text-slate-600">点击右上角「录入黑名单」扩充防骗库</p>
          </div>
        )}
      </div>

      {/* Add Modal */}
      {showAddModal && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4">
          <div className="w-full max-w-md rounded-2xl bg-slate-900 border border-white/15 p-6 space-y-4 shadow-2xl">
            <div className="flex items-center justify-between border-b border-white/10 pb-3">
              <h3 className="font-bold text-slate-100 text-base flex items-center gap-2">
                <ShieldAlert className="w-5 h-5 text-red-400" />
                录入避坑黑名单
              </h3>
              <button
                onClick={() => setShowAddModal(false)}
                className="text-slate-400 hover:text-white cursor-pointer"
              >
                ✕
              </button>
            </div>

            <form onSubmit={handleAdd} className="space-y-4 text-xs">
              <div>
                <label className="block text-slate-400 mb-1">所在区服</label>
                <select
                  value={newServer}
                  onChange={(e) => setNewServer(e.target.value)}
                  className="w-full bg-slate-800 border border-white/10 rounded-lg px-3 py-2 text-slate-200 focus:outline-none focus:border-red-400 cursor-pointer"
                >
                  {servers.map((server) => (
                    <option key={server.id} value={server.name}>
                      {server.name}
                    </option>
                  ))}
                  <option value="全服">全服通用（任何区服都算命中）</option>
                </select>
              </div>

              <div>
                <label className="block text-slate-400 mb-1">角色昵称</label>
                <input
                  type="text"
                  required
                  value={newName}
                  onChange={(e) => setNewName(e.target.value)}
                  placeholder="例如: 盗号小贼"
                  className="w-full bg-slate-800 border border-white/10 rounded-lg px-3 py-2 text-slate-100 focus:outline-none focus:border-red-400"
                />
              </div>

              <div>
                <label className="block text-slate-400 mb-1">违规类别</label>
                <select
                  value={newCategory}
                  onChange={(e) => setNewCategory(e.target.value)}
                  className="w-full bg-slate-800 border border-white/10 rounded-lg px-3 py-2 text-slate-200 focus:outline-none focus:border-red-400 cursor-pointer"
                >
                  <option value="骗子">骗子 (交易不付款/以假乱真)</option>
                  <option value="抢怪">抢怪 (恶意卡图抢怪)</option>
                  <option value="黑金">黑金 (涉黑/洗黑金交易)</option>
                  <option value="跑单">跑单 (订金跑路/不履约)</option>
                  <option value="倒爷">倒爷 (恶意垄断/哄抬物价)</option>
                  <option value="其他">其他恶性行为</option>
                </select>
              </div>

              <div>
                <label className="block text-slate-400 mb-1">原因与证据备忘</label>
                <textarea
                  rows={3}
                  value={newReason}
                  onChange={(e) => setNewReason(e.target.value)}
                  placeholder="记录发生的时间、交易细节或截图链接..."
                  className="w-full bg-slate-800 border border-white/10 rounded-lg px-3 py-2 text-slate-100 focus:outline-none focus:border-red-400"
                />
              </div>

              <p className="text-[11px] text-slate-500 leading-relaxed">
                同一个服里同名会覆盖原有记录，不会多出一行。
              </p>

              <div className="flex justify-end gap-3 pt-2">
                <button
                  type="button"
                  onClick={() => setShowAddModal(false)}
                  className="px-4 py-2 rounded-lg bg-slate-800 hover:bg-slate-700 text-slate-300 font-medium cursor-pointer"
                >
                  取消
                </button>
                <button
                  type="submit"
                  className="px-4 py-2 rounded-lg bg-red-500 hover:bg-red-400 text-slate-950 font-bold cursor-pointer"
                >
                  确认录入
                </button>
              </div>
            </form>
          </div>
        </div>
      )}

      {/* 删除确认：把要删的对象写在正文里，而不是靠 `confirm()` 的一句泛泛而谈 */}
      <ConfirmDialog
        open={pendingDelete !== null}
        title="从黑名单中移除这条记录？"
        body={
          pendingDelete ? (
            <>
              将移除「<span className="font-bold text-slate-100">{pendingDelete.player_name}</span>
              」（{pendingDelete.server} · {pendingDelete.category}）。
              只是删掉这台机器上的记录，之后还能重新录入。
            </>
          ) : null
        }
        confirmLabel="移除"
        onConfirm={() => {
          const target = pendingDelete;
          setPendingDelete(null);
          if (target) handleDelete(target.id);
        }}
        onCancel={() => setPendingDelete(null)}
      />
    </div>
  );
};
