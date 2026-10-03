"""发布包白名单的最小回归测试：python scripts/test_release.py"""

import os
import tempfile
import unittest
from unittest.mock import patch

from release import ensure_skip_build_version, package_archive_errors, package_members


class PackageAllowlistTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory()
        self.share_dir = self.temp_dir.name
        self.prefix = os.path.basename(os.path.normpath(self.share_dir))
        self.exe_member = f"{self.prefix}/枫之助.exe"
        self.readme_member = f"{self.prefix}/使用说明.txt"
        self._touch("枫之助.exe")

    def tearDown(self) -> None:
        self.temp_dir.cleanup()

    def _touch(self, name: str) -> None:
        with open(os.path.join(self.share_dir, name), "wb") as file:
            file.write(b"test")

    def test_only_exe_and_optional_readme_are_packaged(self) -> None:
        self._touch("使用说明.txt")
        self.assertEqual(package_members(self.share_dir), ["枫之助.exe", "使用说明.txt"])
        self.assertEqual(
            package_archive_errors(self.share_dir, [self.exe_member, self.readme_member]),
            [],
        )

        os.remove(os.path.join(self.share_dir, "使用说明.txt"))
        self.assertEqual(package_members(self.share_dir), ["枫之助.exe"])
        self.assertEqual(package_archive_errors(self.share_dir, [self.exe_member]), [])

    def test_unexpected_files_and_directories_fail_closed(self) -> None:
        self._touch("mxd_box.db")
        with self.assertRaisesRegex(ValueError, "mxd_box.db"):
            package_members(self.share_dir)

        os.remove(os.path.join(self.share_dir, "mxd_box.db"))
        os.mkdir(os.path.join(self.share_dir, "logs"))
        with self.assertRaisesRegex(ValueError, "logs"):
            package_members(self.share_dir)

    def test_skip_build_rejects_a_version_bump(self) -> None:
        version_path = os.path.join(self.share_dir, "version.txt")
        with open(version_path, "w", encoding="utf-8") as file:
            file.write('version="0.1.3"')

        with patch("release.ROOT", self.share_dir), patch(
            "release.VERSION_FILES", [("version.txt", r'(version=")([^"]+)(")')]
        ):
            ensure_skip_build_version("0.1.3")
            with self.assertRaisesRegex(ValueError, "版本变更"):
                ensure_skip_build_version("0.1.4")

    def test_archive_must_match_the_allowlisted_source_files_exactly(self) -> None:
        self._touch("使用说明.txt")
        self.assertTrue(
            any(
                "缺少白名单文件" in error
                for error in package_archive_errors(self.share_dir, [self.exe_member])
            )
        )
        self.assertTrue(
            any(
                "白名单外成员" in error
                for error in package_archive_errors(
                    self.share_dir,
                    [self.exe_member, self.readme_member, f"{self.prefix}/mxd_box.db"],
                )
            )
        )
        self.assertTrue(
            any(
                "重复成员名" in error
                for error in package_archive_errors(
                    self.share_dir,
                    [self.exe_member, self.exe_member, self.readme_member],
                )
            )
        )


if __name__ == "__main__":
    unittest.main()
