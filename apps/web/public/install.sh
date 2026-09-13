#!/bin/sh
# Space Station installer. Everything runs only after the complete script has arrived.
set -eu

install_spacestation() {
    die() { printf 'spacestation: %s\n' "$*" >&2; exit 1; }
    fetch() {
        if command -v curl >/dev/null 2>&1; then
            curl --proto '=https' --tlsv1.2 -fsSL --retry 3 "$1" -o "$2"
        elif command -v wget >/dev/null 2>&1; then
            wget -qO "$2" "$1"
        else
            die 'curl or wget is required to download the installer assets.'
        fi
    }
    verify() {
        expected=$(awk -v name="$2" '$2 == name {print $1}' "$3")
        [ "${#expected}" = 64 ] || die "Missing checksum for $2"
        if command -v sha256sum >/dev/null 2>&1; then
            actual=$(sha256sum "$1" | awk '{print $1}')
        elif command -v shasum >/dev/null 2>&1; then
            actual=$(shasum -a 256 "$1" | awk '{print $1}')
        else
            die 'sha256sum or shasum is required.'
        fi
        [ "$actual" = "$expected" ] || die "Checksum mismatch for $2; nothing installed."
    }
    extract_safe() {
        archive=$1 destination=$2
        entries=$(tar -tzf "$archive") || die "Invalid archive: $(basename "$archive")"
        while IFS= read -r member; do
            case "$member" in
                /*|../*|*/../*|*/..|..) die "Unsafe archive path in $(basename "$archive")" ;;
            esac
        done <<EOF
$entries
EOF
        tar -xzf "$archive" -C "$destination" || die "Invalid archive: $(basename "$archive")"
    }
    node_ok() { "$1" -e 'const [a,b]=process.versions.node.split(".").map(Number);process.exit(a>22||(a===22&&b>=13)?0:1)' >/dev/null 2>&1; }
    quote() { printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"; }

    [ -n "${HOME:-}" ] || die 'HOME must be set.'
    case $(uname -s) in
        Darwin) platform=darwin ;;
        Linux) platform=linux ;;
        *) die 'Supported systems: macOS and Linux.' ;;
    esac
    case $(uname -m) in
        x86_64|amd64) arch=x64 ;;
        arm64|aarch64) arch=arm64 ;;
        *) die 'Supported processors: x86_64 and ARM64.' ;;
    esac
    base=${SPACE_STATION_DOWNLOAD_URL:-https://github.com/teamofsilicons/space-station/releases/latest/download}
    root="$HOME/.local/share/spacestation"
    bin="$HOME/.local/bin"
    if [ -n "${SILICON_HOME:-}" ]; then
        home_env_name=SILICON_HOME
        silicon_home=$SILICON_HOME
    elif [ -n "${SPACE_STATION_HOME:-}" ]; then
        home_env_name=SPACE_STATION_HOME
        silicon_home=$SPACE_STATION_HOME
    else
        home_env_name=SILICON_HOME
        silicon_home=$HOME/.silicon
    fi
    case ${SPACE_STATION_UPDATE:-} in
        0|false|FALSE|False) update_setting=0 ;;
        *) update_setting=1 ;;
    esac
    temp=$(mktemp -d "${TMPDIR:-/tmp}/spacestation.XXXXXXXX")
    trap 'rm -rf "$temp"' EXIT HUP INT TERM
    artifact="spacestation-$platform-$arch.tar.gz"
    printf 'Downloading spacestation for %s/%s…\n' "$platform" "$arch"
    fetch "$base/$artifact" "$temp/$artifact"
    fetch "$base/SHA256SUMS" "$temp/SHA256SUMS"
    verify "$temp/$artifact" "$artifact" "$temp/SHA256SUMS"
    # Extract the one expected member to stdout.  Extracting an untrusted tar
    # directly into a directory permits `../` members to overwrite files in the
    # user's home before the checksum or executable checks run.
    entries=$(tar -tzf "$temp/$artifact") || die "Invalid archive: $artifact"
    [ "$entries" = spacestation ] || die "Unexpected archive contents: $artifact"
    tar -xOzf "$temp/$artifact" spacestation > "$temp/spacestation" || die "Invalid archive: $artifact"
    chmod 755 "$temp/spacestation"
    "$temp/spacestation" --version || die 'This binary cannot run on this machine.'

    # A private Node runtime supports `windows run` and `windows tool`, without replacing
    # the user's system Node. Existing compatible Node installations are reused.
    if [ "${SPACE_STATION_SKIP_NODE:-0}" != 1 ] && ! node_ok "$root/node/bin/node" && ! node_ok node; then
        node_version=v22.23.2
        node_archive="node-$node_version-$platform-$arch.tar.gz"
        printf 'Installing private Node.js runtime…\n'
        fetch "https://nodejs.org/dist/$node_version/$node_archive" "$temp/node.tar.gz"
        fetch "https://nodejs.org/dist/$node_version/SHASUMS256.txt" "$temp/node-shasums"
        verify "$temp/node.tar.gz" "$node_archive" "$temp/node-shasums"
        extract_safe "$temp/node.tar.gz" "$temp"
        if node_ok "$temp/node-$node_version-$platform-$arch/bin/node"; then
            mkdir -p "$root"
            mv "$temp/node-$node_version-$platform-$arch" "$root/node-$node_version"
            ln -sfn "node-$node_version" "$root/node"
        else
            printf '%s\n' 'Node runtime is not compatible with this OS. The CLI will work; windows run/tool require Node >=22.13 on PATH.' >&2
        fi
    fi

    mkdir -p "$root/bin" "$bin"
    cp "$temp/spacestation" "$root/bin/spacestation.new"
    chmod 755 "$root/bin/spacestation.new"
    mv -f "$root/bin/spacestation.new" "$root/bin/spacestation"
    {
        printf '#!/bin/sh\nroot=%s\n' "$(quote "$root")"
        printf '%s\n' 'if [ -x "$root/node/bin/node" ]; then export PATH="$root/node/bin:$PATH"; fi' 'exec "$root/bin/spacestation" "$@"'
    } > "$bin/spacestation.new"
    chmod 755 "$bin/spacestation.new"
    mv -f "$bin/spacestation.new" "$bin/spacestation"

    # Cover login and interactive shells. Fish has its own PATH syntax.
    for profile in "$HOME/.profile" "$HOME/.bashrc" "${ZDOTDIR:-$HOME}/.zshrc"; do
        if ! grep -F '# spacestation PATH' "$profile" >/dev/null 2>&1; then
            printf '\n# spacestation PATH\nexport PATH="$HOME/.local/bin:$PATH"\n' >> "$profile"
        fi
    done
    if [ -f "$HOME/.bash_profile" ] && ! grep -F '# spacestation PATH' "$HOME/.bash_profile" >/dev/null 2>&1; then
        printf '\n# spacestation PATH\nexport PATH="$HOME/.local/bin:$PATH"\n' >> "$HOME/.bash_profile"
    fi
    case ${SHELL:-} in
        */fish)
            mkdir -p "$HOME/.config/fish/conf.d"
            printf '%s\n' 'fish_add_path "$HOME/.local/bin"' > "$HOME/.config/fish/conf.d/spacestation.fish"
            ;;
    esac
    install_daemon() {
        [ "${SPACE_STATION_SKIP_DAEMON:-0}" = 1 ] && return 0
        case $(uname -s) in
            Darwin)
                command -v launchctl >/dev/null 2>&1 || {
                    printf 'spacestation: launchctl unavailable; daemon not started\n' >&2
                    return 0
                }
                agents="$HOME/Library/LaunchAgents"
                plist="$agents/com.teamofsilicons.spacestation.daemon.plist"
                mkdir -p "$agents"
                xml() { printf '%s' "$1" | sed 's/&/\\&amp;/g; s/</\\&lt;/g; s/>/\\&gt;/g; s/"/\\&quot;/g'; }
                {
                    printf '%s\n' '<?xml version="1.0" encoding="UTF-8"?>' '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">' '<plist version="1.0"><dict>'
                    printf '%s\n' '<key>Label</key><string>com.teamofsilicons.spacestation.daemon</string>' '<key>ProgramArguments</key><array>'
                    printf '<string>%s</string>\n' "$(xml "$bin/spacestation")"
                    printf '%s\n' '<string>daemon</string>' '<string>run</string>'
                    printf '%s\n' '</array><key>EnvironmentVariables</key><dict>'
                    printf '<key>%s</key><string>%s</string>\n' "$home_env_name" "$(xml "$silicon_home")"
                    printf '%s\n' "<key>SPACE_STATION_UPDATE</key><string>$update_setting</string>"
                    printf '%s\n' '</dict><key>RunAtLoad</key><true/><key>KeepAlive</key><true/></dict></plist>'
                } > "$plist"
                uid=$(id -u)
                launchctl bootout "gui/$uid/com.teamofsilicons.spacestation.daemon" 2>/dev/null || true
                launchctl bootstrap "gui/$uid" "$plist" || die 'Could not register the macOS daemon.'
                launchctl kickstart -k "gui/$uid/com.teamofsilicons.spacestation.daemon" || die 'Could not start the macOS daemon.'
                ;;
            Linux)
                command -v systemctl >/dev/null 2>&1 || {
                    printf 'spacestation: systemctl unavailable; daemon not started\n' >&2
                    return 0
                }
                units="$HOME/.config/systemd/user"
                mkdir -p "$units"
                systemd_quote() { printf '"%s"' "$(printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\\\"/g; s/%/%%/g')"; }
                cat > "$units/spacestation.service" <<EOF
[Unit]
Description=Space Station ingest daemon
After=default.target

[Service]
ExecStart=$(systemd_quote "$bin/spacestation") daemon run
Environment=$home_env_name=$(systemd_quote "$silicon_home")
Environment=SPACE_STATION_UPDATE=$update_setting
Restart=always
RestartSec=5

[Install]
WantedBy=default.target
EOF
                systemctl --user daemon-reload || die 'Could not reload the user systemd manager.'
                systemctl --user enable --now spacestation.service || die 'Could not start the user systemd daemon.'
                ;;
        esac
    }
    install_daemon
    "$bin/spacestation" --version
    printf '\nInstalled at %s/spacestation\n' "$bin"
    printf 'Run: spacestation login --org <your-org>\n'
    case :$PATH: in
        *:"$bin":*) ;;
        *) printf 'For this terminal: export PATH="$HOME/.local/bin:$PATH"\n' ;;
    esac
}
install_spacestation
