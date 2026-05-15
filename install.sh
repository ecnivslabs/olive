#!/usr/bin/env sh
set -e

REPO="olive-language/olive"
BIN="pit"
INSTALL_DIR="${OLIVE_INSTALL_DIR:-$HOME/.local/bin}"

# ── helpers ──────────────────────────────────────────────────────────────────

die() { echo "error: $1" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "required tool '$1' not found"; }

# ── detect OS ────────────────────────────────────────────────────────────────

OS="$(uname -s)"
case "$OS" in
  Linux)   OS=linux ;;
  Darwin)  OS=macos ;;
  MINGW*|MSYS*|CYGWIN*) OS=windows ;;
  *) die "unsupported OS: $OS" ;;
esac

# ── detect arch ──────────────────────────────────────────────────────────────

ARCH="$(uname -m)"
case "$ARCH" in
  x86_64|amd64)   ARCH=x86_64 ;;
  aarch64|arm64)  ARCH=aarch64 ;;
  *) die "unsupported architecture: $ARCH" ;;
esac

# ── resolve artifact name ────────────────────────────────────────────────────

if [ "$OS" = "windows" ]; then
  ARTIFACT="${BIN}-${OS}-${ARCH}.exe"
  BIN_FILE="${BIN}.exe"
  LIB_ARTIFACT="libolive_std-${OS}-${ARCH}.dll"
  LIB_FILE="libolive_std.dll"
elif [ "$OS" = "macos" ]; then
  ARTIFACT="${BIN}-${OS}-${ARCH}"
  BIN_FILE="${BIN}"
  LIB_ARTIFACT="libolive_std-${OS}-${ARCH}.dylib"
  LIB_FILE="libolive_std.dylib"
else
  ARTIFACT="${BIN}-${OS}-${ARCH}"
  BIN_FILE="${BIN}"
  LIB_ARTIFACT="libolive_std-${OS}-${ARCH}.so"
  LIB_FILE="libolive_std.so"
fi

# ── fetch latest release tag ─────────────────────────────────────────────────

need curl

LATEST_URL="https://api.github.com/repos/${REPO}/releases/latest"
TAG="$(curl -sSfL "$LATEST_URL" | grep '"tag_name"' | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')"
[ -n "$TAG" ] || die "could not determine latest release tag"

DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${TAG}/${ARTIFACT}"
LIB_DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${TAG}/${LIB_ARTIFACT}"

# ── download ─────────────────────────────────────────────────────────────────

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "Downloading olive ${TAG} for ${OS}/${ARCH}..."
curl -sSfL "$DOWNLOAD_URL" -o "${TMP}/${BIN_FILE}" || die "download failed: $DOWNLOAD_URL"
chmod +x "${TMP}/${BIN_FILE}"

echo "Downloading stdlib..."
curl -sSfL "$LIB_DOWNLOAD_URL" -o "${TMP}/${LIB_FILE}" || echo "  Warning: could not download stdlib artifact. You may need to build it from source."

echo "Downloading stdlib source..."
SOURCE_URL="https://github.com/${REPO}/archive/refs/tags/${TAG}.tar.gz"
curl -sSfL "$SOURCE_URL" -o "${TMP}/source.tar.gz" || echo "  Warning: could not download stdlib source."

# ── install ──────────────────────────────────────────────────────────────────

mkdir -p "$INSTALL_DIR"
mv "${TMP}/${BIN_FILE}" "${INSTALL_DIR}/${BIN_FILE}"

if [ -f "${TMP}/${LIB_FILE}" ]; then
  LIB_INSTALL_DIR="$(dirname "$INSTALL_DIR")/lib"
  mkdir -p "$LIB_INSTALL_DIR"
  mv "${TMP}/${LIB_FILE}" "${LIB_INSTALL_DIR}/${LIB_FILE}"
fi

if [ -f "${TMP}/source.tar.gz" ]; then
  STDLIB_SRC_DIR="$(dirname "$INSTALL_DIR")/lib/olive"
  mkdir -p "$STDLIB_SRC_DIR"
  mkdir -p "${TMP}/extracted"
  tar -xzf "${TMP}/source.tar.gz" -C "${TMP}/extracted" --strip-components=1
  cp -r "${TMP}/extracted/lib/"* "$STDLIB_SRC_DIR/"
fi

echo "Installed: ${INSTALL_DIR}/${BIN_FILE}"
[ -f "$(dirname "$INSTALL_DIR")/lib/${LIB_FILE}" ] && echo "Installed: $(dirname "$INSTALL_DIR")/lib/${LIB_FILE}"
[ -d "$(dirname "$INSTALL_DIR")/lib/olive" ] && echo "Installed stdlib source to: $(dirname "$INSTALL_DIR")/lib/olive"

# ── PATH hint ────────────────────────────────────────────────────────────────

case ":${PATH}:" in
  *":${INSTALL_DIR}:"*) ;;
  *)
    echo ""
    echo "  Add to PATH:"
    echo "    export PATH=\"\$HOME/.local/bin:\$PATH\""
    echo ""
    echo "  Then run: pit --version"
    ;;
esac
