#!/bin/bash
# Suramak dev environment bootstrap for the Linux build/VPS box.
set -e
export DEBIAN_FRONTEND=noninteractive

echo "=== [1/4] swap (4G extra, helps Rust linking on a 3.6G box) ==="
if ! swapon --show | grep -q '/swap2.img'; then
  sudo fallocate -l 4G /swap2.img
  sudo chmod 600 /swap2.img
  sudo mkswap /swap2.img
  sudo swapon /swap2.img
  grep -q '/swap2.img' /etc/fstab || echo '/swap2.img none swap sw,nofail 0 0' | sudo tee -a /etc/fstab >/dev/null
fi
swapon --show

echo "=== [2/4] apt build deps ==="
sudo apt-get update -qq
sudo apt-get install -y -qq clang cmake ninja-build pkg-config libssl-dev \
  libasound2-dev libpulse-dev libdbus-1-dev libfontconfig1-dev \
  libxkbcommon-dev libxkbcommon-x11-dev libx11-dev libxcb1-dev libxrandr-dev \
  libxi-dev libxcursor-dev libgl1-mesa-dev libegl1-mesa-dev libwayland-dev mesa-utils

echo "=== [3/4] rustup + stable toolchain ==="
if ! command -v cargo >/dev/null 2>&1 && [ ! -x "$HOME/.cargo/bin/cargo" ]; then
  curl -fsSL https://sh.rustup.rs -o /tmp/rustup-init.sh
  sh /tmp/rustup-init.sh -y --default-toolchain stable --profile minimal --component clippy --component rustfmt
fi
. "$HOME/.cargo/env"
rustc --version
cargo --version

echo "=== [4/4] macOS cross-check target ==="
rustup target add aarch64-apple-darwin

echo "=== ALL-DONE ==="
