# 第三方材料与署名

本项目的 MIT 许可（见 [LICENSE](LICENSE)）**只覆盖作者自己写的代码**。下面这些来自别处，权利归原作者。

## 枫记（网页版，fj.need.run）

「经验统计」里定位游戏 HUD 的思路和数据借鉴了网页版 **枫记**：

- `src-tauri/assets/mxdc-hud-templates.json`：HP / MP / EXP 标签的像素模板，取自枫记网页版的布局模板。
- `src-tauri/src/exp/hud.rs`：锚点定位的做法与常数（血条亮边框 → 三个条的固定间距反推缩放 → 模板比对 → 多尺度金字塔）移植自枫记。
- 「预计升级时间」「阶段效率」等统计口径参考了枫记的同名功能。
- 本项目使用 PaddleOCR 读地图名，也是看到枫记走通了这条路。

本项目与枫记**没有隶属或合作关系**，也**没有取得枫记作者的授权**。以上材料不在 MIT 许可范围内。
如果你是枫记的作者，希望撤下或调整，请在 Issues 里留言，我会处理。

## PaddleOCR PP-OCR 识别模型

- 文件：`src-tauri/assets/ppocr_rec.onnx`、`ppocr_dict.txt`
- 来源：PaddlePaddle / PaddleOCR 项目，许可 **Apache-2.0**（<https://www.apache.org/licenses/LICENSE-2.0>）

## 数据来源

物价、金价、掉落、开服监控等数据来自第三方站点 **冒险岛小册子**（mxdc.dvg.cn）的公开页面与接口。
本项目与该站点、与游戏运营方均无关联，数据归其所有者。

## 依赖库

Tauri、React、tract、Recharts、Lucide 等开源项目，分别采用 MIT / Apache-2.0 / ISC 等许可，
完整依赖见 `package.json`、`pnpm-lock.yaml`、`src-tauri/Cargo.toml`、`src-tauri/Cargo.lock`。

## 游戏

「冒险岛」及相关名称、画面归其权利人所有。本项目是玩家自制的非官方工具。
