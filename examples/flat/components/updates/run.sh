#!/bin/sh
set -eu

icon='󰏖'
while [ "$#" -gt 0 ]; do
    case "$1" in
        --icon)
            [ "$#" -ge 2 ] || exit 2
            icon=$2
            shift 2
            ;;
        *)
            printf 'unknown argument: %s\n' "$1" >&2
            exit 2
            ;;
    esac
done

count='?'
if command -v checkupdates >/dev/null 2>&1; then
    count=$(checkupdates 2>/dev/null | wc -l | awk '{print $1}')
elif command -v apt >/dev/null 2>&1; then
    count=$(apt list --upgradable 2>/dev/null | tail -n +2 | wc -l | awk '{print $1}')
fi

printf '[%s %s](@muted)\n' "$icon" "$count"
