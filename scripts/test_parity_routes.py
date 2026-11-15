"""Test cho scripts/parity_routes.py (stdlib unittest, không cần agent-hub): python3 -m unittest scripts/test_parity_routes.py"""

import json
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import parity_routes as P  # noqa: E402

APP = '''
from starlette.routing import Route, Mount
CB = "/cb"
ACTIONS: dict = {"approve": 1, "deny": 2}
def register_actions(actions, Action, _req):
    actions["extra_act"] = Action()
def create_app():
    return [Route("/", a), Route("/x/{id}", b, methods=["GET", "POST"]), Route(CB, c), Mount("/static", app=None)]
'''
OFFICE = '''
from starlette.routing import Route
OFFICE_ACTIONS = {"approve": "approve", "answer": "question_answer"}
def routes():
    return [Route("/office/do/{name}", f, methods=["POST"])]
'''


class ParityTest(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        web = self.tmp / "src" / "cfharness" / "web"
        web.mkdir(parents=True)
        (web / "app.py").write_text(textwrap.dedent(APP))
        (web / "office.py").write_text(textwrap.dedent(OFFICE))

    def test_scan(self):
        f = P.scan_hub(self.tmp)
        self.assertEqual(f["routes"], {"GET /", "GET /x/{id}", "POST /x/{id}", "GET /cb", "MOUNT /static", "POST /office/do/{name}"})
        self.assertEqual(f["actions"], {"approve", "deny", "extra_act"})
        self.assertEqual(f["office_actions"], {"approve", "answer"})

    def test_missing_todo_and_unknown_e2e_fail(self):
        f = P.scan_hub(self.tmp)
        parity = {"schema": 1, "routes": {k: {"cover": "structural"} for k in f["routes"]},
                  "actions": {"approve": {"cover": "e2e:confirm"}, "deny": {"cover": "TODO"}},
                  "office_actions": {"approve": {"cover": "structural"}, "answer": {"cover": "e2e:nope"}, "gone": {"cover": "structural"}}}
        pr = P.check(f, parity, {"confirm"})
        self.assertEqual(pr["missing"], ["actions: extra_act"])
        self.assertEqual(pr["todo"], ["actions: deny"])
        self.assertEqual(len(pr["unknown_e2e"]), 1)
        self.assertEqual(pr["stale"], ["office_actions: gone"])
        parity["routes"]["GET /"] = {"cover": "whatever"}
        self.assertEqual(len(P.check(f, parity, {"confirm"})["bad_cover"]), 1)

    def test_dynamic_route_path_is_an_error(self):
        web = self.tmp / "src" / "cfharness" / "web"
        (web / "extra.py").write_text("from starlette.routing import Route\nr = [Route(make_path(), h)]\n")
        with self.assertRaises(SystemExit):
            P.scan_hub(self.tmp)

    def test_repo_parity_json_is_valid(self):
        data = json.loads(P.PARITY.read_text(encoding="utf-8"))
        self.assertEqual(data["schema"], 1)
        for section in ("routes", "actions", "office_actions"):
            for k, v in data[section].items():
                self.assertRegex(v["cover"], P.COVER_RE, f"{section}: {k}")


if __name__ == "__main__":
    unittest.main()
