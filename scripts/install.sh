#!/bin/sh
set -eu

repository=${ARASEO_REPOSITORY:-mincheol-lee/araseo}
release_version=latest
uninstall=false

usage() {
    cat <<'EOF'
Usage: install.sh [--version TAG] [--uninstall]

Install or update Araseo for the current WSL user. By default, the newest
stable GitHub release is installed.

Options:
  --version TAG  Install a specific release, for example v0.1.9
  --uninstall    Remove Araseo from the current Windows and WSL user
  -h, --help     Show this help
EOF
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --version)
            [ "$#" -ge 2 ] || { echo "--version requires a tag" >&2; exit 2; }
            release_version=$2
            shift 2
            ;;
        --uninstall)
            uninstall=true
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "Unknown option: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

bin_directory=${ARASEO_BIN_DIR:-$HOME/.local/bin}
config_directory=${XDG_CONFIG_HOME:-$HOME/.config}/araseo
config_file=${ARASEO_CONFIG_FILE:-$config_directory/executable}

windows_install_directory() {
    if [ -n "${ARASEO_INSTALL_DIR:-}" ]; then
        printf '%s\n' "$ARASEO_INSTALL_DIR"
        return
    fi

    if ! command -v powershell.exe >/dev/null 2>&1 || ! command -v wslpath >/dev/null 2>&1; then
        echo "Cannot locate Windows LocalAppData from WSL." >&2
        echo "Set ARASEO_INSTALL_DIR to a Windows-accessible directory and try again." >&2
        exit 1
    fi

    windows_local_app_data=$(powershell.exe -NoProfile -NonInteractive -Command \
        '[Environment]::GetFolderPath([Environment+SpecialFolder]::LocalApplicationData)' \
        </dev/null 2>/dev/null | tr -d '\r' | sed -n '1p')
    [ -n "$windows_local_app_data" ] || {
        echo "Windows returned an empty LocalAppData path." >&2
        exit 1
    }
    local_app_data=$(wslpath -u "$windows_local_app_data")
    printf '%s/Programs/Araseo\n' "$local_app_data"
}

install_directory=$(windows_install_directory)
executable_path=$install_directory/araseo.exe
launcher_path=$bin_directory/araseo

if [ "$uninstall" = true ]; then
    if [ -r "$config_file" ]; then
        IFS= read -r configured_executable < "$config_file"
        if [ -n "$configured_executable" ]; then
            executable_path=$configured_executable
        fi
    fi

    rm -f -- "$executable_path" "$launcher_path" "$config_file"
    rmdir -- "$config_directory" 2>/dev/null || true
    rmdir -- "$install_directory" 2>/dev/null || true
    echo "Araseo was removed for the current user."
    exit 0
fi

for command_name in curl sha256sum install mktemp; do
    command -v "$command_name" >/dev/null 2>&1 || {
        echo "Required command not found: $command_name" >&2
        exit 1
    }
done

if [ -n "${ARASEO_RELEASE_URL:-}" ]; then
    release_url=$ARASEO_RELEASE_URL
elif [ "$release_version" = latest ]; then
    release_url=https://github.com/$repository/releases/latest/download
else
    release_url=https://github.com/$repository/releases/download/$release_version
fi

temporary_directory=$(mktemp -d "${TMPDIR:-/tmp}/araseo-install.XXXXXX")
cleanup() {
    rm -rf -- "$temporary_directory"
}
trap cleanup EXIT HUP INT TERM

for asset in araseo.exe araseo SHA256SUMS; do
    echo "Downloading $asset..."
    curl --fail --location --silent --show-error \
        "$release_url/$asset" --output "$temporary_directory/$asset"
done

verify_asset() {
    asset=$1
    expected=$(awk -v asset="$asset" '{ sub(/\r$/, "", $2); if ($2 == asset || $2 == "*" asset) { print $1; exit } }' \
        "$temporary_directory/SHA256SUMS")
    if ! printf '%s\n' "$expected" | grep -Eq '^[0-9a-fA-F]{64}$'; then
        echo "No valid checksum found for $asset." >&2
        exit 1
    fi
    printf '%s  %s\n' "$expected" "$asset" | \
        (cd "$temporary_directory" && sha256sum --check --status -) || {
            echo "Checksum verification failed for $asset." >&2
            exit 1
        }
}

verify_asset araseo.exe
verify_asset araseo

install -d "$install_directory" "$bin_directory" "$config_directory"
install -m 755 "$temporary_directory/araseo.exe" "$executable_path"
install -m 755 "$temporary_directory/araseo" "$launcher_path"

config_temporary=$config_file.tmp.$$
printf '%s\n' "$executable_path" > "$config_temporary"
chmod 600 "$config_temporary"
mv -f -- "$config_temporary" "$config_file"

echo "Installed Araseo $release_version."
echo "  Windows executable: $executable_path"
echo "  WSL command:        $launcher_path"
case :$PATH: in
    *:"$bin_directory":*) ;;
    *) echo "Add $bin_directory to PATH, then run: araseo ." ;;
esac
