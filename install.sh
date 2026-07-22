#!/bin/bash
set -e

# Colors for terminal output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
CYAN='\033[0;36m'
NC='\033[0m' # No Color
VERSION="0.3.9"

compare_release_versions() {
    local left="${1#v}"
    local right="${2#v}"

    if [[ ! "$left" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]]; then
        return 2
    fi
    local left_major="${BASH_REMATCH[1]}"
    local left_minor="${BASH_REMATCH[2]}"
    local left_patch="${BASH_REMATCH[3]}"

    if [[ ! "$right" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]]; then
        return 2
    fi
    local right_major="${BASH_REMATCH[1]}"
    local right_minor="${BASH_REMATCH[2]}"
    local right_patch="${BASH_REMATCH[3]}"

    if (( 10#$left_major < 10#$right_major )); then echo -1; return; fi
    if (( 10#$left_major > 10#$right_major )); then echo 1; return; fi
    if (( 10#$left_minor < 10#$right_minor )); then echo -1; return; fi
    if (( 10#$left_minor > 10#$right_minor )); then echo 1; return; fi
    if (( 10#$left_patch < 10#$right_patch )); then echo -1; return; fi
    if (( 10#$left_patch > 10#$right_patch )); then echo 1; return; fi
    echo 0
}

sha256_file() {
    local path="$1"
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$path" | awk '{print tolower($1)}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$path" | awk '{print tolower($1)}'
    else
        echo -e "${RED}Error: Neither sha256sum nor shasum was found; cannot verify files.${NC}" >&2
        return 1
    fi
}

TEMP_DIR=""
LOCK_DIR=""
cleanup_install() {
    if [ -n "$TEMP_DIR" ] && [ -d "$TEMP_DIR" ]; then
        rm -rf -- "$TEMP_DIR"
    fi
    if [ -n "$LOCK_DIR" ] && [ -d "$LOCK_DIR" ] && [ -f "$LOCK_DIR/pid" ] && [ "$(cat "$LOCK_DIR/pid" 2>/dev/null)" = "$$" ]; then
        rm -rf -- "$LOCK_DIR"
    fi
}
trap cleanup_install EXIT
trap 'exit 130' INT
trap 'exit 143' TERM HUP

acquire_install_lock() {
    LOCK_DIR="$INSTALL_DIR/.ask-bridge.install.lock.d"
    local attempts=0
    while ! mkdir "$LOCK_DIR" 2>/dev/null; do
        attempts=$((attempts + 1))
        if [ -f "$LOCK_DIR/pid" ]; then
            local owner_pid
            owner_pid="$(cat "$LOCK_DIR/pid" 2>/dev/null || true)"
            if [[ "$owner_pid" =~ ^[0-9]+$ ]] && ! kill -0 "$owner_pid" 2>/dev/null; then
                rm -rf -- "$LOCK_DIR"
                continue
            fi
        fi
        if [ "$attempts" -ge 300 ]; then
            echo -e "${RED}Error: timed out waiting for another Ask Bridge installer to finish ($LOCK_DIR).${NC}" >&2
            exit 1
        fi
        sleep 0.2
    done
    printf '%s\n' "$$" > "$LOCK_DIR/pid"
}

MINIMUM_VERSION="${ASK_BRIDGE_MIN_VERSION:-}"
if [ -n "$MINIMUM_VERSION" ]; then
    if ! VERSION_COMPARISON="$(compare_release_versions "$VERSION" "$MINIMUM_VERSION")"; then
        echo -e "${RED}Error: target and minimum versions must use stable MAJOR.MINOR.PATCH format.${NC}" >&2
        exit 1
    fi
    if [ "$VERSION_COMPARISON" -lt 0 ]; then
        echo -e "${RED}Refusing to install ask-bridge $VERSION because the updater requires $MINIMUM_VERSION or newer.${NC}" >&2
        exit 1
    fi
fi

if [ "${ASK_BRIDGE_VERSION_CHECK_ONLY:-0}" = "1" ]; then
    exit 0
fi

echo -e "${CYAN}Starting Ask Bridge installation...${NC}"

# 1. Check Node.js and npx
if ! command -v node >/dev/null 2>&1; then
    echo -e "${RED}Error: Node.js is not installed.${NC}"
    echo -e "${YELLOW}Please install Node.js (https://nodejs.org/) and retry.${NC}"
    exit 1
fi

if ! command -v npx >/dev/null 2>&1; then
    echo -e "${RED}Error: npx is not installed.${NC}"
    echo -e "${YELLOW}Please make sure NPM/npx is available in your PATH.${NC}"
    exit 1
fi

# 2. Check Google Chrome
OS="$(uname -s)"
ARCH="$(uname -m)"

if [ "$OS" = "Darwin" ]; then
    if [ ! -x "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" ]; then
        echo -e "${YELLOW}Warning: Google Chrome was not found at /Applications/Google Chrome.app.${NC}"
        if command -v brew >/dev/null 2>&1; then
            echo -e "${CYAN}Installing Google Chrome via Homebrew...${NC}"
            brew install --cask google-chrome
        else
            echo -e "${YELLOW}Please install Google Chrome manually: https://www.google.com/chrome/${NC}"
        fi
    fi
elif [ "$OS" = "Linux" ]; then
    if ! command -v google-chrome >/dev/null 2>&1 && ! command -v google-chrome-stable >/dev/null 2>&1; then
        echo -e "${YELLOW}Warning: Google Chrome was not found in your PATH.${NC}"
        echo -e "${YELLOW}Please make sure Google Chrome is installed, as it is required by Chrome DevTools MCP.${NC}"
    fi
fi

# 3. Determine target architecture and file name
REPO_OWNER="EngelsChou"
REPO_NAME="ask-bridge"

if [ "$OS" = "Darwin" ]; then
    if [ "$ARCH" = "arm64" ]; then
        TARGET="aarch64-apple-darwin"
    else
        TARGET="x86_64-apple-darwin"
    fi
    EXT="tar.xz"
elif [ "$OS" = "Linux" ]; then
    if [ "$ARCH" = "x86_64" ]; then
        TARGET="x86_64-unknown-linux-gnu"
    else
        echo -e "${RED}Error: Unsupported Linux architecture: $ARCH. Only x86_64 is supported.${NC}"
        exit 1
    fi
    EXT="tar.xz"
else
    echo -e "${RED}Error: Unsupported operating system: $OS. For Windows, please run install.ps1.${NC}"
    exit 1
fi

ARTIFACT_NAME="ask-bridge-${TARGET}.${EXT}"
RELEASE_URL="https://github.com/${REPO_OWNER}/${REPO_NAME}/releases/download/v${VERSION}/${ARTIFACT_NAME}"
CHECKSUM_URL="${RELEASE_URL}.sha256"

# 4. Create installation directory
INSTALL_DIR="$HOME/.local/bin"
mkdir -p "$INSTALL_DIR"
VERSION_RECORD_PATH="$INSTALL_DIR/ask-bridge.version"
acquire_install_lock

if [ -f "$INSTALL_DIR/ask-bridge" ] && [ "${ASK_BRIDGE_ALLOW_DOWNGRADE:-0}" != "1" ]; then
    if [ ! -f "$VERSION_RECORD_PATH" ]; then
        echo -e "${RED}Error: the existing ask-bridge has no trusted version record. Set ASK_BRIDGE_ALLOW_DOWNGRADE=1 only if you explicitly want to replace it.${NC}" >&2
        exit 1
    fi
    if [ "$(awk 'END { print NR }' "$VERSION_RECORD_PATH")" -ne 1 ]; then
        echo -e "${RED}Error: the existing ask-bridge version record is invalid. Set ASK_BRIDGE_ALLOW_DOWNGRADE=1 only if you explicitly want to replace it.${NC}" >&2
        exit 1
    fi
    read -r INSTALLED_VERSION RECORDED_BINARY_SHA256 RECORD_EXTRA < "$VERSION_RECORD_PATH"
    if [[ ! "$INSTALLED_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || [[ ! "$RECORDED_BINARY_SHA256" =~ ^[[:xdigit:]]{64}$ ]] || [ -n "${RECORD_EXTRA:-}" ]; then
        echo -e "${RED}Error: the existing ask-bridge version record is invalid. Set ASK_BRIDGE_ALLOW_DOWNGRADE=1 only if you explicitly want to replace it.${NC}" >&2
        exit 1
    fi
    ACTUAL_INSTALLED_SHA256="$(sha256_file "$INSTALL_DIR/ask-bridge")"
    RECORDED_BINARY_SHA256="$(printf '%s' "$RECORDED_BINARY_SHA256" | tr '[:upper:]' '[:lower:]')"
    if [ "$ACTUAL_INSTALLED_SHA256" != "$RECORDED_BINARY_SHA256" ]; then
        echo -e "${RED}Error: the installed ask-bridge does not match its trusted version record. Refusing to replace it without ASK_BRIDGE_ALLOW_DOWNGRADE=1.${NC}" >&2
        exit 1
    fi
    if ! INSTALLED_COMPARISON="$(compare_release_versions "$VERSION" "$INSTALLED_VERSION")"; then
        echo -e "${RED}Error: installed and target versions must use stable MAJOR.MINOR.PATCH format.${NC}" >&2
        exit 1
    fi
    if [ "$INSTALLED_COMPARISON" -lt 0 ]; then
        echo -e "${RED}Refusing to downgrade ask-bridge from $INSTALLED_VERSION to $VERSION. Set ASK_BRIDGE_ALLOW_DOWNGRADE=1 only if you explicitly want this downgrade.${NC}" >&2
        exit 1
    fi
fi

TEMP_DIR=$(mktemp -d)

download_file() {
    local url="$1"
    local destination="$2"
    if command -v curl >/dev/null 2>&1; then
        curl -fL "$url" -o "$destination"
    elif command -v wget >/dev/null 2>&1; then
        wget "$url" -O "$destination"
    else
        echo -e "${RED}Error: Neither curl nor wget was found. Please install one of them to proceed.${NC}" >&2
        exit 1
    fi
}

ARCHIVE_PATH="$TEMP_DIR/$ARTIFACT_NAME"
CHECKSUM_PATH="$ARCHIVE_PATH.sha256"
echo -e "${CYAN}Downloading ${ARTIFACT_NAME} and SHA-256 checksum...${NC}"
download_file "$RELEASE_URL" "$ARCHIVE_PATH"
download_file "$CHECKSUM_URL" "$CHECKSUM_PATH"

if [ "$(awk 'END { print NR }' "$CHECKSUM_PATH")" -ne 1 ]; then
    echo -e "${RED}Error: Invalid multi-line checksum file downloaded from ${CHECKSUM_URL}.${NC}" >&2
    exit 1
fi
CHECKSUM_LINE="$(tr -d '\r' < "$CHECKSUM_PATH")"
read -r EXPECTED_SHA256 CHECKSUM_FILE_NAME CHECKSUM_EXTRA <<< "$CHECKSUM_LINE"
CHECKSUM_FILE_NAME="${CHECKSUM_FILE_NAME#\*}"
if [[ ! "$EXPECTED_SHA256" =~ ^[[:xdigit:]]{64}$ ]] || [ -z "$CHECKSUM_FILE_NAME" ] || [ -n "${CHECKSUM_EXTRA:-}" ]; then
    echo -e "${RED}Error: Invalid checksum file format downloaded from ${CHECKSUM_URL}.${NC}" >&2
    exit 1
fi
if [ "$CHECKSUM_FILE_NAME" != "$ARTIFACT_NAME" ]; then
    echo -e "${RED}Error: Checksum file names '$CHECKSUM_FILE_NAME', expected '$ARTIFACT_NAME'.${NC}" >&2
    exit 1
fi

ACTUAL_SHA256="$(sha256_file "$ARCHIVE_PATH")"
EXPECTED_SHA256="$(printf '%s' "$EXPECTED_SHA256" | tr '[:upper:]' '[:lower:]')"
if [ "$ACTUAL_SHA256" != "$EXPECTED_SHA256" ]; then
    echo -e "${RED}Error: SHA-256 checksum mismatch for $ARTIFACT_NAME. Expected $EXPECTED_SHA256, got $ACTUAL_SHA256.${NC}" >&2
    exit 1
fi
echo -e "${GREEN}Verified SHA-256 checksum for ${ARTIFACT_NAME}.${NC}"

echo -e "${CYAN}Extracting archive...${NC}"
tar -xJf "$ARCHIVE_PATH" -C "$TEMP_DIR"

# Find extracted binary (it could be in a subdirectory or direct)
BINARY_PATH=$(find "$TEMP_DIR" -type f -name "ask-bridge" | head -n 1)

if [ -z "$BINARY_PATH" ]; then
    echo -e "${RED}Error: Could not find ask-bridge binary in the downloaded archive.${NC}"
    exit 1
fi

# 5. Install binary and alias
echo -e "${CYAN}Installing ask-bridge to $INSTALL_DIR/ask-bridge...${NC}"
STAGED_BINARY="$INSTALL_DIR/.ask-bridge.new.$$"
STAGED_RECORD="$INSTALL_DIR/.ask-bridge.version.new.$$"
rm -f -- "$STAGED_BINARY" "$STAGED_RECORD"
cp "$BINARY_PATH" "$STAGED_BINARY"
chmod +x "$STAGED_BINARY"
INSTALLED_BINARY_SHA256="$(sha256_file "$STAGED_BINARY")"
printf '%s  %s\n' "$VERSION" "$INSTALLED_BINARY_SHA256" > "$STAGED_RECORD"

# Both files are staged on the destination filesystem.  Each rename is atomic;
# if the process stops between them, the hash-bound record fails closed on the
# next run instead of allowing an incorrect downgrade decision.
mv -f -- "$STAGED_BINARY" "$INSTALL_DIR/ask-bridge"
mv -f -- "$STAGED_RECORD" "$VERSION_RECORD_PATH"
ln -sf "$INSTALL_DIR/ask-bridge" "$INSTALL_DIR/ask"

# 6. Check PATH
case :$PATH: in
    *:$INSTALL_DIR:*) ;;
    *) 
        echo -e "${YELLOW}Warning: $INSTALL_DIR is not in your PATH.${NC}"
        echo -e "${YELLOW}To run 'ask-bridge' globally, add it to your shell configuration (e.g. ~/.bashrc, ~/.zshrc):${NC}"
        echo -e "${CYAN}  export PATH=\"\$PATH:$INSTALL_DIR\"${NC}"
        ;;
esac

echo -e "${GREEN}Successfully installed! You can now use the 'ask-bridge' command. The 'ask' alias is also available.${NC}"
