#!/bin/sh
exec python3 -I "$(dirname "$0")/fake_claude.py" "$@"
