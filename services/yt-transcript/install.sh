#!/bin/sh
# Instala o serviço de transcrição do YouTube fora do repositório (o volume pode não estar montado no login)
# e grava o LaunchAgent com.anthive.novo-yt, que sobe com o Mac. Rodar de novo atualiza as bibliotecas.
set -e
cd "$(dirname "$0")"
DEST="$HOME/.local/share/novo-yt-transcript"
PORT="${NOVO_YT_PORT:-8790}"
LABEL=com.anthive.novo-yt
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
mkdir -p "$DEST" "$HOME/Library/Logs"
[ -x "$DEST/venv/bin/python" ] || python3 -m venv "$DEST/venv"
"$DEST/venv/bin/pip" install -q --upgrade -r requirements.txt
cp server.py "$DEST/server.py"
cat > "$PLIST" <<PL
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>$LABEL</string>
  <key>ProgramArguments</key><array>
    <string>$DEST/venv/bin/python</string><string>-I</string><string>$DEST/server.py</string><string>--port</string><string>$PORT</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardErrorPath</key><string>$HOME/Library/Logs/novo-yt-transcript.log</string>
  <key>StandardOutPath</key><string>$HOME/Library/Logs/novo-yt-transcript.log</string>
</dict></plist>
PL
launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null || true
launchctl bootstrap "gui/$(id -u)" "$PLIST"
echo "Serviço em http://127.0.0.1:$PORT (log em ~/Library/Logs/novo-yt-transcript.log)"
