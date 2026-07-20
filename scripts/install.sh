#!/bin/sh
# Install the Duckle headless runner.
#
#   curl -fsSL https://duckle.org/install.sh | sh
#
# Downloads the duckle-runner binary for this OS and architecture from a
# GitHub release, verifies it against the release's SHA256SUMS.txt, and
# installs it as `duckle` on PATH.
#
# This installs the ~20 MB CLI, not the desktop app. It is what CI, cron and
# containers need. The desktop studio is a separate download.
#
# Environment:
#   DUCKLE_VERSION   release tag to install (default: latest)
#   DUCKLE_INSTALL   install directory (default: ~/.duckle/bin)
#   DUCKLE_NO_MODIFY_PATH  set to 1 to skip the PATH hint
set -eu

REPO="slothflowlabs/duckle"
INSTALL_DIR="${DUCKLE_INSTALL:-$HOME/.duckle/bin}"
VERSION="${DUCKLE_VERSION:-latest}"

die() { printf 'duckle install: %s\n' "$1" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "$1 is required but not installed"; }

need uname
need mkdir
need chmod

# curl or wget, whichever exists.
if command -v curl >/dev/null 2>&1; then
    fetch() { curl -fsSL "$1" -o "$2"; }
    fetch_stdout() { curl -fsSL "$1"; }
elif command -v wget >/dev/null 2>&1; then
    fetch() { wget -qO "$2" "$1"; }
    fetch_stdout() { wget -qO- "$1"; }
else
    die "either curl or wget is required"
fi

os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
    Linux)  os_part="linux" ;;
    Darwin) os_part="macos" ;;
    MINGW*|MSYS*|CYGWIN*)
        die "Windows detected. Download duckle-runner-windows-x64.exe from
     https://github.com/$REPO/releases and put it on your PATH." ;;
    *) die "unsupported OS: $os" ;;
esac
case "$arch" in
    x86_64|amd64)  arch_part="x64" ;;
    aarch64|arm64) arch_part="arm64" ;;
    *) die "unsupported architecture: $arch" ;;
esac

asset="duckle-runner-${os_part}-${arch_part}"

if [ "$VERSION" = "latest" ]; then
    base="https://github.com/$REPO/releases/latest/download"
else
    base="https://github.com/$REPO/releases/download/$VERSION"
fi

tmp="$(mktemp -d)"
# shellcheck disable=SC2064
trap "rm -rf '$tmp'" EXIT INT TERM

printf 'Downloading %s (%s)...\n' "$asset" "$VERSION"
fetch "$base/$asset" "$tmp/$asset" || die "could not download $base/$asset"

# Verify against the release checksums. Refuse to install on a mismatch; only
# skip when no checksum tool is present, and say so rather than staying quiet.
if fetch "$base/SHA256SUMS.txt" "$tmp/SHA256SUMS.txt" 2>/dev/null; then
    expected="$(awk -v a="$asset" '$2 == a || $2 == "*"a { print $1 }' "$tmp/SHA256SUMS.txt" | head -n 1)"
    if [ -z "$expected" ]; then
        printf 'warning: %s is not listed in SHA256SUMS.txt, skipping verification\n' "$asset" >&2
    else
        if command -v sha256sum >/dev/null 2>&1; then
            actual="$(sha256sum "$tmp/$asset" | cut -d' ' -f1)"
        elif command -v shasum >/dev/null 2>&1; then
            actual="$(shasum -a 256 "$tmp/$asset" | cut -d' ' -f1)"
        else
            actual=""
            printf 'warning: no sha256sum or shasum found, skipping verification\n' >&2
        fi
        if [ -n "$actual" ]; then
            [ "$actual" = "$expected" ] || die "checksum mismatch for $asset
     expected $expected
     actual   $actual
     Refusing to install. Please report this."
            printf 'Checksum verified.\n'
        fi
    fi
else
    printf 'warning: could not fetch SHA256SUMS.txt, skipping verification\n' >&2
fi

mkdir -p "$INSTALL_DIR"
mv "$tmp/$asset" "$INSTALL_DIR/duckle"
chmod +x "$INSTALL_DIR/duckle"

printf 'Installed duckle to %s/duckle\n' "$INSTALL_DIR"

# Duckle compiles pipelines to SQL and executes them on the DuckDB CLI. Say so
# plainly if it is not reachable, rather than letting the first run fail.
if ! command -v duckdb >/dev/null 2>&1 && [ -z "${DUCKLE_DUCKDB_BIN:-}" ]; then
    printf '\nNote: no `duckdb` on PATH and DUCKLE_DUCKDB_BIN is unset.\n'
    printf '`duckle validate` works without it, but `duckle --pipeline ...` needs it:\n'
    printf '  pip install duckdb-cli    # or download from https://duckdb.org/docs/installation/\n'
fi

case ":${PATH}:" in
    *":$INSTALL_DIR:"*) ;;
    *)
        if [ "${DUCKLE_NO_MODIFY_PATH:-0}" != "1" ]; then
            printf '\nAdd it to your PATH:\n  export PATH="%s:$PATH"\n' "$INSTALL_DIR"
        fi
        ;;
esac

printf '\nTry:  duckle --help\n'
