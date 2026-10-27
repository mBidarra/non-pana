#!/bin/sh
# Empacota o Non Panã.app com o novod e o novo dentro (sidecars do Tauri).
set -e
cd "$(dirname "$0")/.."
TRIPLE=$(rustc -vV | sed -n 's/^host: //p')
cargo build --release -p novo -p novod
mkdir -p crates/novo-app/binaries
cp target/release/novod "crates/novo-app/binaries/novod-$TRIPLE"
cp target/release/novo "crates/novo-app/binaries/novo-$TRIPLE"
cd crates/novo-app
CONF='{"bundle":{"externalBin":["binaries/novod","binaries/novo"]}}'
if command -v cargo-tauri >/dev/null; then
  cargo tauri build --bundles app --config "$CONF"
else
  npx --yes @tauri-apps/cli@2 build --bundles app --config "$CONF"
fi
# o Tauri só assina os binários (linker-signed); sem assinar o pacote, `codesign -v` falha e o Gatekeeper reclama
cd ../..
codesign --force --deep -s - "target/release/bundle/macos/Non Panã.app"
echo "App em target/release/bundle/macos/ (na raiz do workspace). Para instalar:"
echo "  ditto \"target/release/bundle/macos/Non Panã.app\" \"/Applications/Non Panã.app\""
