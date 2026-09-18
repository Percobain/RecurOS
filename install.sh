#!/bin/sh
# Install the `ctx` binary (ContextOS) on macOS or Linux.
#
#   curl -fsSL https://raw.githubusercontent.com/Percobain/ContextOS/main/install.sh | sh
#
# Installs to ~/.local/bin (override with CTX_INSTALL_DIR). No root, no
# runtime dependencies: ctx is a single static-ish binary with SQLite built in.
set -eu

REPO="${CTX_REPO:-Percobain/ContextOS}"
DIR="${CTX_INSTALL_DIR:-$HOME/.local/bin}"

os=$(uname -s)
arch=$(uname -m)
case "$os-$arch" in
  Linux-x86_64)  target=x86_64-unknown-linux-gnu ;;
  Linux-aarch64 | Linux-arm64) target=aarch64-unknown-linux-gnu ;;
  Darwin-x86_64) target=x86_64-apple-darwin ;;
  Darwin-arm64)  target=aarch64-apple-darwin ;;
  *) echo "ctx: no prebuilt binary for $os-$arch; build from source with:" >&2
     echo "  cargo install --git https://github.com/$REPO ctx-cli" >&2
     exit 1 ;;
esac

url="https://github.com/$REPO/releases/latest/download/ctx-$target.tar.gz"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

echo "downloading $url"
curl -fsSL "$url" -o "$tmp/ctx.tar.gz"
tar xzf "$tmp/ctx.tar.gz" -C "$tmp"
mkdir -p "$DIR"
mv "$tmp/ctx" "$DIR/ctx"
chmod +x "$DIR/ctx"

echo "installed $("$DIR/ctx" --version) to $DIR/ctx"
case ":$PATH:" in
  *":$DIR:"*) ;;
  *) echo
     echo "add it to your PATH (then open a new terminal):"
     echo "  echo 'export PATH=\"$DIR:\$PATH\"' >> ~/.profile" ;;
esac
echo
echo "next: cd into a project and run  ctx init"
