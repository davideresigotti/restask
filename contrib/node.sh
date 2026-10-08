#!/usr/bin/env bash
# The sync node — the one machine that runs the restask daemon (spec §1.1) — as a Docker
# compose stack on an always-on server, driven from here over ssh (docs/INSTALL-AI.md, spec
# App. E). `restask setup` runs `stack`, `check` and `install`; `contrib/update.sh` runs
# `update`. A server holds one stack per vault.
#
#   contrib/node.sh stack   <ssh-host> <vault-on-host>
#       Prints the stack directory of that vault, and nothing else: the stack that
#       already serves it — wherever it is, when its container exists, else among
#       `restask` and `restask-*` in the ssh user's home directory — else a directory
#       no stack is in: `restask`, then `restask-<vault folder>` (a folder called
#       `restask-x` gives `restask-x`).
#
#   contrib/node.sh check   <ssh-host> <vault-on-host> [<stack-dir> [<caldav-url>]]
#       The host is reachable, has Docker with compose, holds the vault folder, and
#       <stack-dir> is free or already serves that vault. A daemon running there is
#       stopped: setup is about to change the vault. With <caldav-url>: the host can
#       reach that server (see "The server's name" below).
#
#   contrib/node.sh install <ssh-host> <vault-on-host> <stack-dir> <local-vault>
#       Standard input: CalDAV URL, username, password — one per line.
#       Creates the stack if it is not there, ships the sources of this tree, builds the
#       image, waits until the file sync has made the host's copy of the vault equal to
#       <local-vault>, joins the vault from inside the container (`restask setup --join`
#       with the credentials read above) and starts the daemon.
#
#   contrib/node.sh update  <ssh-host> [<stack-dir>]
#       Ships the sources of this tree, rebuilds, restarts, shows the first log lines.
#
# The server's name. The daemon must reach the CalDAV server from the host, and a name
# only the home network's DNS answers is not known to a host that asks another resolver.
# `check` and `install` test it: the host resolves the URL's name and connects to its
# port. A name the host does not know is pinned to the address it has on this machine,
# when the host reaches the server there: `install` writes it into the stack's
# docker-compose.override.yml (`extra_hosts`), which compose reads with every command.
# A server the host cannot reach either way stops the run, before anything is installed.
#
# <stack-dir> defaults to `restask`, relative to the ssh user's home directory. The
# stack holds  docker-compose.yml  .env  src/  data/ ; install and update replace src/
# and the image, install also writes data/ (the machine config) through the join and
# docker-compose.override.yml when the server's name is pinned (the file is setup's:
# written or removed by every install, left alone when it is someone else's).
# A stack's own docker-compose.yml and .env are never rewritten once they exist.
# Project, container and image of a stack are named after its directory (.env), so the
# stacks of two vaults share nothing.
set -euo pipefail

cd "$(dirname "$0")/.."

die() { printf 'node: %s\n' "$*" >&2; exit 1; }
say() { printf '   %s\n' "$*"; }

[ $# -ge 2 ] || die "usage: node.sh stack|check|install|update <ssh-host> … (see the head of this file)"
action="$1"
host="$2"

# One ssh connection for the whole run: whatever ssh asks (a passphrase, a password) is
# asked once. `restask setup` has asked already: it opened the connection when the host
# was typed and names its socket in RESTASK_SSH_CONTROL; that connection is used, for
# `check` and `install` alike, and left open — it is setup's to close.
if [ -n "${RESTASK_SSH_CONTROL-}" ]; then
    ssh_opts=(-o ControlMaster=auto -o "ControlPath=$RESTASK_SSH_CONTROL" -o ControlPersist=60)
else
    control="$(mktemp -d)"
    ssh_opts=(-o ControlMaster=auto -o "ControlPath=$control/%C" -o ControlPersist=60)
    cleanup() {
        ssh "${ssh_opts[@]}" -O exit "$host" 2>/dev/null || true
        rm -rf "$control"
    }
    trap cleanup EXIT
fi

# remote <args…> — runs the script given on standard input on the host, as bash, with
# the arguments as "$1" "$2" …; quoting survives whatever the paths contain.
remote() {
    local quoted=""
    if [ $# -gt 0 ]; then quoted="$(printf '%q ' "$@")"; fi
    ssh "${ssh_opts[@]}" "$host" "bash -s -- $quoted"
}

# remote_in <dir> <command…> — runs one command in a directory of the host, this
# script's standard input going to the command.
remote_in() {
    local dir="$1"
    shift
    # shellcheck disable=SC2029  # the command line is meant to be expanded here
    ssh "${ssh_opts[@]}" "$host" "cd $(printf '%q' "$dir") && $(printf '%q ' "$@")"
}

# server_endpoint <url> — sets server_name and server_port to what the URL names.
server_endpoint() {
    local url="$1" scheme rest authority
    scheme="${url%%://*}"
    rest="${url#*://}"
    authority="${rest%%[/?#]*}"
    authority="${authority##*@}"
    case "$authority" in
    \[*)
        server_name="${authority#\[}"
        server_name="${server_name%%\]*}"
        server_port="${authority##*\]}"
        server_port="${server_port#:}"
        ;;
    *:*)
        server_name="${authority%%:*}"
        server_port="${authority##*:}"
        ;;
    *)
        server_name="$authority"
        server_port=""
        ;;
    esac
    if [ -z "$server_port" ]; then
        if [ "$scheme" = https ]; then server_port=443; else server_port=80; fi
    fi
    [ -n "$server_name" ] || die "\`$url\` names no server"
}

# Run on the host with <name> <port> <address>: 3 when the host does not know the name
# (asked only without an address), 4 when nothing answers on the port.
probe='
name="$1"; port="$2"; target="${3:-$1}"
if [ -z "$3" ] && command -v getent >/dev/null 2>&1; then
    getent ahosts "$name" >/dev/null 2>&1 || exit 3
fi
timeout 8 bash -c "exec 3<>\"/dev/tcp/\$0/\$1\"" "$target" "$port" 2>/dev/null || exit 4
'

# server_pin <url> — checks that the host reaches the CalDAV server, and prints the
# address its name must be pinned to there: nothing when the host knows the name, the
# address the name has on this machine when it does not. Dies when the host cannot
# reach the server either way. Called in a command substitution: the caller names the
# server itself (server_endpoint) when it needs server_name.
server_pin() {
    local url="$1" code=0 address
    server_endpoint "$url"
    remote "$server_name" "$server_port" "" <<<"$probe" || code=$?
    case "$code" in
    0) return 0 ;;
    3) ;;
    4) die "$host cannot connect to $server_name, port $server_port: is $url the address the server has on the network, and does a firewall let $host through?" ;;
    *) die "cannot test from $host whether it reaches $url" ;;
    esac
    address="$(getent ahostsv4 "$server_name" 2>/dev/null | awk 'NR == 1 { print $1 }')"
    if [ -z "$address" ]; then
        address="$(getent ahosts "$server_name" 2>/dev/null | awk 'NR == 1 { print $1 }')"
    fi
    [ -n "$address" ] || die "neither this computer nor $host knows the name $server_name"
    case "$address" in
    127.* | ::1) die "$server_name is this computer itself; the daemon on $host needs the address the server has on the network" ;;
    esac
    case "$server_name" in
    *[!A-Za-z0-9.-]*) die "$host does not know the name $server_name" ;;
    esac
    code=0
    remote "$server_name" "$server_port" "$address" <<<"$probe" || code=$?
    [ "$code" -eq 0 ] ||
        die "$host does not know the name $server_name (its DNS is not the one of this computer), and cannot connect to the address the name has here ($address, port $server_port). Give setup a URL that $host reaches too, or add the name to the DNS $host uses"
    printf '%s\n' "$address"
}

# pin_server <stack> <address> — makes the stack's containers know the server's name by
# that address, or by the host's own DNS when the address is empty. The override file
# is this script's (its first line says so); one that is not is left as it is.
pin_server() {
    local stack="$1" address="$2"
    remote "$stack" "$server_name" "$address" <<'REMOTE' || die "could not record the server's address in $host:$stack"
set -e
cd "$1"
file=docker-compose.override.yml
mark='# restask setup: the address of the CalDAV server'
if [ -f "$file" ] && ! grep -qF "$mark" "$file"; then
    [ -n "$3" ] || exit 0
    echo "node: $1/$file is not one setup wrote. Add to its service restask:  extra_hosts: [\"$2:$3\"]  and run setup again" >&2
    exit 1
fi
if [ -z "$3" ]; then
    rm -f "$file"
    exit 0
fi
printf '%s\n' \
    "$mark, whose name this server's DNS does not know." \
    "# Written by every \`restask setup\` for this vault; edits are lost." \
    "services:" \
    "  restask:" \
    "    extra_hosts:" \
    "      - \"$2:$3\"" >"$file"
REMOTE
}

# What the image is built from, as it is in this tree: tracked and new files in a
# clone, everything but build output otherwise.
ship_sources() {
    local stack="$1"
    say "sending the sources to $host:$stack/src"
    {
        if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
            git ls-files -z -co --exclude-standard --deduplicate -- \
                Cargo.toml Cargo.lock .dockerignore crates contrib/docker/Dockerfile |
                while IFS= read -r -d '' file; do
                    if [ -e "$file" ]; then printf '%s\0' "$file"; fi
                done
        else
            find Cargo.toml Cargo.lock .dockerignore crates contrib/docker/Dockerfile \
                -name target -prune -o -type f -print0
        fi
    } | tar --null --files-from=- -czf - |
        remote_in "$stack" sh -c 'rm -rf src.new && mkdir src.new && tar -xzf - --no-same-owner -C src.new && rm -rf src && mv src.new src'
}

build_image() {
    local stack="$1"
    say "building the image on $host (the first build compiles everything: a few minutes)"
    remote_in "$stack" docker compose build --quiet </dev/null
}

start_daemon() {
    local stack="$1"
    remote "$stack" <<'REMOTE' || die "the daemon on $host did not come up"
set -e
cd "$1"
docker compose up -d
sleep 5
docker compose logs --tail 20 --no-log-prefix
test -n "$(docker compose ps --status running -q)"
docker image prune -f --filter label=org.opencontainers.image.title=restask >/dev/null
REMOTE
}

# The files a pass reads, with their digests: the notes, the vault config and the sync
# state. Run in a vault folder, here and on the host; equal output means the file sync
# has delivered everything a pass depends on.
manifest='find . -type d -name ".?*" ! -name .restask -prune -o -type f \( -name "*.md" -o -name restask.toml -o -path "./.restask/*" \) ! -name lock ! -name "*.sync-conflict-*" ! -name "*.restask-tmp" ! -name ".syncthing.*" -print0 | LC_ALL=C sort -z | xargs -0 -r sha256sum'

case "$action" in
stack)
    [ $# -eq 3 ] || die "usage: node.sh stack <ssh-host> <vault-on-host>"
    vault="${3%/}"
    remote "$vault" <<'REMOTE'
vault="$1"
# 1. A container of a restask stack that mounts the vault says where its stack is.
for id in $(docker ps -aq --filter label=com.docker.compose.service=restask 2>/dev/null); do
    mounted="$(docker inspect -f '{{range .Mounts}}{{if eq .Destination "/vault"}}{{.Source}}{{end}}{{end}}' "$id" 2>/dev/null)"
    if [ "${mounted%/}" = "$vault" ]; then
        dir="$(docker inspect -f '{{index .Config.Labels "com.docker.compose.project.working_dir"}}' "$id" 2>/dev/null)"
        if [ -n "$dir" ] && [ -f "$dir/docker-compose.yml" ]; then
            printf '%s\n' "${dir#"$HOME"/}"
            exit 0
        fi
    fi
done
# 2. A stack without a container (stopped and removed, or never started).
for dir in restask restask-*; do
    [ -f "$dir/docker-compose.yml" ] || continue
    if (cd "$dir" && docker compose config 2>/dev/null) |
        awk -v vault="$vault" '{ line = $0; sub(/^[ \t-]*source:[ \t]*/, "", line); sub(/\/$/, "", line); if ($0 ~ /source:/ && line == vault) found = 1 } END { exit !found }'; then
        printf '%s\n' "$dir"
        exit 0
    fi
done
# 3. A directory no stack is in.
name="$(basename "$vault" | tr '[:upper:]' '[:lower:]' | sed 's/[^a-z0-9]\{1,\}/-/g; s/^-//; s/-$//; s/^restask-//')"
dir=restask
count=1
while [ -f "$dir/docker-compose.yml" ]; do
    if [ "$count" -eq 1 ]; then dir="restask-${name:-vault}"; else dir="restask-${name:-vault}-$count"; fi
    count=$((count + 1))
done
printf '%s\n' "$dir"
REMOTE
    ;;

check)
    [ $# -ge 3 ] || die "usage: node.sh check <ssh-host> <vault-on-host> [<stack-dir> [<caldav-url>]]"
    vault="${3%/}"
    stack="${4:-restask}"
    url="${5-}"
    ssh "${ssh_opts[@]}" "$host" true || die "cannot reach \`$host\` over ssh (does \`ssh $host\` work?)"
    remote "$vault" "$stack" <<'REMOTE'
vault="$1"
stack="$2"
fail() { printf 'node: %s\n' "$*" >&2; exit 1; }
docker compose version >/dev/null 2>&1 ||
    fail "this server has no Docker with the compose plugin. Install restask on it by hand (docs/INSTALL-AI.md, \"A server without Docker\") and run setup here with --no-daemon"
[ -d "$vault" ] ||
    fail "$vault is not a folder on this server. Share the vault with it in the file sync first, and give the folder it lands in"
if [ -f "$stack/docker-compose.yml" ]; then
    cd "$stack"
    docker compose config 2>/dev/null |
        awk -v vault="$vault" '{ line = $0; sub(/^[ \t-]*source:[ \t]*/, "", line); sub(/\/$/, "", line); if ($0 ~ /source:/ && line == vault) found = 1 } END { exit !found }' ||
        fail "the stack in $stack serves another vault. Give this vault a directory of its own with --node-dir"
    if [ -n "$(docker compose ps --status running -q 2>/dev/null)" ]; then
        docker compose stop >/dev/null 2>&1
        echo "   stopped the daemon in $stack while the setup runs"
    fi
fi
REMOTE
    say "$host can run the daemon (Docker, $vault)"
    if [ -n "$url" ]; then
        server_endpoint "$url"
        pin="$(server_pin "$url")" || exit 1
        if [ -n "$pin" ]; then
            say "$host does not know the name $server_name: its daemon will use $pin, the address the name has here"
        else
            say "$host reaches $server_name"
        fi
    fi
    ;;

install)
    [ $# -eq 5 ] || die "usage: node.sh install <ssh-host> <vault-on-host> <stack-dir> <local-vault>"
    vault="${3%/}"
    stack="$4"
    local_vault="$5"
    IFS= read -r url || die "no CalDAV URL on standard input"
    IFS= read -r username || die "no username on standard input"
    IFS= read -r password || die "no password on standard input"
    case "$vault" in *"'"* | *$'\n'*) die "the vault path on the server must not contain a quote or a line break" ;; esac

    # Completion dates are stamped in local time: the zone is the user's, i.e. this
    # machine's, whatever the server's clock is set to.
    zone="${TZ:-$(timedatectl show -p Timezone --value 2>/dev/null || true)}"
    if [ -z "$zone" ]; then
        zone="$(readlink -f /etc/localtime 2>/dev/null | sed -n 's|.*/zoneinfo/||p')"
    fi
    zone="${zone:-Etc/UTC}"

    remote "$vault" "$stack" "$zone" <<'REMOTE' || die "could not create the stack on $host"
set -e
vault="$1"
stack="$2"
zone="$3"
mkdir -p "$stack/data"
cd "$stack"
owner="$(stat -c '%u:%g' "$vault")"
if [ ! -f docker-compose.yml ]; then
    # The daemon runs as the owner of the vault's files and writes its config as that user.
    # The stack is named after its directory: project, container and image are its own.
    project="$(basename "$PWD" | tr '[:upper:]' '[:lower:]' | sed 's/[^a-z0-9_-]/-/g; s/^[^a-z0-9]*//')"
    printf "RESTASK_VAULT_DIR='%s'\nRESTASK_USER=%s\nTZ=%s\nCOMPOSE_PROJECT_NAME=%s\n" "$vault" "$owner" "$zone" "${project:-restask}" >.env
fi
if [ "$(stat -c '%u:%g' data)" != "$owner" ]; then
    chown "$owner" data 2>/dev/null || {
        echo "node: $stack/data must belong to $owner, the owner of $vault (chown it as root)" >&2
        exit 1
    }
fi
REMOTE
    if ! remote_in "$stack" test -f docker-compose.yml </dev/null; then
        remote_in "$stack" sh -c 'cat >docker-compose.yml' <contrib/docker/docker-compose.yml
        say "created the stack $host:$stack"
    fi
    # Before the build: a server the host cannot reach is known in seconds.
    server_endpoint "$url"
    pin="$(server_pin "$url")" || exit 1
    pin_server "$stack" "$pin"
    if [ -n "$pin" ]; then
        say "$host does not know the name $server_name: the daemon uses $pin, the address the name has here"
    fi
    ship_sources "$stack"
    build_image "$stack"

    # The join makes a pass over the host's copy of the vault. It must be the copy this
    # machine just synced: a pass over files still on their way would register and pull
    # into stale notes, and the file sync would meet two versions of them.
    limit="${RESTASK_NODE_WAIT:-300}"
    mine="$(cd "$local_vault" && bash -c "$manifest")"
    waited=0
    while :; do
        theirs="$(remote_in "$vault" bash -c "$manifest" </dev/null)" || die "cannot read $vault on $host"
        [ "$mine" = "$theirs" ] && break
        if [ "$waited" -ge "$limit" ]; then
            printf 'node: after %s s the vault on %s is still not the one here. Files that differ:\n' "$limit" "$host" >&2
            diff <(printf '%s\n' "$mine") <(printf '%s\n' "$theirs") | sed -n 's/^[<>] [0-9a-f]*  /    /p' | sort -u | head -n 10 >&2
            die "is the file sync running on both machines, and does it carry .restask/ ?"
        fi
        if [ "$waited" -eq 0 ]; then say "waiting for the file sync to deliver the vault to $host:$vault"; fi
        sleep 3
        waited=$((waited + 3))
        mine="$(cd "$local_vault" && bash -c "$manifest")"
    done
    say "the vault on $host is the one here"

    say "connecting the daemon to $url"
    printf '%s\n' "$password" |
        remote_in "$stack" docker compose run --rm -T restask \
            restask setup --join --non-interactive --url "$url" --username "$username" --password-stdin ||
        die "the daemon could not join the vault on $host (can the server reach $url?)"
    start_daemon "$stack"
    say "$host: the daemon is running"
    ;;

update)
    stack="${3:-restask}"
    remote "$stack" <<<'test -f "$1/docker-compose.yml"' ||
        die "$host:$stack has no docker-compose.yml: install the daemon there first (\`restask setup --join --node $host\`, docs/INSTALL-AI.md)"
    ship_sources "$stack"
    build_image "$stack"
    start_daemon "$stack"
    say "$host: restask is running the sources of this tree"
    ;;

*)
    die "unknown action \`$action\` (stack, check, install, update)"
    ;;
esac
