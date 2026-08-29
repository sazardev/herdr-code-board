#!/bin/sh
# Unlink herdr-code-board. Board state and config are left in place; the paths
# are printed so you can remove them yourself if you actually want them gone.
set -eu

herdr plugin unlink herdr-code-board 2>/dev/null || \
  herdr plugin uninstall herdr-code-board 2>/dev/null || \
  echo "herdr-code-board was not registered" >&2

config=$(herdr plugin config-dir herdr-code-board 2>/dev/null || true)
cat <<EOF

Unlinked. Your board was not deleted. To remove it too:

  rm -rf ${config:-~/.config/herdr/plugins/config/herdr-code-board}
  rm -rf ~/.local/state/herdr-code-board

EOF
