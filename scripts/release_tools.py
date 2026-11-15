#!/usr/bin/env python3
"""Công cụ phát hành dùng trong `.github/workflows/release.yml` (chỉ stdlib, chạy được trên máy dev).

  python3 scripts/release_tools.py check-version 0.2.0      # tag khớp package.json + Cargo.toml + tauri.conf.json
  python3 scripts/release_tools.py notes 0.2.0              # in mục "## v0.2.0 — <ngày>" của CHANGELOG.md (thiếu → mã 1)
  python3 scripts/release_tools.py config --out f.json      # cấu hình phát hành: pubkey updater + endpoint repo + updater artifacts
  python3 scripts/release_tools.py latest-json --dir dist-release --version 0.2.0 --repo owner/repo --notes-file notes.md

Quy trình (AGENTS.md "Phát hành"): release note TRƯỚC → nâng version → tag `vX.Y.Z` → workflow build + ký updater + tạo
GitHub Release với thân = đúng mục CHANGELOG của phiên bản đó.
"""

from __future__ import annotations

import argparse
import base64
import json
import os
import re
import shutil
import sys
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
VERSION_RE = re.compile(r"^\d+\.\d+\.\d+(-[0-9A-Za-z.]+)?$")
REPO_RE = re.compile(r"^[A-Za-z0-9_.-]{1,100}/[A-Za-z0-9_.-]{1,100}$")


def fail(msg: str) -> None:
    print(f"LỖI: {msg}", file=sys.stderr)
    raise SystemExit(1)


# ------------------------------------------------------------------------------------------------ versions

def versions(root: Path = ROOT) -> dict[str, str]:
    pkg = json.loads((root / "package.json").read_text(encoding="utf-8"))["version"]
    conf = json.loads((root / "src-tauri" / "tauri.conf.json").read_text(encoding="utf-8"))["version"]
    cargo = ""
    in_pkg = False
    for line in (root / "src-tauri" / "Cargo.toml").read_text(encoding="utf-8").splitlines():
        s = line.strip()
        if s.startswith("["):
            in_pkg = s == "[package]"
        elif in_pkg and s.startswith("version"):
            cargo = s.split("=", 1)[1].strip().strip('"')
            break
    return {"package.json": pkg, "src-tauri/Cargo.toml": cargo, "src-tauri/tauri.conf.json": conf}


def check_version(version: str, root: Path = ROOT) -> None:
    version = version.removeprefix("v")
    if not VERSION_RE.match(version):
        fail(f"phiên bản '{version}' không đúng dạng X.Y.Z")
    bad = {f: v for f, v in versions(root).items() if v != version}
    if bad:
        fail("tag không khớp phiên bản trong mã: " + ", ".join(f"{f}={v}" for f, v in bad.items())
             + " — nâng version (sau khi viết release note) rồi mới tag")


# ------------------------------------------------------------------------------------------------ changelog

def changelog_section(version: str, text: str) -> str:
    """Body of `## vX.Y.Z — YYYY-MM-DD` up to the next `## ` heading (without the heading itself)."""
    version = version.removeprefix("v")
    head = re.compile(rf"^## v{re.escape(version)} — \d{{4}}-\d{{2}}-\d{{2}}\s*$")
    lines = text.splitlines()
    for i, line in enumerate(lines):
        if head.match(line):
            body: list[str] = []
            for nxt in lines[i + 1:]:
                if nxt.startswith("## "):
                    break
                body.append(nxt)
            out = "\n".join(body).strip()
            if not out:
                fail(f"mục v{version} trong CHANGELOG.md trống")
            return out + "\n"
    fail(f"CHANGELOG.md chưa có mục '## v{version} — <YYYY-MM-DD>' — viết release note TRƯỚC khi tag (AGENTS.md)")
    raise AssertionError  # unreachable


# ------------------------------------------------------------------------------------------------ release config

def valid_pubkey(b64: str) -> bool:
    """Same check as `updater::valid_pubkey` (Rust): base64 of a minisign public key file."""
    try:
        text = base64.b64decode(b64.strip(), validate=True).decode("utf-8")
    except Exception:  # noqa: BLE001
        return False
    lines = text.splitlines()
    if len(lines) < 2 or not lines[0].startswith("untrusted comment:"):
        return False
    try:
        return len(lines[1].strip()) == 56 and len(base64.b64decode(lines[1].strip(), validate=True)) == 42
    except Exception:  # noqa: BLE001
        return False


def release_config(pubkey: str, repo: str, windows_cert: str | None = None) -> dict:
    if not valid_pubkey(pubkey):
        fail("TAURI_UPDATER_PUBKEY thiếu hoặc không phải khoá công khai của `tauri signer generate` (đặt ở Settings → Variables)")
    if not REPO_RE.match(repo) or "OWNER/REPO" in repo:
        fail(f"repo '{repo}' không hợp lệ")
    conf: dict = {
        "bundle": {"createUpdaterArtifacts": True},
        "plugins": {"updater": {"pubkey": pubkey.strip(),
                                "endpoints": [f"https://github.com/{repo}/releases/latest/download/latest.json"]}},
    }
    if windows_cert:  # Authenticode (disabled until a certificate exists): thumbprint of the cert imported in the job
        if not re.fullmatch(r"[0-9A-Fa-f]{40}", windows_cert):
            fail("WINDOWS_CERT_THUMBPRINT không hợp lệ")
        conf["bundle"]["windows"] = {"certificateThumbprint": windows_cert, "digestAlgorithm": "sha256",
                                     "timestampUrl": "http://timestamp.digicert.com"}
    return conf


# ------------------------------------------------------------------------------------------------ latest.json

PLATFORMS = {
    # updater artifact suffix → Tauri platform keys
    ".app.tar.gz": ["darwin-aarch64", "darwin-x86_64"],  # universal macOS bundle serves both
    "-setup.exe": ["windows-x86_64"],
}


def safe_name(name: str) -> str:
    """GitHub rewrites spaces in asset names; publish names without them so URLs in latest.json match."""
    return re.sub(r"[^A-Za-z0-9._-]+", "-", name)


def latest_json(dir_: Path, version: str, repo: str, notes: str, now: datetime | None = None) -> tuple[dict, list[Path]]:
    """Rename assets in `dir_` to safe names, build latest.json. Returns (doc, files to upload)."""
    version = version.removeprefix("v")
    if not REPO_RE.match(repo):
        fail(f"repo '{repo}' không hợp lệ")
    files: list[Path] = []
    for f in sorted(p for p in dir_.rglob("*") if p.is_file()):
        dst = dir_ / safe_name(f.name)
        if dst != f:
            if dst.exists():
                fail(f"trùng tên tệp phát hành: {dst.name}")
            shutil.move(str(f), dst)
        files.append(dst)
    platforms: dict[str, dict[str, str]] = {}
    for f in files:
        for suffix, keys in PLATFORMS.items():
            if f.name.endswith(suffix):
                sig = f.with_name(f.name + ".sig")
                if not sig.exists():
                    fail(f"thiếu chữ ký updater {sig.name} (TAURI_SIGNING_PRIVATE_KEY chưa đặt?)")
                url = f"https://github.com/{repo}/releases/download/v{version}/{f.name}"
                for k in keys:
                    platforms[k] = {"signature": sig.read_text(encoding="utf-8").strip(), "url": url}
    missing = [k for keys in PLATFORMS.values() for k in keys if k not in platforms]
    if missing:
        fail("thiếu gói updater cho: " + ", ".join(missing))
    doc = {
        "version": version,
        "notes": notes.strip(),
        "pub_date": (now or datetime.now(timezone.utc)).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "platforms": dict(sorted(platforms.items())),
    }
    return doc, files


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="Công cụ phát hành Cfharness Desktop")
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("check-version")
    p.add_argument("version")
    p = sub.add_parser("notes")
    p.add_argument("version")
    p.add_argument("--out")
    p = sub.add_parser("config")
    p.add_argument("--out", required=True)
    p.add_argument("--pubkey", default=os.environ.get("TAURI_UPDATER_PUBKEY", ""))
    p.add_argument("--repo", default=os.environ.get("GITHUB_REPOSITORY", ""))
    p.add_argument("--windows-cert", default=os.environ.get("WINDOWS_CERT_THUMBPRINT") or None)
    p = sub.add_parser("latest-json")
    p.add_argument("--dir", required=True)
    p.add_argument("--version", required=True)
    p.add_argument("--repo", default=os.environ.get("GITHUB_REPOSITORY", ""))
    p.add_argument("--notes-file", required=True)
    p.add_argument("--out", default="latest.json")
    a = ap.parse_args(argv)
    if a.cmd == "check-version":
        check_version(a.version)
        print(f"phiên bản {a.version.removeprefix('v')} khớp")
    elif a.cmd == "notes":
        body = changelog_section(a.version, (ROOT / "CHANGELOG.md").read_text(encoding="utf-8"))
        if a.out:
            Path(a.out).write_text(body, encoding="utf-8")
        else:
            sys.stdout.write(body)
    elif a.cmd == "config":
        Path(a.out).write_text(json.dumps(release_config(a.pubkey, a.repo, a.windows_cert), indent=2) + "\n", encoding="utf-8")
        print(f"đã ghi {a.out}")
    elif a.cmd == "latest-json":
        notes = Path(a.notes_file).read_text(encoding="utf-8")
        doc, files = latest_json(Path(a.dir), a.version, a.repo, notes)
        Path(a.out).write_text(json.dumps(doc, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        for f in files:
            print(f)
    return 0


if __name__ == "__main__":
    sys.exit(main())
