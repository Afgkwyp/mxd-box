/**
 * 外部链接。**不要**在界面里写 `<a href target="_blank">`。
 *
 * Tauri 的 webview 里没有「新窗口」这个概念：`<a target="_blank">` 点下去什么都
 * 不会发生（用户报过一次「主窗口左下角的小册子网站点击没打开」）。要打开外部
 * 网页一律走 `invoke("open_external_url", { url })`，它由 Rust 侧交给系统浏览器。
 *
 * 有一条自动检查守着这条规矩（src-tauri/tests/frontend_conventions.rs）。
 */

/** 小册子首页。 */
export const MXDC_HOME = "https://mxdc.dvg.cn/";

/** 小册子「开服监控」页（和站点自己的判断对照用）。 */
export const MXDC_SERVER_MONITOR = "https://mxdc.dvg.cn/tools/server-monitor/";

/** Apache-2.0 许可全文（随包分发的 PP-OCR 模型用的是这个许可）。 */
export const APACHE_LICENSE = "https://www.apache.org/licenses/LICENSE-2.0";

/** 作者建的玩家交流群。点一下直接用系统浏览器拉起 QQ 加群。 */
export const QQ_GROUP_NUMBER = "1124493167";
export const QQ_GROUP_INVITE = "https://qm.qq.com/q/BghxNhhkas";
