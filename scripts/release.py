"""发一个版本：改版本号 → 打包 → 自检 → 生成发给朋友的压缩包。

用法：

    python scripts/release.py 0.1.1                # 正常发版
    python scripts/release.py 0.1.1 --skip-build   # 同版本重打包；版本号已变化时必须重新构建
    python scripts/release.py 0.1.1 --dry-run      # 只打印会做什么，什么都不改

为什么要有这个脚本：发版要动三个地方的版本号（package.json / Cargo.toml / tauri.conf.json），
漏一个的症状是「界面显示的版本和文件属性里的不一样」，而这种不一致在排查问题时最误导人。
另外它强迫顺序正确：**先 `pnpm tauri build`，再打包自检**（用裸 `cargo build --release`
出过的包里会带着本机绝对路径和旧的版本信息，见 README）。
"""


import argparse
import os
import re
import shutil
import subprocess
import sys
import time
import zipfile

# 控制台默认是 GBK（Windows 中文环境），直接 print ✅/❌ 会抛 UnicodeEncodeError 把
# 整个自检打断在最关键的那一行。统一按 UTF-8 输出，实在编不出的字符退化成 ? 而不是崩。
for _stream in (sys.stdout, sys.stderr):
    try:
        _stream.reconfigure(encoding="utf-8", errors="replace")
    except (AttributeError, OSError):
        pass


ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DESKTOP = os.path.join(os.path.expanduser("~"), "Desktop")
SHARE_DIR = os.path.join(DESKTOP, "枫之助-怀旧服小助手")
EXE = os.path.join(SHARE_DIR, "枫之助.exe")
PACKAGE_FILES = ("枫之助.exe", "使用说明.txt")

VERSION_FILES = [
    ("package.json", r'("version"\s*:\s*")([^"]+)(")'),
    ("src-tauri/Cargo.toml", r'(^version\s*=\s*")([^"]+)(")'),
    ("src-tauri/tauri.conf.json", r'("version"\s*:\s*")([^"]+)(")'),
]


def update_page() -> str:
    """从 Rust 常量里读「上传到哪个分享页」，不在这里再写一份。

    以前的报错信息里手抄了一遍链接，换网盘时很容易只改代码、忘了这里 ——
    于是脚本把发布者引向已经作废的旧分享页。
    """
    path = os.path.join(ROOT, "src-tauri", "src", "update.rs")
    text = open(path, encoding="utf-8").read()
    match = re.search(r'DEFAULT_UPDATE_URL:\s*&str\s*=\s*"([^"]*)"', text)
    if not match or not match.group(1):
        sys.exit("❌ 在 update.rs 里找不到 DEFAULT_UPDATE_URL，或者它是空的")
    return match.group(1)


def log(message: str) -> None:
    print(message, flush=True)


def ensure_skip_build_version(version: str) -> None:
    """不引入平台专用 PE 解析依赖；跳过构建只能重打当前版本，不能给旧 exe 改标签。"""
    for relative, pattern in VERSION_FILES:
        path = os.path.join(ROOT, relative)
        try:
            with open(path, encoding="utf-8") as file:
                text = file.read()
        except OSError as err:
            raise ValueError(f"无法读取 {relative}，不能确认 --skip-build 的版本：{err}") from err
        match = re.search(pattern, text, flags=re.MULTILINE)
        if not match:
            raise ValueError(f"在 {relative} 里找不到版本号，不能安全使用 --skip-build")
        current = match.group(2)
        if current != version:
            raise ValueError(
                f"--skip-build 不能与版本变更同时使用：{relative} 当前是 {current}，目标是 {version}；"
                "请去掉 --skip-build 重新构建"
            )


def bump_versions(version: str, dry_run: bool) -> None:
    for relative, pattern in VERSION_FILES:
        path = os.path.join(ROOT, relative)
        text = open(path, encoding="utf-8").read()
        match = re.search(pattern, text, flags=re.MULTILINE)
        if not match:
            sys.exit(f"❌ 在 {relative} 里找不到版本号，脚本要不重写一遍")
        old = match.group(2)
        if old == version:
            log(f"  {relative}: 已经是 {version}")
            continue
        new_text = text[: match.start()] + match.group(1) + version + match.group(3) + text[match.end() :]
        log(f"  {relative}: {old} → {version}")
        if not dry_run:
            open(path, "w", encoding="utf-8").write(new_text)


README_TXT = os.path.join(SHARE_DIR, "使用说明.txt")


def sync_share_readme(version: str, dry_run: bool) -> None:
    """把分享包里那份「使用说明.txt」的版本号和日期改对。

    它不在 `VERSION_FILES` 里（不是程序版本，是给人看的），但同样属于「必须跟上」：
    曾经发过一版，exe 是 0.1.2、说明里写的是 0.1.1 —— 朋友排查问题时最先看的
    就是这份说明，写错版本会把他引到错的方向（比如以为自己的包不对）。
    """
    if not os.path.isfile(README_TXT):
        log(f"  （没有 {README_TXT}，跳过）")
        return
    text = open(README_TXT, encoding="utf-8").read()
    stamp = time.strftime("%Y-%m-%d")
    new_text, count = re.subn(
        r"版本 v\d+\.\d+\.\d+（构建于 [\d-]+）",
        f"版本 v{version}（构建于 {stamp}）",
        text,
        count=1,
    )
    if count == 0:
        log("  使用说明.txt: 开头没有「版本 vx.y.z（构建于 ……）」那行，没改")
        return
    if new_text == text:
        log(f"  使用说明.txt: 已经是 {version}")
        return
    log(f"  使用说明.txt: → v{version}（{stamp}）")
    if not dry_run:
        open(README_TXT, "w", encoding="utf-8").write(new_text)


def replace_exe(built: str, target: str) -> None:
    """把新 exe 拷过去；正在运行的那个会锁住文件。

    这不是罕见情况 —— 打包时本机的枫之助通常正开着（它就是拿来用的），
    直接 copy 会抛一个只有 Windows 错误码的 PermissionError，看不出该怎么办。
    """
    try:
        shutil.copy2(built, target)
    except PermissionError:
        sys.exit(
            f"❌ {target} 正被占用 —— 先退出正在运行的枫之助（托盘图标 → 退出程序），然后重跑：\n"
            f"   python scripts/release.py <版本> --skip-build"
        )


def run(command: list[str], cwd: str = ROOT) -> None:
    log(f"$ {' '.join(command)}")
    result = subprocess.run(command, cwd=cwd, shell=(os.name == "nt"))
    if result.returncode != 0:
        sys.exit(f"❌ 命令失败（退出码 {result.returncode}）：{' '.join(command)}")


def package_members(share_dir: str) -> list[str]:
    """只允许发布 exe 和使用说明，额外内容必须先由人检查、移走。"""
    try:
        with os.scandir(share_dir) as entries:
            files = {entry.name: entry.is_file(follow_symlinks=False) for entry in entries}
    except OSError as err:
        raise ValueError(f"无法读取分享目录 {share_dir}：{err}") from err

    unexpected = sorted(files.keys() - set(PACKAGE_FILES))
    if unexpected:
        raise ValueError(f"发现白名单外的文件或目录，请移走后重试：{', '.join(unexpected)}")
    if "枫之助.exe" not in files:
        raise ValueError("分享目录缺少 枫之助.exe，请先构建并复制产物")
    non_files = sorted(name for name, is_file in files.items() if not is_file)
    if non_files:
        raise ValueError(f"白名单项目必须是普通文件：{', '.join(non_files)}")

    return [name for name in PACKAGE_FILES if name in files]


def package_archive_errors(share_dir: str, archive_members: list[str]) -> list[str]:
    """确认最终 ZIP 与分享目录白名单完全一致，避免只检查 exe 漏掉旁置数据。"""
    try:
        expected = {
            f"{os.path.basename(os.path.normpath(share_dir))}/{name}"
            for name in package_members(share_dir)
        }
    except ValueError as err:
        return [str(err)]

    actual = list(archive_members)
    errors = []
    if len(actual) != len(set(actual)):
        errors.append("ZIP 中存在重复成员名")
    missing = sorted(expected - set(actual))
    extra = sorted(set(actual) - expected)
    if missing:
        errors.append(f"ZIP 缺少白名单文件：{', '.join(missing)}")
    if extra:
        errors.append(f"ZIP 含有白名单外成员：{', '.join(extra)}")
    return errors


def package(version: str) -> str:
    """把白名单中的 exe 和使用说明（若存在）打成 `枫之助-<版本>.zip`。"""
    try:
        members = package_members(SHARE_DIR)
    except ValueError as err:
        sys.exit(f"❌ 发布目录不符合白名单：{err}")

    zip_path = os.path.join(DESKTOP, f"枫之助-{version}.zip")
    if os.path.exists(zip_path):
        os.remove(zip_path)
    with zipfile.ZipFile(zip_path, "w", zipfile.ZIP_DEFLATED) as archive:
        for name in members:
            full = os.path.join(SHARE_DIR, name)
            archive.write(full, f"{os.path.basename(SHARE_DIR)}/{name}")
    return zip_path


def main() -> int:
    parser = argparse.ArgumentParser(description="发一个版本")
    parser.add_argument("version", help="新版本号，例如 0.1.1")
    parser.add_argument(
        "--skip-build",
        action="store_true",
        help="仅重打包当前版本；版本号变化时必须重新构建",
    )
    parser.add_argument("--dry-run", action="store_true", help="只打印，不改文件、不构建")
    args = parser.parse_args()

    if not re.fullmatch(r"\d+\.\d+\.\d+", args.version):
        sys.exit("❌ 版本号要写成 x.y.z，例如 0.1.1")
    version = args.version
    if args.skip_build:
        try:
            ensure_skip_build_version(version)
        except ValueError as err:
            sys.exit(f"❌ {err}")

    log(f"=== 发版 {version} ===")
    log("① 改三处版本号 + 分享包里的使用说明")
    bump_versions(version, args.dry_run)
    sync_share_readme(version, args.dry_run)

    if args.dry_run:
        log("（--dry-run：以下步骤跳过）")
        return 0

    built = os.path.join(ROOT, "src-tauri", "target", "release", "mxd_box.exe")
    if args.skip_build:
        if not os.path.isfile(built):
            sys.exit(f"❌ 找不到 {built}，去掉 --skip-build 重新构建")
        log("② 跳过构建（--skip-build），直接复用 target/release/mxd_box.exe")
    else:
        log("② 构建（必须用 pnpm tauri build，不能用裸 cargo build）")
        run(["pnpm", "tauri", "build"])

    # 拷贝不属于「构建」而属于「交付」：--skip-build 只跳过编译，不该把新的
    # 前端产物配着一个旧 exe 发出去（这一点上卡过一次：dist 比 exe 新，自检才拦住）。
    replace_exe(built, os.path.join(ROOT, "枫之助.exe"))
    replace_exe(built, EXE)
    log(f"  已更新 {EXE}")

    log("③ 单元测试 + 约定检查")
    run(["cargo", "test"], cwd=os.path.join(ROOT, "src-tauri"))

    log("④ 打包（仅 exe + 可选使用说明；其他内容会阻止发布）")
    zip_path = package(version)
    size_mb = os.path.getsize(zip_path) / 1024 / 1024
    log(f"  {zip_path}（{size_mb:.2f} MB）")

    log("⑤ 发布自检（个人数据 0 命中 + 前端新版 + ZIP 成员白名单）")
    run([sys.executable, os.path.join(ROOT, "scripts", "check-share-exe.py"), "--zip", zip_path])

    log("")
    log("=" * 60)
    log(f"上传这个文件到固定的分享页（替换旧文件，链接不变）：枫之助-{version}.zip")
    log(f"  {update_page()}")
    log("")
    log("朋友那边怎么知道有新版本：程序启动时去这个分享页看文件名里最大的")
    log("「枫之助-x.y.z」，比自己在跑的新就在顶栏冒一个「有新版本」小胶囊；")
    log("基础配置页的「版本与更新」里也能手动点一次、并直接打开这个分享页。")
    log("zip 直接放分享根目录，或者放进一个文件夹里都行（程序会往下看几层）。")
    log("地址只有一处定义：src-tauri/src/update.rs 的 DEFAULT_UPDATE_URL；")
    log("界面上的链接也是问后端要的这个值，两边不会写岔。只有**换分享页**时")
    log("才需要改那个常量、重新出包。")
    log("")
    log("上传完可以当场验一次（会真的去读那个分享页）：")
    log("  cd src-tauri && cargo test --lib -- --ignored real_quark_share")
    log("=" * 60)
    log(f"（打包时间 {time.strftime('%Y-%m-%d %H:%M:%S')}）")
    return 0


if __name__ == "__main__":
    sys.exit(main())
