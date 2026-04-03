#!/usr/bin/env sh
# =============================================================================
# Nexa Language — Installer
#
# Usage:
#   curl --proto '=https' --tlsv1.2 -sSf \
#     https://raw.githubusercontent.com/nexa-lang-org/nexa-lang/main/setup.sh | sh
#
# Options (pass after '--'):
#   --channel <stable|snapshot|latest>   Release channel  (default: stable)
#   --version <v1.2.3>                   Pin a specific version (stable only)
#   --install-dir <path>                 Install directory (default: ~/.nexa/bin)
#   --no-modify-path                     Skip shell config updates
#   --force                              Reinstall even if already up-to-date
#
# Examples:
#   … | sh -s -- --channel snapshot
#   … | sh -s -- --version v0.2.0
#   … | sh -s -- --install-dir /usr/local/bin --no-modify-path
# =============================================================================
set -eu

# ── repository ──────────────────────────────────────────────────────────────
NEXA_REPO="nexa-lang-org/nexa-lang"
NEXA_RELEASES="https://github.com/$NEXA_REPO/releases"

# ── defaults ────────────────────────────────────────────────────────────────
CHANNEL="stable"
VERSION=""
INSTALL_DIR="${NEXA_HOME:-$HOME/.nexa}/bin"
MODIFY_PATH=1
FORCE=0

# ── argument parsing ─────────────────────────────────────────────────────────
while [ $# -gt 0 ]; do
    case "$1" in
        --channel)        CHANNEL="$2";      shift ;;
        --channel=*)      CHANNEL="${1#*=}"  ;;
        --version)        VERSION="$2";      shift ;;
        --version=*)      VERSION="${1#*=}"  ;;
        --install-dir)    INSTALL_DIR="$2";  shift ;;
        --install-dir=*)  INSTALL_DIR="${1#*=}" ;;
        --no-modify-path) MODIFY_PATH=0      ;;
        --force|-f)       FORCE=1            ;;
        -h|--help)
            sed -n '/^# Usage:/,/^# =====/p' "$0" | grep '^#' | sed 's/^# \?//'
            exit 0
            ;;
        *) _nx_err "unknown argument: $1 (run with --help)" ;;
    esac
    shift
done

# ── terminal colours ─────────────────────────────────────────────────────────
# Only enable if stdout is a terminal
if [ -t 1 ]; then
    CLR_GREEN='\033[1;32m'
    CLR_YELLOW='\033[1;33m'
    CLR_RED='\033[1;31m'
    CLR_CYAN='\033[1;36m'
    CLR_BOLD='\033[1m'
    CLR_RESET='\033[0m'
else
    CLR_GREEN=''; CLR_YELLOW=''; CLR_RED=''; CLR_CYAN=''; CLR_BOLD=''; CLR_RESET=''
fi

# ── helpers ──────────────────────────────────────────────────────────────────
_nx_say()  { printf "${CLR_GREEN}==>${CLR_RESET} ${CLR_BOLD}%s${CLR_RESET}\n" "$*"; }
_nx_info() { printf "   ${CLR_CYAN}%s${CLR_RESET}\n" "$*"; }
_nx_warn() { printf "${CLR_YELLOW}warn:${CLR_RESET} %s\n" "$*" >&2; }
_nx_err()  { printf "${CLR_RED}error:${CLR_RESET} %s\n" "$*" >&2; exit 1; }

_nx_need() {
    command -v "$1" > /dev/null 2>&1 || \
        _nx_err "required command '$1' not found — please install it first"
}

# ── input validation ─────────────────────────────────────────────────────────
case "$CHANNEL" in
    stable|latest|snapshot) ;;
    *) _nx_err "unknown channel '$CHANNEL' — choose: stable, latest, snapshot" ;;
esac

if [ -n "$VERSION" ] && [ "$CHANNEL" = "snapshot" ]; then
    _nx_err "--version cannot be combined with --channel snapshot"
fi

if [ -n "$VERSION" ]; then
    # Normalise: add leading 'v' if missing
    case "$VERSION" in v*) ;; *) VERSION="v$VERSION" ;; esac
fi

# ── platform detection ───────────────────────────────────────────────────────
_NX_OS=$(uname -s)
_NX_ARCH=$(uname -m)

case "$_NX_OS" in
    Linux)  _NX_OS_TYPE="linux"  ;;
    Darwin) _NX_OS_TYPE="macos"  ;;
    *)
        _nx_err "unsupported OS '$_NX_OS'
       Supported: Linux, macOS
       Windows users: download the .zip from
       $NEXA_RELEASES/latest"
        ;;
esac

case "$_NX_ARCH" in
    x86_64|amd64)   _NX_ARCH_TYPE="x86_64"  ;;
    aarch64|arm64)  _NX_ARCH_TYPE="aarch64" ;;
    *)
        _nx_err "unsupported architecture '$_NX_ARCH'
       Supported: x86_64, aarch64"
        ;;
esac

_NX_ASSET_STEM="nexa-${_NX_OS_TYPE}-${_NX_ARCH_TYPE}"
_NX_ARCHIVE="${_NX_ASSET_STEM}.tar.gz"
_NX_CHECKSUM="${_NX_ARCHIVE}.sha256"

# ── resolve download URL ─────────────────────────────────────────────────────
case "$CHANNEL" in
    stable|latest)
        if [ -n "$VERSION" ]; then
            _NX_BASE_URL="$NEXA_RELEASES/download/$VERSION"
        else
            _NX_BASE_URL="$NEXA_RELEASES/latest/download"
        fi
        ;;
    snapshot)
        _NX_BASE_URL="$NEXA_RELEASES/download/snapshot"
        ;;
esac

_NX_DOWNLOAD_URL="$_NX_BASE_URL/$_NX_ARCHIVE"
_NX_CHECKSUM_URL="$_NX_BASE_URL/$_NX_CHECKSUM"

# ── network fetch (curl preferred, wget fallback) ────────────────────────────
_nx_fetch() {
    _nx_fetch_url="$1"
    _nx_fetch_dest="$2"
    if command -v curl > /dev/null 2>&1; then
        curl --proto '=https' --tlsv1.2 -sSfL "$_nx_fetch_url" -o "$_nx_fetch_dest"
    elif command -v wget > /dev/null 2>&1; then
        wget --https-only -q "$_nx_fetch_url" -O "$_nx_fetch_dest"
    else
        _nx_err "neither curl nor wget found — please install one first"
    fi
}

# ── checksum verification ────────────────────────────────────────────────────
_nx_verify() {
    _nx_v_archive="$1"
    _nx_v_sum_file="$2"
    _nx_say "Verifying SHA-256 checksum..."
    (
        cd "$(dirname "$_nx_v_archive")"
        if command -v sha256sum > /dev/null 2>&1; then
            sha256sum -c "$_nx_v_sum_file" --status \
                || _nx_err "checksum mismatch — the download may be corrupted or tampered with"
        elif command -v shasum > /dev/null 2>&1; then
            shasum -a 256 -c "$_nx_v_sum_file" --status \
                || _nx_err "checksum mismatch — the download may be corrupted or tampered with"
        else
            _nx_warn "sha256sum/shasum not found — skipping integrity check"
        fi
    )
    _nx_info "Checksum OK"
}

# ── temp directory (cleaned up on exit) ─────────────────────────────────────
_NX_TMP=$(mktemp -d 2>/dev/null || mktemp -d -t 'nexa-install')
trap 'rm -rf "$_NX_TMP"' EXIT INT TERM

# ── banner ───────────────────────────────────────────────────────────────────
printf "\n"
printf "  ${CLR_BOLD}Nexa Language Installer${CLR_RESET}\n"
printf "  ────────────────────────────────────────────────────\n"
_nx_info "Channel     : $CHANNEL${VERSION:+ ($VERSION)}"
_nx_info "Platform    : $_NX_OS_TYPE / $_NX_ARCH_TYPE"
_nx_info "Install dir : $INSTALL_DIR"
printf "\n"

# ── check existing installation ──────────────────────────────────────────────
_NX_EXISTING_VERSION=""
if command -v nexa > /dev/null 2>&1; then
    _NX_EXISTING_VERSION=$(nexa --version 2>/dev/null | head -1 || true)
fi

if [ -n "$_NX_EXISTING_VERSION" ] && [ "$FORCE" = "0" ]; then
    _nx_info "Found existing installation: $_NX_EXISTING_VERSION"
    if [ "$CHANNEL" = "stable" ] || [ "$CHANNEL" = "latest" ]; then
        _nx_info "Use --force to reinstall, or --channel snapshot for dev builds."
    fi
fi

# ═════════════════════════════════════════════════════════════════════════════
# STEP 1 — try prebuilt binary
# ═════════════════════════════════════════════════════════════════════════════
_NX_PREBUILT_OK=0

_nx_say "Downloading prebuilt binary..."
_nx_info "$_NX_DOWNLOAD_URL"

if  _nx_fetch "$_NX_DOWNLOAD_URL" "$_NX_TMP/$_NX_ARCHIVE" 2>/dev/null && \
    _nx_fetch "$_NX_CHECKSUM_URL"  "$_NX_TMP/$_NX_CHECKSUM"  2>/dev/null
then
    _nx_verify "$_NX_TMP/$_NX_ARCHIVE" "$_NX_TMP/$_NX_CHECKSUM"

    _nx_say "Installing..."
    mkdir -p "$INSTALL_DIR"
    tar -xzf "$_NX_TMP/$_NX_ARCHIVE" -C "$_NX_TMP"
    mv "$_NX_TMP/nexa" "$INSTALL_DIR/nexa"
    chmod 755 "$INSTALL_DIR/nexa"
    _NX_PREBUILT_OK=1
fi

# ═════════════════════════════════════════════════════════════════════════════
# STEP 2 — fallback: build from source
# ═════════════════════════════════════════════════════════════════════════════
if [ "$_NX_PREBUILT_OK" = "0" ]; then
    _nx_warn "No prebuilt binary available for $_NX_OS_TYPE/$_NX_ARCH_TYPE."
    printf "\n"
    _nx_say "Falling back to building from source..."
    printf "\n"

    # ── ensure Rust is installed ──────────────────────────────────────────────
    if ! command -v cargo > /dev/null 2>&1; then
        _nx_say "Rust not found — installing rustup..."
        _NX_RUSTUP_INIT="$_NX_TMP/rustup-init.sh"
        _nx_fetch "https://sh.rustup.rs" "$_NX_RUSTUP_INIT"
        chmod +x "$_NX_RUSTUP_INIT"
        "$_NX_RUSTUP_INIT" -y --no-modify-path --default-toolchain stable \
            || _nx_err "rustup installation failed — visit https://rustup.rs"
        # source the cargo env for the rest of this script
        # shellcheck source=/dev/null
        . "$HOME/.cargo/env"
    fi

    _nx_need cargo
    _nx_need git

    _nx_say "Cloning Nexa repository..."
    git clone --quiet --depth 1 "https://github.com/$NEXA_REPO.git" "$_NX_TMP/nexa-src" \
        || _nx_err "failed to clone repository"

    # Checkout the requested tag/version
    if [ -n "$VERSION" ]; then
        git -C "$_NX_TMP/nexa-src" checkout --quiet "$VERSION" \
            || _nx_err "version '$VERSION' not found in the repository"
    elif [ "$CHANNEL" != "snapshot" ]; then
        # Checkout the latest stable git tag
        _NX_LATEST_TAG=$(git -C "$_NX_TMP/nexa-src" tag -l 'v*' 2>/dev/null \
            | sort -t. -k1,1V -k2,2V -k3,3V | tail -1)
        if [ -n "$_NX_LATEST_TAG" ]; then
            git -C "$_NX_TMP/nexa-src" checkout --quiet "$_NX_LATEST_TAG"
        fi
    fi

    _nx_say "Compiling (this may take a few minutes)..."
    cargo install \
        --path "$_NX_TMP/nexa-src/crates/cli" \
        --root "$(dirname "$INSTALL_DIR")" \
        --locked \
        --quiet \
        || _nx_err "compilation failed — check the output above for details"
fi

# ═════════════════════════════════════════════════════════════════════════════
# STEP 3 — verify the installed binary
# ═════════════════════════════════════════════════════════════════════════════
_NX_BIN="$INSTALL_DIR/nexa"

if ! "$_NX_BIN" --version > /dev/null 2>&1; then
    _nx_err "installation succeeded but the binary does not run — please report this at
       https://github.com/$NEXA_REPO/issues"
fi

_NX_VERSION=$("$_NX_BIN" --version 2>&1 | head -1)

# ═════════════════════════════════════════════════════════════════════════════
# STEP 4 — make `nexa` available everywhere
#
# Strategy (in order of preference):
#
#  a) Symlink into a directory that is ALREADY in $PATH
#     → nexa works immediately in the current terminal, no action needed
#
#  b) Create ~/.nexa/env and source it from shell config files
#     → nexa works in every future terminal automatically
#     → one short command activates it in the current session
#
# The two steps are independent and complementary.
# ═════════════════════════════════════════════════════════════════════════════
_nx_say "Configuring environment..."

# ── 4a. Create ~/.nexa/env (sourceable, like ~/.cargo/env) ──────────────────
#
#   This file is idempotent: sourcing it multiple times is harmless.
#   Shell configs will `. ~/.nexa/env` so users only need one line per config.
#
_NX_ENV_FILE="$HOME/.nexa/env"
mkdir -p "$(dirname "$_NX_ENV_FILE")"
cat > "$_NX_ENV_FILE" << 'ENVEOF'
#!/usr/bin/env sh
# Nexa language — source this file to add nexa to your PATH.
# Added automatically by setup.sh. Safe to source multiple times.
case ":${PATH}:" in
    *":${HOME}/.nexa/bin:"*) ;;
    *) export PATH="${HOME}/.nexa/bin:${PATH}" ;;
esac
ENVEOF
chmod +x "$_NX_ENV_FILE"

# ── 4b. Symlink into an existing PATH directory (current terminal fix) ───────
#
#   We try a known priority list. The first candidate that is:
#     - present in $PATH (confirmed accessible by the shell)
#     - writable by the current user
#   gets a symlink:  <dir>/nexa → ~/.nexa/bin/nexa
#
#   Why a symlink?  The real binary stays in ~/.nexa/bin so `nexa update`
#   always knows where to replace it, and the symlink follows automatically.
#
_NX_SYMLINK_DIR=""
_NX_SYMLINK_OK=0

if [ "$MODIFY_PATH" = "1" ]; then
    for _nx_candidate in \
        "/usr/local/bin" \
        "$HOME/.local/bin" \
        "$HOME/bin" \
        "/opt/homebrew/bin"
    do
        # Is this directory present in the current $PATH?
        case ":${PATH}:" in
            *":${_nx_candidate}:"*) ;;   # yes → continue
            *) continue ;;               # no  → skip
        esac

        # Is it writable?
        if [ -w "$_nx_candidate" ] 2>/dev/null; then
            # Create (or update) the symlink
            if ln -sf "$_NX_BIN" "$_nx_candidate/nexa" 2>/dev/null; then
                _NX_SYMLINK_DIR="$_nx_candidate"
                _NX_SYMLINK_OK=1
                _nx_info "Symlinked: $_nx_candidate/nexa → $_NX_BIN"
                break
            fi
        fi
    done
fi

# ── 4c. Update shell config files ────────────────────────────────────────────
#
#   We add a single line — `. "$HOME/.nexa/env"` — to each shell config that
#   exists.  The env file is idempotent, so re-sourcing is safe.
#   Idempotency check: skip if the file already mentions .nexa/env.
#
_nx_add_source_line() {
    _nx_asf_file="$1"
    [ -f "$_nx_asf_file" ] || return 0
    grep -qF '.nexa/env' "$_nx_asf_file" 2>/dev/null && return 0
    printf '\n# Nexa language — added by setup.sh\n. "$HOME/.nexa/env"\n' \
        >> "$_nx_asf_file"
}

_NX_CONFIGS_UPDATED=0

if [ "$MODIFY_PATH" = "1" ]; then
    # bash
    for _nx_f in "$HOME/.bashrc" "$HOME/.bash_profile" "$HOME/.profile"; do
        if [ -f "$_nx_f" ] && ! grep -qF '.nexa/env' "$_nx_f" 2>/dev/null; then
            _nx_add_source_line "$_nx_f"
            _nx_info "Updated $_nx_f"
            _NX_CONFIGS_UPDATED=1
        fi
    done

    # zsh
    if [ -f "$HOME/.zshrc" ] && ! grep -qF '.nexa/env' "$HOME/.zshrc" 2>/dev/null; then
        _nx_add_source_line "$HOME/.zshrc"
        _nx_info "Updated $HOME/.zshrc"
        _NX_CONFIGS_UPDATED=1
    fi

    # zsh — create .zshrc if it doesn't exist yet (common on fresh macOS)
    if [ ! -f "$HOME/.zshrc" ] && [ "$(basename "${SHELL:-}")" = "zsh" ]; then
        printf '# Nexa language — added by setup.sh\n. "$HOME/.nexa/env"\n' \
            > "$HOME/.zshrc"
        _nx_info "Created $HOME/.zshrc"
        _NX_CONFIGS_UPDATED=1
    fi

    # fish
    _NX_FISH_CFG="$HOME/.config/fish/conf.d/nexa.fish"
    if command -v fish > /dev/null 2>&1 && [ ! -f "$_NX_FISH_CFG" ]; then
        mkdir -p "$(dirname "$_NX_FISH_CFG")"
        printf '# Nexa language — added by setup.sh\nfish_add_path "$HOME/.nexa/bin"\n' \
            > "$_NX_FISH_CFG"
        _nx_info "Created $_NX_FISH_CFG"
        _NX_CONFIGS_UPDATED=1
    fi
fi

# ═════════════════════════════════════════════════════════════════════════════
# Done
# ═════════════════════════════════════════════════════════════════════════════
printf "\n"
printf "  ${CLR_GREEN}✓${CLR_RESET} ${CLR_BOLD}Nexa installed successfully!${CLR_RESET}\n"
printf "\n"
printf "  %-14s %s\n" "Version:"   "$_NX_VERSION"
printf "  %-14s %s\n" "Channel:"   "$CHANNEL"
printf "  %-14s %s\n" "Binary:"    "$_NX_BIN"
if [ "$_NX_SYMLINK_OK" = "1" ]; then
    printf "  %-14s %s\n" "Also linked:" "$_NX_SYMLINK_DIR/nexa"
fi
printf "\n"

if [ "$_NX_SYMLINK_OK" = "1" ]; then
    # Best case: already in PATH via symlink — works right now
    printf "  ${CLR_GREEN}✓${CLR_RESET} ${CLR_BOLD}nexa is ready to use in this terminal right now.${CLR_RESET}\n"
    printf "\n"
    printf "    ${CLR_BOLD}nexa --version${CLR_RESET}\n"
    printf "    ${CLR_BOLD}nexa init my-app${CLR_RESET}\n"
else
    # Fallback: needs one command to activate in the current session
    printf "  ${CLR_YELLOW}One command to activate nexa in this terminal:${CLR_RESET}\n"
    printf "\n"
    printf "    ${CLR_CYAN}. \"\$HOME/.nexa/env\"${CLR_RESET}\n"
    printf "\n"
    if [ "$_NX_CONFIGS_UPDATED" = "1" ]; then
        printf "  All future terminals are already configured automatically.\n"
    fi
    printf "\n"
    printf "  Then try:\n"
    printf "    ${CLR_BOLD}nexa --version${CLR_RESET}\n"
    printf "    ${CLR_BOLD}nexa init my-app${CLR_RESET}\n"
fi

printf "\n"
printf "  Docs   → https://github.com/$NEXA_REPO#readme\n"
printf "  Issues → https://github.com/$NEXA_REPO/issues\n"
printf "\n"
