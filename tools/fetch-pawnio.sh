#!/usr/bin/env bash
# Downloads the official PawnIO driver installer that the app installer bundles.
set -euo pipefail
cd "$(dirname "$0")/../src-tauri/resources/pawnio"
URL="https://github.com/namazso/PawnIO.Setup/releases/download/2.2.0/PawnIO_setup.exe"
SHA="1f519a22e47187f70a1379a48ca604981c4fcf694f4e65b734aaa74a9fba3032"
if [ -f PawnIO_setup.exe ] && echo "$SHA  PawnIO_setup.exe" | sha256sum -c --status; then
  echo "PawnIO_setup.exe already present"
  exit 0
fi
curl -fsSL -o PawnIO_setup.exe "$URL"
echo "$SHA  PawnIO_setup.exe" | sha256sum -c -
