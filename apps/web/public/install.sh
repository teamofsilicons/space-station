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
    base=${SPACE_STATION_DOWNLOAD_URL:-https://spacestation.teamofsilicons.com/downloads}
    root="$HOME/.local/share/spacestation"
    bin="$HOME/.local/bin"
    temp=$(mktemp -d "${TMPDIR:-/tmp}/spacestation.XXXXXXXX")
    trap 'rm -rf "$temp"' EXIT HUP INT TERM
    artifact="spacestation-$platform-$arch.tar.gz"
    printf 'Downloading spacestation for %s/%s…\n' "$platform" "$arch"
    fetch "$base/$artifact" "$temp/$artifact"
    fetch "$base/SHA256SUMS" "$temp/SHA256SUMS"
    verify "$temp/$artifact" "$artifact" "$temp/SHA256SUMS"
    tar -xzf "$temp/$artifact" -C "$temp"
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
        tar -xzf "$temp/node.tar.gz" -C "$temp"
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
    "$bin/spacestation" --version
    printf '\nInstalled at %s/spacestation\n' "$bin"
    printf 'Run: spacestation login --org <your-org>\n'
    case :$PATH: in
        *:"$bin":*) ;;
        *) printf 'For this terminal: export PATH="$HOME/.local/bin:$PATH"\n' ;;
    esac
}
install_spacestation
