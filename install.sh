#!/bin/sh
# jdkm installer (macOS, zsh)
#
#   curl -fsSL https://raw.githubusercontent.com/yxx-z/jdkm/main/install.sh | sh
#
# Environment variables:
#   JDKM_HOME             where jdkm lives (default: ~/.jdkm); the binary goes to $JDKM_HOME/bin
#   JDKM_INSTALL_VERSION  release tag to install, e.g. v0.1.0 (default: latest)
#                         (not JDKM_VERSION: that one selects the JDK for `jdkm shell`)
#   ZDOTDIR               zsh config directory (default: ~), as zsh itself uses it
#
# Safe to run again: it upgrades the binary and leaves an existing
# ~/.zshrc block alone.

set -eu

REPO="yxx-z/jdkm"
DEFAULT_HOME="$HOME/.jdkm"
JDKM_HOME="${JDKM_HOME:-$DEFAULT_HOME}"
BIN_DIR="$JDKM_HOME/bin"
VERSION="${JDKM_INSTALL_VERSION:-latest}"
ZSHRC="${ZDOTDIR:-$HOME}/.zshrc"
BEGIN_MARK="# >>> jdkm >>>"
END_MARK="# <<< jdkm <<<"

say() { printf '%s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

# --- platform -------------------------------------------------------------
[ "$(uname -s)" = Darwin ] || die "jdkm currently supports macOS only"
case "$(uname -m)" in
  arm64 | aarch64) target=aarch64-apple-darwin ;;
  x86_64)
    target=x86_64-apple-darwin
    # An x86_64 shell under Rosetta on Apple Silicon: install the native build.
    if [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = 1 ]; then
      target=aarch64-apple-darwin
    fi
    ;;
  *) die "unsupported CPU architecture: $(uname -m)" ;;
esac
command -v curl >/dev/null 2>&1 || die "curl is required"
command -v shasum >/dev/null 2>&1 || die "shasum is required"

if [ -n "${JDKM_INSTALL_BASE_URL:-}" ]; then
  base="$JDKM_INSTALL_BASE_URL" # for testing, e.g. file:///path/to/dist
elif [ "$VERSION" = latest ]; then
  base="https://github.com/$REPO/releases/latest/download"
else
  base="https://github.com/$REPO/releases/download/$VERSION"
fi
asset="jdkm-$target.tar.gz"

tmp=$(mktemp -d "${TMPDIR:-/tmp}/jdkm-install.XXXXXX")
trap 'rm -rf "$tmp"' EXIT INT TERM

# --- download and verify --------------------------------------------------
say "Downloading $asset ($VERSION)..."
curl -fsSL --retry 3 -o "$tmp/$asset" "$base/$asset" ||
  die "download failed: $base/$asset"
curl -fsSL --retry 3 -o "$tmp/$asset.sha256" "$base/$asset.sha256" ||
  die "download failed: $base/$asset.sha256"

expected=$(awk '{print $1}' "$tmp/$asset.sha256")
actual=$(shasum -a 256 "$tmp/$asset" | awk '{print $1}')
if [ -z "$expected" ] || [ "$expected" != "$actual" ]; then
  die "SHA-256 mismatch for $asset (expected $expected, got $actual)"
fi
say "SHA-256 verified"

tar -xzf "$tmp/$asset" -C "$tmp"
[ -f "$tmp/jdkm" ] || die "archive does not contain jdkm"
chmod 755 "$tmp/jdkm"
"$tmp/jdkm" --version >/dev/null 2>&1 || die "the downloaded jdkm does not run on this machine"

# --- install the binary (atomic replace) ------------------------------------
mkdir -p "$BIN_DIR"
cp "$tmp/jdkm" "$BIN_DIR/.jdkm.new"
mv -f "$BIN_DIR/.jdkm.new" "$BIN_DIR/jdkm"
say "Installed $("$BIN_DIR/jdkm" --version) to $BIN_DIR/jdkm"

# --- zsh configuration ----------------------------------------------------
if [ -f "$ZSHRC" ] && grep -qF "$BEGIN_MARK" "$ZSHRC"; then
  say "$ZSHRC already contains the jdkm block; left unchanged"
else
  # Lines added by hand earlier (outside our block) would run before our PATH
  # entry exists. Point them out instead of editing them silently.
  manual=""
  if [ -f "$ZSHRC" ]; then
    manual=$(grep -n 'jdkm init' "$ZSHRC" | cut -d: -f1 | tr '\n' ' ' || true)
  fi

  # Use $HOME literally when possible, so the block stays readable.
  if [ "$JDKM_HOME" = "$DEFAULT_HOME" ]; then
    path_line='export PATH="$HOME/.jdkm/bin:$PATH"'
    home_line=""
  else
    path_line="export PATH=\"$BIN_DIR:\$PATH\""
    home_line="export JDKM_HOME=\"$JDKM_HOME\""
  fi
  {
    printf '\n%s\n' "$BEGIN_MARK"
    if [ -n "$home_line" ]; then printf '%s\n' "$home_line"; fi
    printf '%s\n' "$path_line"
    printf '%s\n' 'eval "$(jdkm init zsh)"'
    printf '%s\n' "$END_MARK"
  } >>"$ZSHRC" # append: keeps a symlinked .zshrc (dotfiles) intact
  say "Added the jdkm block to the end of $ZSHRC"

  if [ -n "$manual" ]; then
    warn "$ZSHRC already had \`jdkm init\` on line(s): ${manual% }"
    warn "remove those lines; the new block at the end of the file replaces them"
  fi
fi

# A different jdkm earlier in PATH (e.g. from `cargo install`) would be confusing.
other=$(command -v jdkm 2>/dev/null || true)
if [ -n "$other" ] && [ "$other" != "$BIN_DIR/jdkm" ]; then
  warn "another jdkm is installed at $other"
  case "$other" in
    */.cargo/bin/jdkm) warn "remove it with: cargo uninstall jdkm" ;;
    *) warn "consider removing it to avoid confusion" ;;
  esac
fi

say ""
say "Done. Open a new terminal (or run: exec zsh), then try:"
say "  jdkm list-remote"
say "  jdkm install 21"
