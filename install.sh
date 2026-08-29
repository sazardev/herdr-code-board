#!/bin/sh
# Build and link herdr-code-board as a local Herdr plugin.
#
# `herdr plugin link` deliberately does not run build commands, so a local
# install has to build the release binary itself.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
cd "$root"

command -v herdr >/dev/null 2>&1 || { echo "herdr is not on PATH" >&2; exit 1; }
command -v cargo >/dev/null 2>&1 || { echo "cargo is not on PATH; install Rust 1.89+" >&2; exit 1; }

echo "building..."
cargo build --release

echo "linking $root ..."
herdr plugin link "$root"

echo
herdr plugin action invoke herdr-code-board.doctor || true

cat <<'EOF'

Linked. Next:

  herdr-code-board repo add           # inside a repo you want on the board
  herdr plugin pane open --plugin herdr-code-board --entrypoint board

Bind a key by adding this to ~/.config/herdr/config.toml:

  [[keys.command]]
  key = "prefix+b"
  type = "plugin_action"
  command = "herdr-code-board.open"
  description = "code board"

Then: herdr server reload-config
EOF
