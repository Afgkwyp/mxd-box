"""验证最终发布物：① exe 个人数据 0 命中 ② 前端是新版 ③ ZIP 只有白名单成员。

用脚本文件而不是 heredoc —— 中文经由 shell 传递会被按当前代码页转换，
图案串变成了错的字节，结果是「查什么都是 0」（比没有检查更糟）。

注意：**不能用 exe 里有没有界面文案来判断前端新旧**。Tauri 2 会把前端产物
压缩（brotli）后嵌进二进制，`加载更多` 这种串在里面根本不是明文，搜到 0 不代表缺失。
所以这里分别检查 exe 中的个人数据、dist 的前端标记与时间戳，以及最终 ZIP 的成员清单；
ZIP 只允许含 exe 和可选的使用说明，不能把分享目录里的其他文件一并发出。
"""

import argparse
import os
import re
import sys
import zipfile

from release import DESKTOP, SHARE_DIR, package_archive_errors

# 控制台默认是 GBK（Windows 中文环境），直接 print ✅/❌ 会抛 UnicodeEncodeError 把
# 整个自检打断在最关键的那一行。统一按 UTF-8 输出，实在编不出的字符退化成 ? 而不是崩。
for _stream in (sys.stdout, sys.stderr):
    try:
        _stream.reconfigure(encoding="utf-8", errors="replace")
    except (AttributeError, OSError):
        pass


ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXE = os.path.join(SHARE_DIR, "枫之助.exe")
DIST = os.path.join(ROOT, "dist")

# ① 个人数据：这些出现在 exe 里就是事故（这几个词本身就不该出现）
PERSONAL = [
    "C:\\Users",          # 本机绝对路径（cargo registry / 项目路径）
    "Administrator",      # Windows 用户名
    "AppData",            # 用户数据目录
    "acw_tc",             # 小册子 Cookie 名
    "wittemlauth",        # 小册子登录凭证
    "thirdwx.qlogo.cn",   # 微信头像地址（登录后才会出现）
]

# ①' 「字段名 + 真值」才算事故的那几个。
#
# 光看 `session_id` / `openid` 这几个词会把**自己的代码**当成事故：`exp_samples` 表里
# 就有一列叫 `session_id`（Sqlite 的建表语句会原样嵌进二进制）。一个列名不是个人数据，
# 真正要防的是**值**被带出去，所以这里要求后面跟着一个看起来像凭证的值。
PERSONAL_WITH_VALUE = [
    re.compile(rb"session_id\s*[=:]\s*['\"]?[A-Za-z0-9_\-]{6,}"),
    re.compile(rb"openid\s*[=:]\s*['\"]?[A-Za-z0-9_\-]{6,}"),
]

# ② 前端新版标记：动前端时顺手加一条
FEATURES = [
    "加载更多",
    "查掉落",
    "怪物名 / 道具名 / ID（锅盖 = 谁掉它",
    "名字命中",
    "在小册子看这只怪的图鉴",
    "经验统计",       # 加了经验统计页
    "本段平均",       # 会话小结那一行
    "不建议用于比较",  # 数据质量分级（历史列表上的那个标签）
    "有效打怪",       # 质量面板里和「画面覆盖」并列的那一个
    "复制图片",       # 小结卡片的分享弹窗（复制到剪贴板发微信 / QQ）
    "界面动效",       # 动效开关（设置页 / 总览）
    "分段效率",       # 按升级分段的每级时速
    "枫之助 · 冒险岛怀旧服小助手",  # 卡片页脚（宣传那一行）
]


def scan(data: bytes, tokens: list[str]) -> list[tuple[str, int]]:
    return [(t, data.count(t.encode("utf-8"))) for t in tokens]


def scan_patterns(data: bytes, patterns: list[re.Pattern[bytes]]) -> list[tuple[str, int]]:
    return [(p.pattern.decode("utf-8", "replace"), len(p.findall(data))) for p in patterns]


def self_test() -> int:
    """证明这个扫描器抓得住真事故、又不会误伤自己的代码。

    「检查永远返回 ✅」是最危险的失效方式（比没有检查更糟），所以这里用两份
    合成数据各跑一次。
    """
    fake_clean = (
        b"CREATE TABLE exp_samples (id INTEGER, session_id INTEGER NOT NULL, ...)"
        b"openid TEXT NOT NULL"
    )
    fake_leak = fake_clean + b"\nCookie: wittemlauth=abc123; session_id=9f3a2b1c8d; "

    clean_hits = sum(n for _, n in scan(fake_clean, PERSONAL))
    clean_hits += sum(n for _, n in scan_patterns(fake_clean, PERSONAL_WITH_VALUE))
    leak_hits = sum(n for _, n in scan(fake_leak, PERSONAL))
    leak_hits += sum(n for _, n in scan_patterns(fake_leak, PERSONAL_WITH_VALUE))

    print(f"干净数据（只有列名）-> {clean_hits} 处（应为 0）")
    print(f"真泄露数据（带凭证值）-> {leak_hits} 处（应 > 0）")
    if clean_hits or not leak_hits:
        print("❌ 扫描器本身不对：要么误伤自己的代码，要么漏掉真泄露")
        return 1
    print("✅ 扫描器自证通过")
    return 0


def default_zip_path() -> str:
    cargo_toml = os.path.join(ROOT, "src-tauri", "Cargo.toml")
    with open(cargo_toml, encoding="utf-8") as file:
        text = file.read()
    match = re.search(r'(?m)^version\s*=\s*"([^"]+)"\s*$', text)
    if not match:
        raise ValueError(f"在 {cargo_toml} 找不到 Cargo 版本号")
    return os.path.join(DESKTOP, f"枫之助-{match.group(1)}.zip")


def main() -> int:
    parser = argparse.ArgumentParser(description="发布产物自检")
    parser.add_argument("--self-test", action="store_true", help="只验证扫描器自身")
    parser.add_argument("--zip", dest="zip_path", help="指定最终发布 ZIP（默认按 Cargo 版本查找）")
    args = parser.parse_args()
    if args.self_test:
        return self_test()

    ok = True

    print(f"exe: {EXE}")
    exe_data = open(EXE, "rb").read()
    print(f"     {len(exe_data)} 字节")

    print("--- ① 个人数据（应全为 0）---")
    total = 0
    for token, n in scan(exe_data, PERSONAL):
        total += n
        print(f"  {token!r:24} -> {n}")
    for token, n in scan_patterns(exe_data, PERSONAL_WITH_VALUE):
        total += n
        print(f"  {token:24} -> {n}")
    if total:
        print(f"  ❌ 有 {total} 处个人数据混进去了")
        ok = False
    else:
        print("  ✅ 0 处")

    print("--- ② 前端是否为新版（查 dist/，不是查 exe）---")
    if not os.path.isdir(DIST):
        print("  ❌ 找不到 dist/，先跑 pnpm build")
        return 1
    bundle = b""
    newest_dist = 0.0
    for name in os.listdir(os.path.join(DIST, "assets")):
        path = os.path.join(DIST, "assets", name)
        if os.path.isfile(path):
            bundle += open(path, "rb").read()
            newest_dist = max(newest_dist, os.path.getmtime(path))
    bundle += open(os.path.join(DIST, "index.html"), "rb").read()
    for token, n in scan(bundle, FEATURES):
        if n == 0:
            print(f"  ❌ 前端产物里没有 {token!r} —— 忘了重新构建？")
            ok = False
        else:
            print(f"  ✅ {token!r} 命中 {n} 次")

    exe_mtime = os.path.getmtime(EXE)
    if newest_dist > exe_mtime:
        print(
            f"  ❌ dist 比 exe 新（dist {newest_dist:.0f} > exe {exe_mtime:.0f}）："
            "改完前端没重新打包，exe 里是旧界面"
        )
        ok = False
    else:
        print("  ✅ dist 早于 exe：前端产物是在这次打包之前生成的")

    try:
        zip_path = args.zip_path or default_zip_path()
    except (OSError, ValueError) as err:
        errors = [str(err)]
    else:
        print(f"--- ③ 最终 ZIP 成员：{zip_path} ---")
        try:
            with zipfile.ZipFile(zip_path) as archive:
                errors = package_archive_errors(SHARE_DIR, archive.namelist())
        except (OSError, zipfile.BadZipFile) as err:
            errors = [f"无法读取 ZIP：{err}"]
    if errors:
        for error in errors:
            print(f"  ❌ {error}")
        ok = False
    else:
        print("  ✅ ZIP 成员与分享目录白名单完全一致")

    print("结果：" + ("✅ 可以发出去" if ok else "❌ 先修上面标 ❌ 的项"))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
