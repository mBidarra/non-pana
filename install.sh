#!/bin/sh
# Compila o Anthive Novo em modo release e instala `novo` e `novod` em ~/.local/bin.
set -e
cd "$(dirname "$0")"
cargo build --release -p novo -p novod
mkdir -p "$HOME/.local/bin"
# sobrescrever no lugar invalida a assinatura em cache e o macOS mata o binário (Killed: 9): remove antes
for b in novo novod; do rm -f "$HOME/.local/bin/$b"; cp "target/release/$b" "$HOME/.local/bin/$b"; done
echo "Instalado em $HOME/.local/bin (garanta que ele está no PATH)."
"$HOME/.local/bin/novo" doctor || true
