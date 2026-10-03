# 枫之助（mxd-box）

冒险岛怀旧服的桌面小助手，Windows 上用。基于 Tauri 2 + React + TypeScript + Rust。

**功能**

- **查价**：拍卖行物价查询、金价走势、金价到价提醒
- **掉落速查**：一个框查怪物名 / 道具名 / ID
- **经验统计**：截屏识别等级和经验，算每分钟 / 每小时、预计升级时间，练级历史和镭射小结卡片
- **当前几线**、**开服提醒**（含声音和通知）
- **避坑黑名单**：查一个人，剪贴板里的名字自动比对
- **两个置顶悬浮窗**：`Alt+F` 查价窗、`F10` 经验窗，快捷键都能改

## 介绍视频


https://github.com/user-attachments/assets/63e5e7d6-94d8-436e-8455-dcc61be57646

## 下载

- 网盘：<https://pan.quark.cn/s/a9632b3efa8e>（软件内「检查更新」也读这里）
- QQ 群：1124493167（[点击加入](https://qm.qq.com/q/BghxNhhkas)），反馈问题、领最新版都可以来
- 或自己编译，见下。

## 它会做什么、不会做什么

- 经验和地图名是**截屏 + OCR** 读出来的，**不读写游戏进程内存，不注入，不模拟按键，不改游戏文件**。
- 「内存」指电脑的内存占用，不是游戏内存。
- 价格等数据来自第三方站点[冒险岛小册子](https://mxdc.dvg.cn/)的公开页面和接口，本项目与它、与游戏运营方都没有关系。
- 你的设置和黑名单只存在本机。

## 自己编译

需要 Windows、Node.js、pnpm、Rust（1.77.2 及以上）。

```bash
pnpm install
pnpm tauri dev      # 开发

https://github.com/user-attachments/assets/1ac38ba0-6a83-44eb-94cb-7db34ca35c33


pnpm tauri build    # 打包，产物在 src-tauri/target/release/mxd_box.exe
```

实现细节、踩过的坑和开发约定都在 [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md)。

## 致谢与署名

- 经验统计里定位游戏 HUD 的思路和模板数据借鉴了网页版 **枫记**（fj.need.run），详见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。这部分**不在 MIT 许可范围内**，权利归原作者。
- 地图名识别使用 PaddleOCR 的 PP-OCR 模型（Apache-2.0）。
- 数据来自冒险岛小册子。
- Tauri、React、tract、Recharts、Lucide 等开源项目。

## 许可证

[MIT](LICENSE)，只覆盖作者自己写的代码；第三方材料见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。

## 赞赏

枫之助一直免费，赞不赞赏用起来都一样。觉得好用的话，可以请作者喝杯咖灰：

<img src="src/assets/tip-qr.jpg" alt="微信赞赏码" width="260">
