#!/bin/sh
# Compila o Anthive Novo em modo release e instala `novo` e `novod` em ~/.local/bin.
set -e
cd "$(dirname "$0")"
cargo build --release -p novo -p novod
mkdir -p "$HOME/.local/bin"
cp target/release/novo target/release/novod "$HOME/.local/bin/"
echo "Instalado em $HOME/.local/bin (garanta que ele está no PATH)."
"$HOME/.local/bin/novo" doctor || true
