#!/bin/sh
# Chạy app (tauri dev) với một hub demo của agent-hub (FakeRuntime — không gọi claude thật, không tốn tiền),
# tách biệt hoàn toàn khỏi dữ liệu app thật và hub thật của bạn.
#
#   AGENT_HUB=~/Workspace/agent-hub scripts/dev-demo.sh
#
# Biến môi trường dev (chỉ dùng khi phát triển/kiểm thử):
#   CFHARNESS_DESKTOP_DATA_DIR   thư mục dữ liệu app thay cho app_data_dir
#   CFHARNESS_DESKTOP_HOMES      danh sách HUB_HOME thay cho bước khám phá hub local
#   CFHARNESS_DESKTOP_CFHARNESS  đường dẫn binary cfharness
set -eu
AGENT_HUB="${AGENT_HUB:-$HOME/Workspace/agent-hub}"
PORT="${PORT:-8799}"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/cfh-desktop-dev.XXXXXX")"
HOME_DIR="$WORK/demo-hub"
mkdir -p "$HOME_DIR" "$WORK/bin"
printf '#!/bin/sh\nexec uv run --quiet --project "%s" cfharness "$@"\n' "$AGENT_HUB" > "$WORK/bin/cfharness"
chmod +x "$WORK/bin/cfharness"

echo "Khởi động hub demo ở $HOME_DIR (cổng $PORT)…"
( cd "$AGENT_HUB" && uv run python -m cfharness.web.demo --home "$HOME_DIR" --port "$PORT" > "$WORK/demo.log" 2>&1 ) &
DEMO_PID=$!
trap 'kill $DEMO_PID 2>/dev/null || true' EXIT INT TERM
i=0
until grep -q '^LOGIN ' "$WORK/demo.log" 2>/dev/null; do
  i=$((i + 1)); [ $i -gt 120 ] && { echo "hub demo không lên — xem $WORK/demo.log"; exit 1; }
  sleep 1
done

CFHARNESS_DESKTOP_DATA_DIR="$WORK/appdata" \
CFHARNESS_DESKTOP_HOMES="$HOME_DIR" \
CFHARNESS_DESKTOP_CFHARNESS="$WORK/bin/cfharness" \
RUST_LOG="${RUST_LOG:-info,cfharness_desktop_lib=debug}" \
npm run tauri dev
