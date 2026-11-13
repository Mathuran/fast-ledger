#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# Ensure non-interactive shells sourcing ~/.bashrc via BASH_ENV receive PATH exports.
touch "$HOME/.bashrc"
sed -i '/case \$- in/,/esac/d' "$HOME/.bashrc" 2>/dev/null || true

ensure_path() {
  local dir="$1"
  local bashrc_line="$2"
  if [ -d "$dir" ]; then
    grep -qxF "$bashrc_line" "$HOME/.bashrc" || echo "$bashrc_line" >> "$HOME/.bashrc"
    case ":$PATH:" in
      *":$dir:"*) ;;
      *) export PATH="$dir:$PATH" ;;
    esac
  fi
}

# Install Rust via rustup if cargo is not already available.
if ! command -v cargo >/dev/null 2>&1 && [ ! -x "$HOME/.cargo/bin/cargo" ]; then
  curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs \
    | sh -s -- -y --no-modify-path --default-toolchain stable
fi
ensure_path "$HOME/.cargo/bin" 'export PATH="$HOME/.cargo/bin:$PATH"'

# Install elan and the Lean 4 toolchain pinned in spec/lean-toolchain.
if ! command -v elan >/dev/null 2>&1 && [ ! -x "$HOME/.elan/bin/elan" ]; then
  lean_toolchain="$(tr -d '[:space:]' < spec/lean-toolchain)"
  curl -fsSL https://elan.lean-lang.org/elan-init.sh \
    | sh -s -- -y --no-modify-path --default-toolchain "$lean_toolchain"
fi
ensure_path "$HOME/.elan/bin" 'export PATH="$HOME/.elan/bin:$PATH"'

# Build the Lean 4 specification and verify all Rust workspace targets compile.
(cd spec && lake build)
cargo check --workspace --all-targets
