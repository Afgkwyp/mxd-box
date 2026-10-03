import React, { useState } from "react";
import { Layers, Settings } from "lucide-react";
import { SettingsManager } from "../components/SettingsManager";
import { FloatSettings } from "../components/FloatSettings";
import { SubTabs, type TabDef } from "../components/ui/kit";

/**
 * 设置：原来的「基础配置」「悬浮窗与内存」两页合成一页，页内分两段。
 * 「关于与更新说明」单独成页（见 AboutPage，侧栏里排在设置下面）。
 */
type SettingsTab = "general" | "float";

const TABS: TabDef<SettingsTab>[] = [
  { key: "general", label: "通用", icon: <Settings className="w-3.5 h-3.5" /> },
  { key: "float", label: "悬浮窗与内存", icon: <Layers className="w-3.5 h-3.5" /> },
];

export const SettingsPage: React.FC = () => {
  const [tab, setTab] = useState<SettingsTab>("general");

  return (
    <div className="space-y-4">
      <SubTabs tabs={TABS} value={tab} onChange={setTab} />
      {tab === "general" && <SettingsManager />}
      {tab === "float" && <FloatSettings />}
    </div>
  );
};
