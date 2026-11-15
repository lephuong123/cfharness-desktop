"""Test cho scripts/release_tools.py: python3 -m unittest scripts/test_release_tools.py"""

import base64
import json
import sys
import tempfile
import unittest
from datetime import datetime, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import release_tools as R  # noqa: E402

KEY = base64.b64encode(f"untrusted comment: minisign public key: 0707\n{base64.b64encode(bytes(42)).decode()}\n".encode()).decode()

CHANGELOG = """# Nhật ký thay đổi

## Chưa phát hành
- việc dở

## v0.2.0 — 2026-12-01
### Mới
- Tự cập nhật

## v0.1.0 — 2026-11-20
- Bản đầu
"""


class ReleaseToolsTest(unittest.TestCase):
    def test_changelog_section(self):
        self.assertEqual(R.changelog_section("v0.2.0", CHANGELOG), "### Mới\n- Tự cập nhật\n")
        self.assertEqual(R.changelog_section("0.1.0", CHANGELOG), "- Bản đầu\n")
        with self.assertRaises(SystemExit):
            R.changelog_section("0.3.0", CHANGELOG)  # release note first: no section → no release

    def test_repo_versions_agree(self):
        v = set(R.versions().values())
        self.assertEqual(len(v), 1, R.versions())
        R.check_version("v" + v.pop())
        with self.assertRaises(SystemExit):
            R.check_version("99.0.0")

    def test_release_config(self):
        c = R.release_config(KEY, "acme/cfharness-desktop")
        self.assertTrue(c["bundle"]["createUpdaterArtifacts"])
        self.assertEqual(c["plugins"]["updater"]["endpoints"],
                         ["https://github.com/acme/cfharness-desktop/releases/latest/download/latest.json"])
        for bad_key, repo in [("", "a/b"), ("CFHARNESS_UPDATER_PUBKEY_PLACEHOLDER", "a/b"), (KEY, "OWNER/REPO"), (KEY, "a/b/c")]:
            with self.assertRaises(SystemExit):
                R.release_config(bad_key, repo)
        w = R.release_config(KEY, "a/b", "A" * 40)
        self.assertEqual(w["bundle"]["windows"]["certificateThumbprint"], "A" * 40)

    def test_latest_json(self):
        d = Path(tempfile.mkdtemp())
        (d / "mac").mkdir()
        (d / "mac" / "Cfharness Desktop.app.tar.gz").write_bytes(b"x")
        (d / "mac" / "Cfharness Desktop.app.tar.gz.sig").write_text("SIGMAC\n")
        (d / "mac" / "Cfharness Desktop_0.2.0_universal.dmg").write_bytes(b"x")
        (d / "Cfharness Desktop_0.2.0_x64-setup.exe").write_bytes(b"x")
        (d / "Cfharness Desktop_0.2.0_x64-setup.exe.sig").write_text("SIGWIN")
        doc, files = R.latest_json(d, "v0.2.0", "acme/app", "notes\n", datetime(2026, 12, 1, tzinfo=timezone.utc))
        self.assertEqual(doc["version"], "0.2.0")
        self.assertEqual(doc["pub_date"], "2026-12-01T00:00:00Z")
        self.assertEqual(sorted(doc["platforms"]), ["darwin-aarch64", "darwin-x86_64", "windows-x86_64"])
        self.assertEqual(doc["platforms"]["windows-x86_64"],
                         {"signature": "SIGWIN", "url": "https://github.com/acme/app/releases/download/v0.2.0/Cfharness-Desktop_0.2.0_x64-setup.exe"})
        self.assertTrue(all(" " not in f.name for f in files))
        self.assertIn("Cfharness-Desktop_0.2.0_universal.dmg", {f.name for f in files})
        json.dumps(doc)

    def test_latest_json_requires_signatures(self):
        d = Path(tempfile.mkdtemp())
        (d / "A.app.tar.gz").write_bytes(b"x")
        (d / "B-setup.exe").write_bytes(b"x")
        (d / "B-setup.exe.sig").write_text("s")
        with self.assertRaises(SystemExit):
            R.latest_json(d, "0.2.0", "acme/app", "")


if __name__ == "__main__":
    unittest.main()
