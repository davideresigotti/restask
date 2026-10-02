#!/usr/bin/env bash
# The sync node — the one machine that runs the restask daemon (spec §1.1) — as a Docker
# compose stack on an always-on server, driven from here over ssh (INSTALL.md, spec
# App. E). `restask setup` runs `check` and `install`; `contrib/update.sh` runs `update`.
#
#   contrib/node.sh check   <ssh-host> <vault-on-host> [<stack-dir>]
#       The host is reachable, has Docker with compose, holds the vault folder, and
#       <stack-dir> is free or already serves that vault. A daemon running there is
#       stopped: setup is about to change the vault.
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
# <stack-dir> defaults to `restask`, relative to the ssh user's home directory. The
# stack holds  docker-compose.yml  .env  src/  data/ ; install and update replace src/
# and the image, install also writes data/ (the machine config) through the join.
# A stack's own docker-compose.yml and .env are never rewritten once they exist.
set -euo pipefail

cd "$(dirname "$0")/.."

die() { printf 'node: %s\n' "$*" >&2; exit 1; }
say() { printf '   %s\n' "$*"; }

[ $# -ge 2 ] || die "usage: node.sh check|install|update <ssh-host> … (see the head of this file)"
action="$1"
host="$2"

# One ssh connection for the whole run: whatever ssh asks (a passphrase, a password) is
# asked once.
control="$(mktemp -d)"
ssh_opts=(-o ControlMaster=auto -o "ControlPath=$control/%C" -o ControlPersist=60)
cleanup() {
    ssh "${ssh_opts[@]}" -O exit "$host" 2>/dev/null || true
    rm -rf "$control"
}
trap cleanup EXIT

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
check)
    [ $# -ge 3 ] || die "usage: node.sh check <ssh-host> <vault-on-host> [<stack-dir>]"
    vault="${3%/}"
    stack="${4:-restask}"
    ssh "${ssh_opts[@]}" "$host" true || die "cannot reach \`$host\` over ssh (does \`ssh $host\` work?)"
    remote "$vault" "$stack" <<'REMOTE'
vault="$1"
stack="$2"
fail() { printf 'node: %s\n' "$*" >&2; exit 1; }
docker compose version >/dev/null 2>&1 ||
    fail "this server has no Docker with the compose plugin. Install restask on it by hand (INSTALL.md, \"A server without Docker\") and run setup here with --no-daemon"
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
    printf "RESTASK_VAULT_DIR='%s'\nRESTASK_USER=%s\nTZ=%s\n" "$vault" "$owner" "$zone" >.env
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
        die "$host:$stack has no docker-compose.yml: install the daemon there first (\`restask setup --join --node $host\`, INSTALL.md)"
    ship_sources "$stack"
    build_image "$stack"
    start_daemon "$stack"
    say "$host: restask is running the sources of this tree"
    ;;

*)
    die "unknown action \`$action\` (check, install, update)"
    ;;
esac
