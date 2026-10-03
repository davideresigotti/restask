#!/usr/bin/env bash
# Brings everything that runs restask up to this working tree (AGENTS.md §5.4,
# docs/INSTALL-AI.md "Updating"). Run it when the gates are green:
#
#   contrib/update.sh [<ssh-host> [<stack-dir>]]
#
# 1. this machine's `restask` binary (the CLI the editors call), when one is installed;
# 2. this machine's daemon, when its unit is enabled;
# 3. the Obsidian plugin in the vault this machine works on;
# 4. the sync node: the compose stack in <stack-dir> on <ssh-host> gets the sources of
#    this tree, is rebuilt and restarted (`contrib/node.sh update`). Host and stack come
#    from the arguments, else $RESTASK_SERVER / $RESTASK_SERVER_DIR, else the [node]
#    section `restask setup` wrote into the machine config; a host without a known stack
#    means /opt/docker/restask. Without a host this step is skipped.
#
# Every step leaves alone what is already current; a daemon that does not come up
# fails the script.
set -euo pipefail

cd "$(dirname "$0")/.."
server="${1-${RESTASK_SERVER-}}"
config="${XDG_CONFIG_HOME:-$HOME/.config}/restask/config.toml"

step() { printf '\n== %s\n' "$*"; }

step "binary on this machine"
if command -v restask >/dev/null; then
    cargo install --path crates/restask --locked --force --quiet
    restask --version
else
    echo "restask is not installed here: skipped"
fi

step "daemon on this machine"
unit_vault=""
if systemctl --user is-enabled --quiet restask 2>/dev/null; then
    unit_vault="$(systemctl --user cat restask | sed -n 's/^ExecStart=.* --vault "\(.*\)"$/\1/p')"
    systemctl --user restart restask
    sleep 3
    journalctl --user -u restask -n 20 --no-pager
    systemctl --user is-active --quiet restask || { echo "the daemon did not come up" >&2; exit 1; }
else
    echo "no enabled unit: skipped (this machine is not the sync node)"
fi

step "Obsidian plugin in the vault"
vault="${RESTASK_VAULT:-$unit_vault}"
if [ -z "$vault" ] && [ -f "$config" ]; then
    vault="$(sed -n 's/^path *= *"\(.*\)"$/\1/p' "$config" | head -n 1)"
fi
plugin="$vault/.obsidian/plugins/restask"
if [ -z "$vault" ] || [ ! -d "$plugin" ]; then
    echo "no vault with the plugin installed: skipped"
elif [ -L "$plugin" ] || [ -L "$plugin/main.js" ] || [ -L "$plugin/manifest.json" ] || [ -L "$plugin/styles.css" ]; then
    echo "$plugin is a symlinked development install: left alone"
else
    copied=0
    for file in main.js manifest.json styles.css; do
        if ! cmp -s "crates/restask/assets/obsidian/$file" "$plugin/$file"; then
            cp "crates/restask/assets/obsidian/$file" "$plugin/$file"
            copied=1
        fi
    done
    if [ "$copied" = 1 ]; then
        echo "updated $plugin (reload the plugin or Obsidian to load it)"
    else
        echo "$plugin is current"
    fi
fi

step "sync node"
# The host and the stack: arguments, else the environment, else what `restask setup`
# recorded in the machine config ([node], spec §14.2).
node_key() { [ -f "$config" ] && sed -n "/^\[node\]/,/^\[/s/^$1 *= *\"\(.*\)\"\$/\1/p" "$config" | head -n 1; }
server="${server:-$(node_key host || true)}"
stack="${2-${RESTASK_SERVER_DIR:-$(node_key dir || true)}}"
if [ -z "$server" ]; then
    echo "no host given: skipped"
    exit 0
fi
exec contrib/node.sh update "$server" "${stack:-/opt/docker/restask}"
