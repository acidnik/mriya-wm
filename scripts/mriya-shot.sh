#!/usr/bin/env bash
# Screenshot only the Mriya WM window, without disturbing the user's desktop.
#
# `niri msg action screenshot-window` captures the focused window to the
# configured screenshot-path. This script focuses the Mriya window for a moment,
# captures it, restores the previously focused window and prints the new file.
#
# Usage: scripts/mriya-shot.sh [output-file]
set -euo pipefail

output=${1:-}

# Identifier of the window that currently has focus, to restore it afterwards.
previous=$(niri msg focused-window 2>/dev/null | awk '/Window ID:/ {print $3}' | tr -d ':')
mriya=$(niri msg windows | awk '/^Window ID/ {id=$3} /Title: "Mriya WM"/ {gsub(":", "", id); print id; exit}')

if [ -z "${mriya:-}" ]; then
    echo "mriya-shot: no Mriya WM window found in niri" >&2
    exit 1
fi

before=$(ls -t "$HOME/Pictures/screenshots" 2>/dev/null | head -1 || true)

niri msg action focus-window --id "$mriya" >/dev/null
sleep 0.3
niri msg action screenshot-window >/dev/null

# Wait for niri to write the file (it is asynchronous).
newest=""
for _ in $(seq 1 50); do
    newest=$(ls -t "$HOME/Pictures/screenshots" 2>/dev/null | head -1 || true)
    if [ -n "$newest" ] && [ "$newest" != "$before" ]; then
        break
    fi
    sleep 0.1
done

if [ -n "${previous:-}" ] && [ "$previous" != "$mriya" ]; then
    niri msg action focus-window --id "$previous" >/dev/null || true
fi

if [ -z "$newest" ] || [ "$newest" = "$before" ]; then
    echo "mriya-shot: niri did not write a screenshot" >&2
    exit 1
fi

path="$HOME/Pictures/screenshots/$newest"
if [ -n "$output" ]; then
    mv "$path" "$output"
    path="$output"
fi
echo "$path"
