#!/usr/bin/env python3
"""Kiểm ngang chức năng app ↔ web của hub (PLAN P5).

Đọc mã nguồn web của agent-hub (TĨNH, chỉ `ast` của stdlib — không import, không cần venv của hub):
  - mọi `Route(...)` / `Mount(...)` / `WebSocketRoute(...)` trong `src/cfharness/web/*.py` (app.py, office.py và các module
    trang mà app.py gộp vào) → khoá `"<METHOD> <path>"` (Mount → `"MOUNT <path>"`);
  - khoá của bảng `ACTIONS` (dict literal trong app.py + mọi `actions["…"] = …` trong các `register_actions`);
  - khoá của `OFFICE_ACTIONS` (office.py).
So với `tests/e2e/parity.json`: mục nào của hub chưa có trong parity.json → thoát mã 1 (CI đỏ). Mục trong parity.json mà hub
không còn → cảnh báo (thoát 1 khi `--strict`). Mỗi mục có `cover`:
  - `"e2e:<id>"`   có ca Playwright mang nhãn `[<id>]` trong `tests/e2e/*.spec.ts` (script kiểm nhãn có thật);
  - `"structural"` ngang chức năng do cấu trúc (WebView hiển thị chính trang của hub — PLAN Q1), không cần native;
  - `"native:<mô tả>"` app có phần native riêng (tải tệp, giọng nói, badge…);
  - `"manual:<mô tả>"` checklist tay (PLAN §9).

  python3 scripts/parity_routes.py [--hub ../agent-hub] [--write] [--strict] [--json]

`--write` thêm mục mới với `cover: "TODO"` (vẫn thoát 1 tới khi có người điền) và giữ nguyên mục cũ.
"""

from __future__ import annotations

import argparse
import ast
import json
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PARITY = ROOT / "tests" / "e2e" / "parity.json"
E2E_DIR = ROOT / "tests" / "e2e"
ROUTE_CALLS = {"Route", "WebSocketRoute"}
COVER_RE = re.compile(r"^(e2e:[a-z0-9][a-z0-9_.-]*|structural|native:.+|manual:.+)$")


def _name(func: ast.expr) -> str | None:
    if isinstance(func, ast.Name):
        return func.id
    if isinstance(func, ast.Attribute):
        return func.attr
    return None


def _str(node: ast.expr | None, consts: dict[str, str] | None = None) -> str | None:
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return node.value
    if consts and isinstance(node, ast.Name):
        return consts.get(node.id)  # module-level constant, e.g. CALLBACK_PATH = "/mcp/oauth/callback"
    return None


def _consts(tree: ast.Module) -> dict[str, str]:
    out: dict[str, str] = {}
    for st in tree.body:
        if isinstance(st, ast.Assign) and len(st.targets) == 1 and isinstance(st.targets[0], ast.Name):
            v = _str(st.value)
            if v is not None:
                out[st.targets[0].id] = v
        elif isinstance(st, ast.AnnAssign) and isinstance(st.target, ast.Name):
            v = _str(st.value)
            if v is not None:
                out[st.target.id] = v
    return out


def scan_file(path: Path) -> tuple[set[str], set[str], set[str]]:
    """→ (routes, actions, office_actions) found statically in one module."""
    tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
    routes: set[str] = set()
    actions: set[str] = set()
    office: set[str] = set()
    consts = _consts(tree)
    for node in ast.walk(tree):
        if isinstance(node, ast.Call):
            fn = _name(node.func)
            if fn in ROUTE_CALLS or fn == "Mount":
                p = _str(node.args[0], consts) if node.args else None
                if p is None:
                    for kw in node.keywords:
                        if kw.arg == "path":
                            p = _str(kw.value, consts)
                if p is None:
                    raise SystemExit(f"{path}:{node.lineno}: {fn}(...) với đường dẫn không tĩnh — sửa scripts/parity_routes.py")
                if fn == "Mount":
                    routes.add(f"MOUNT {p}")
                    continue
                if fn == "WebSocketRoute":
                    routes.add(f"WS {p}")
                    continue
                methods = ["GET"]
                for kw in node.keywords:
                    if kw.arg == "methods" and isinstance(kw.value, (ast.List, ast.Tuple, ast.Set)):
                        ms = [_str(e) for e in kw.value.elts]
                        methods = [m.upper() for m in ms if m]
                for m in methods:
                    if m != "HEAD":
                        routes.add(f"{m} {p}")
        targets: list[ast.expr] = []
        value: ast.expr | None = None
        if isinstance(node, ast.Assign):
            targets, value = node.targets, node.value
        elif isinstance(node, ast.AnnAssign) and node.value is not None:
            targets, value = [node.target], node.value
        for t in targets:
            if isinstance(t, ast.Name) and t.id in ("ACTIONS", "OFFICE_ACTIONS") and isinstance(value, ast.Dict):
                keys = {k for k in (_str(k) for k in value.keys if k is not None) if k}
                (actions if t.id == "ACTIONS" else office).update(keys)
            if isinstance(t, ast.Subscript) and isinstance(t.value, ast.Name) and t.value.id in ("actions", "ACTIONS"):
                k = _str(t.slice)
                if k:
                    actions.add(k)
    return routes, actions, office


def scan_hub(hub: Path) -> dict[str, set[str]]:
    web = hub / "src" / "cfharness" / "web"
    if not (web / "app.py").is_file():
        raise SystemExit(f"không thấy {web / 'app.py'} — truyền --hub <đường dẫn agent-hub> hoặc biến AGENT_HUB")
    routes: set[str] = set()
    actions: set[str] = set()
    office: set[str] = set()
    for f in sorted(web.glob("*.py")):
        r, a, o = scan_file(f)
        routes |= r
        actions |= a
        office |= o
    if not office:
        raise SystemExit("không đọc được OFFICE_ACTIONS trong office.py — cấu trúc hub đã đổi, sửa scripts/parity_routes.py")
    return {"routes": routes, "actions": actions, "office_actions": office}


def e2e_ids() -> set[str]:
    ids: set[str] = set()
    for f in E2E_DIR.rglob("*.spec.ts"):
        ids |= set(re.findall(r"\[([a-z0-9][a-z0-9_.-]*)\]", f.read_text(encoding="utf-8")))
    return ids


def hub_commit(hub: Path) -> str | None:
    try:
        out = subprocess.run(["git", "-C", str(hub), "rev-parse", "--short", "HEAD"], capture_output=True, text=True, timeout=10)
        return out.stdout.strip() or None
    except (OSError, subprocess.SubprocessError):
        return None


def load_parity() -> dict:
    if not PARITY.exists():
        return {"schema": 1, "routes": {}, "actions": {}, "office_actions": {}}
    data = json.loads(PARITY.read_text(encoding="utf-8"))
    if data.get("schema") != 1:
        raise SystemExit(f"{PARITY}: schema {data.get('schema')!r} không hỗ trợ")
    return data


def check(found: dict[str, set[str]], parity: dict, ids: set[str]) -> dict[str, list[str]]:
    problems: dict[str, list[str]] = {"missing": [], "todo": [], "bad_cover": [], "unknown_e2e": [], "stale": []}
    for section, keys in found.items():
        have = parity.get(section) or {}
        for k in sorted(keys):
            entry = have.get(k)
            if entry is None:
                problems["missing"].append(f"{section}: {k}")
                continue
            cover = entry.get("cover") if isinstance(entry, dict) else None
            if cover == "TODO":
                problems["todo"].append(f"{section}: {k}")
            elif not isinstance(cover, str) or not COVER_RE.match(cover):
                problems["bad_cover"].append(f"{section}: {k} ({cover!r})")
            elif cover.startswith("e2e:") and cover[4:] not in ids:
                problems["unknown_e2e"].append(f"{section}: {k} → {cover} (không có ca [{cover[4:]}] trong tests/e2e/*.spec.ts)")
        for k in sorted(set(have) - keys):
            problems["stale"].append(f"{section}: {k}")
    return problems


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="Kiểm ngang chức năng app ↔ web của hub (route + ACTIONS + OFFICE_ACTIONS)")
    default_hub = os.environ.get("AGENT_HUB") or str(ROOT.parent / "agent-hub")
    ap.add_argument("--hub", default=default_hub, help="thư mục repo agent-hub (mặc định $AGENT_HUB hoặc ../agent-hub)")
    ap.add_argument("--write", action="store_true", help="thêm mục mới (cover TODO) vào parity.json")
    ap.add_argument("--strict", action="store_true", help="mục thừa (hub không còn) cũng làm thoát 1")
    ap.add_argument("--json", action="store_true", help="in kết quả dạng JSON")
    a = ap.parse_args(argv)
    hub = Path(a.hub).expanduser().resolve()
    found = scan_hub(hub)
    parity = load_parity()
    if a.write:
        for section, keys in found.items():
            sec = parity.setdefault(section, {})
            for k in keys:
                sec.setdefault(k, {"cover": "TODO"})
            parity[section] = dict(sorted(sec.items()))
        parity["source"] = f"agent-hub@{hub_commit(hub) or '?'}"
        PARITY.write_text(json.dumps(parity, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    problems = check(found, parity, e2e_ids())
    fail = any(problems[k] for k in ("missing", "todo", "bad_cover", "unknown_e2e")) or (a.strict and problems["stale"])
    counts = {k: len(v) for k, v in found.items()}
    if a.json:
        print(json.dumps({"ok": not fail, "counts": counts, "problems": problems}, ensure_ascii=False, indent=2))
        return 1 if fail else 0
    print(f"hub: {hub} ({hub_commit(hub) or '?'}) — {counts['routes']} route, {counts['actions']} ACTIONS, "
          f"{counts['office_actions']} OFFICE_ACTIONS")
    labels = {
        "missing": "Chưa có trong tests/e2e/parity.json (thêm mục + cover)",
        "todo": "Còn cover TODO",
        "bad_cover": "cover không hợp lệ (e2e:<id> | structural | native:… | manual:…)",
        "unknown_e2e": "cover e2e trỏ tới ca không tồn tại",
        "stale": "Có trong parity.json nhưng hub không còn" + (" (lỗi do --strict)" if a.strict else " (cảnh báo)"),
    }
    for k, label in labels.items():
        if problems[k]:
            print(f"\n{label}: {len(problems[k])}")
            for line in problems[k]:
                print(f"  - {line}")
    print("\nKẾT QUẢ: " + ("THIẾU — CI đỏ" if fail else "đủ"))
    return 1 if fail else 0


if __name__ == "__main__":
    sys.exit(main())
