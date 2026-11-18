#!/bin/sh
# jdkm uninstaller (macOS, zsh)
#
#   curl -fsSL https://raw.githubusercontent.com/yxx-z/jdkm/main/uninstall.sh | sh
#       removes the jdkm binary and the ~/.zshrc block, KEEPS installed JDKs
#
#   curl -fsSL https://raw.githubusercontent.com/yxx-z/jdkm/main/uninstall.sh | sh -s -- --all
#       also deletes $JDKM_HOME (default ~/.jdkm), including every installed JDK
#
# Honours JDKM_HOME and ZDOTDIR like install.sh.

set -eu

DEFAULT_HOME="$HOME/.jdkm"
JDKM_HOME="${JDKM_HOME:-$DEFAULT_HOME}"
BIN_DIR="$JDKM_HOME/bin"
ZSHRC="${ZDOTDIR:-$HOME}/.zshrc"
BEGIN_MARK="# >>> jdkm >>>"
END_MARK="# <<< jdkm <<<"

say() { printf '%s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

purge=no
for arg in "$@"; do
  case "$arg" in
    --all) purge=yes ;;
    -h | --help)
      sed -n '2,11p' "$0" 2>/dev/null || true
      exit 0
      ;;
    *) die "unknown option: $arg (use --all to also delete installed JDKs)" ;;
  esac
done

# --- zsh configuration ----------------------------------------------------
if [ -f "$ZSHRC" ] && grep -qF "$BEGIN_MARK" "$ZSHRC"; then
  backup="$ZSHRC.jdkm-backup"
  cp "$ZSHRC" "$backup"
  # Drop the block and the blank line(s) right before it (install.sh adds one).
  # Blank lines are held back and only printed once a non-blank line follows.
  awk -v b="$BEGIN_MARK" -v e="$END_MARK" '
    $0 == b { skip = 1; blank = 0; next }
    $0 == e { skip = 0; next }
    skip { next }
    $0 == "" { blank++; next }
    { while (blank > 0) { print ""; blank-- } print }
    END { while (blank > 0) { print ""; blank-- } }
  ' "$backup" >"$ZSHRC.jdkm-tmp"
  # Write through the existing file so a symlinked .zshrc stays a symlink.
  cat "$ZSHRC.jdkm-tmp" >"$ZSHRC"
  rm -f "$ZSHRC.jdkm-tmp"
  say "Removed the jdkm block from $ZSHRC (backup: $backup)"
else
  say "No jdkm block found in $ZSHRC"
fi
if [ -f "$ZSHRC" ] && grep -q 'jdkm init' "$ZSHRC"; then
  lines=$(grep -n 'jdkm init' "$ZSHRC" | cut -d: -f1 | tr '\n' ' ')
  warn "$ZSHRC still mentions \`jdkm init\` on line(s): ${lines% }; remove them by hand"
fi

# --- binary ---------------------------------------------------------------
if [ -e "$BIN_DIR/jdkm" ]; then
  rm -f "$BIN_DIR/jdkm"
  rmdir "$BIN_DIR" 2>/dev/null || true
  say "Removed $BIN_DIR/jdkm"
else
  say "No jdkm binary at $BIN_DIR/jdkm"
fi

# --- data -----------------------------------------------------------------
if [ "$purge" = yes ]; then
  # Refuse anything that does not look like a jdkm directory.
  case "$JDKM_HOME" in
    "" | / | "$HOME" | "$HOME/") die "refusing to delete JDKM_HOME=$JDKM_HOME" ;;
  esac
  if [ -d "$JDKM_HOME" ]; then
    if [ ! -d "$JDKM_HOME/versions" ] && [ ! -e "$JDKM_HOME/default" ] && [ ! -d "$JDKM_HOME/cache" ]; then
      die "$JDKM_HOME does not look like a jdkm directory; not deleting it"
    fi
    rm -rf "$JDKM_HOME"
    say "Deleted $JDKM_HOME (all installed JDKs)"
  fi
elif [ -d "$JDKM_HOME/versions" ]; then
  count=$(find "$JDKM_HOME/versions" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')
  say "Kept $count installed JDK(s) in $JDKM_HOME."
  say "To delete them too: rm -rf $JDKM_HOME   (or re-run with: sh -s -- --all)"
fi

say ""
say "jdkm is uninstalled. Open a new terminal to finish."
