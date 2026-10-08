#!/usr/bin/env bash
# Brings everything that runs restask up to this working tree (AGENTS.md §5.4,
# docs/INSTALL-AI.md "Updating"). Run it when the gates are green:
#
#   contrib/update.sh [<ssh-host> [<stack-dir>]]
#
# 1. this machine's `restask` binary (the CLI the editors call), when one is installed;
# 2. this machine's daemons, one unit per vault, when enabled;
# 3. the Obsidian plugin in every vault this machine works on;
# 4. the sync nodes: each compose stack gets the sources of this tree, is rebuilt and
#    restarted (`contrib/node.sh update`). Host and stack come from the arguments, else
#    $RESTASK_SERVER / $RESTASK_SERVER_DIR; without a stack, every stack on that host
#    that `restask setup` recorded in a machine config ([node]) — one per vault — and
#    /opt/docker/restask when none is recorded; without a host, every recorded stack on
#    every host. Nothing recorded and no host: this step is skipped.
#
# A machine has one machine config per vault (spec §14.2): config.toml and
# vaults/*/config.toml under $XDG_CONFIG_HOME/restask, or the one $RESTASK_CONFIG names.
#
# Every step leaves alone what is already current; a daemon that does not come up
# fails the script.
set -euo pipefail

cd "$(dirname "$0")/.."
server="${1-${RESTASK_SERVER-}}"
stack="${2-${RESTASK_SERVER_DIR-}}"

configs=()
if [ -n "${RESTASK_CONFIG-}" ]; then
    configs=("$RESTASK_CONFIG")
else
    home="${XDG_CONFIG_HOME:-$HOME/.config}/restask"
    for file in "$home/config.toml" "$home"/vaults/*/config.toml; do
        if [ -f "$file" ]; then configs+=("$file"); fi
    done
fi

step() { printf '\n== %s\n' "$*"; }
# key <config> <section> <name> — a quoted value of a machine config.
key() { sed -n "/^\[$2\]/,/^\[/s/^$3 *= *\"\(.*\)\"\$/\1/p" "$1" | head -n 1; }

step "binary on this machine"
if command -v restask >/dev/null; then
    cargo install --path crates/restask --locked --force --quiet
    restask --version
else
    echo "restask is not installed here: skipped"
fi

step "daemons on this machine"
vaults=()
if [ -n "${RESTASK_VAULT-}" ]; then vaults+=("$RESTASK_VAULT"); fi
units="$(systemctl --user list-unit-files 'restask.service' 'restask-*.service' --state=enabled --no-legend 2>/dev/null | awk '{ print $1 }' || true)"
if [ -z "$units" ]; then
    echo "no enabled unit: skipped (this machine is not a sync node)"
fi
for unit in $units; do
    unit_vault="$(systemctl --user cat "$unit" | sed -n 's/^ExecStart=.* --vault "\(.*\)"$/\1/p')"
    if [ -n "$unit_vault" ]; then vaults+=("$unit_vault"); fi
    systemctl --user restart "$unit"
    sleep 3
    journalctl --user -u "$unit" -n 20 --no-pager
    systemctl --user is-active --quiet "$unit" || { echo "the daemon of $unit did not come up" >&2; exit 1; }
done

step "Obsidian plugin in the vaults"
for config in ${configs[@]+"${configs[@]}"}; do
    vault="$(key "$config" vault path)"
    if [ -n "$vault" ]; then vaults+=("$vault"); fi
done
seen=""
for vault in ${vaults[@]+"${vaults[@]}"}; do
    case "$seen" in *"|$vault|"*) continue ;; esac
    seen="$seen|$vault|"
    plugin="$vault/.obsidian/plugins/restask"
    if [ ! -d "$plugin" ]; then
        continue
    elif [ -L "$plugin" ] || [ -L "$plugin/main.js" ] || [ -L "$plugin/manifest.json" ] || [ -L "$plugin/styles.css" ]; then
        echo "$plugin is a symlinked development install: left alone"
        continue
    fi
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
done
if [ -z "$seen" ]; then echo "no vault known on this machine: skipped"; fi

step "sync nodes"
# One `host<TAB>stack` line per stack to update.
targets=""
if [ -n "$server" ] && [ -n "$stack" ]; then
    targets="$server"$'\t'"$stack"
else
    for config in ${configs[@]+"${configs[@]}"}; do
        host="$(key "$config" node host)"
        dir="$(key "$config" node dir)"
        if [ -n "$host" ] && [ -n "$dir" ] && { [ -z "$server" ] || [ "$server" = "$host" ]; }; then
            targets="$targets$host"$'\t'"$dir"$'\n'
        fi
    done
    if [ -z "$targets" ] && [ -n "$server" ]; then
        targets="$server"$'\t'"/opt/docker/restask"
    fi
fi
if [ -z "$targets" ]; then
    echo "no host given and none recorded: skipped"
    exit 0
fi
printf '%s\n' "$targets" | sort -u | while IFS=$'\t' read -r host dir; do
    if [ -z "$host" ]; then continue; fi
    step "sync node $host:$dir"
    contrib/node.sh update "$host" "$dir" </dev/null
done
