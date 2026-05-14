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
else
  ARTIFACT="${BIN}-${OS}-${ARCH}"
  BIN_FILE="${BIN}"
fi

# ── fetch latest release tag ─────────────────────────────────────────────────

need curl

LATEST_URL="https://api.github.com/repos/${REPO}/releases/latest"
TAG="$(curl -sSfL "$LATEST_URL" | grep '"tag_name"' | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')"
[ -n "$TAG" ] || die "could not determine latest release tag"

DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${TAG}/${ARTIFACT}"

# ── download ─────────────────────────────────────────────────────────────────

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "Downloading olive ${TAG} for ${OS}/${ARCH}..."
curl -sSfL "$DOWNLOAD_URL" -o "${TMP}/${BIN_FILE}" || die "download failed: $DOWNLOAD_URL"
chmod +x "${TMP}/${BIN_FILE}"

# ── install ──────────────────────────────────────────────────────────────────

mkdir -p "$INSTALL_DIR"
mv "${TMP}/${BIN_FILE}" "${INSTALL_DIR}/${BIN_FILE}"

echo "Installed: ${INSTALL_DIR}/${BIN_FILE}"

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
