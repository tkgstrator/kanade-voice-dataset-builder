#!/bin/zsh
set -e

sudo chown -R $(whoami):$(whoami) target 2>/dev/null || true
sudo chown -R $(whoami):$(whoami) ~/.cargo 2>/dev/null || true
sudo chown -R $(whoami):$(whoami) /usr/local/cargo 2>/dev/null || true

mkdir -p ~/.config/direnv
cat > ~/.config/direnv/direnv.toml <<'EOF'
[global]
log_format = ""
hide_env_diff = true
EOF

if [ -f Cargo.toml ] && { [ -d src ] || grep -q '^\[workspace\]' Cargo.toml; }; then
  cargo fetch --locked
fi
