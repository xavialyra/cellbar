#!/bin/sh
set -eu

active_icon='󰾫'
idle_icon='󰾪'
event=null
while [ "$#" -gt 0 ]; do
    case "$1" in
        --active-icon|--idle-icon)
            [ "$#" -ge 2 ] || exit 2
            if [ "$1" = "--active-icon" ]; then active_icon=$2; else idle_icon=$2; fi
            shift 2
            ;;
        --event)
            [ "$#" -ge 2 ] || exit 2
            event=$2
            shift 2
            ;;
        *)
            printf 'unknown argument: %s\n' "$1" >&2
            exit 2
            ;;
    esac
done

session=${XDG_SESSION_ID:-}
if [ "$event" != 'null' ] && [ -n "$session" ] && command -v jq >/dev/null 2>&1; then
    path=$(printf '%s' "$event" | jq -r '.message.path // empty' 2>/dev/null || true)
    idle=$(printf '%s' "$event" | jq -r '.message.body[1].IdleHint // empty' 2>/dev/null || true)
    expected_path="/org/freedesktop/login1/session/$session"
    if [ "$path" = "$expected_path" ]; then
        case "$idle" in
            true) printf '[%s](@muted)\n' "$idle_icon"; exit 0 ;;
            false) printf '[%s]\n' "$active_icon"; exit 0 ;;
        esac
    fi
fi

if [ -z "$session" ] || ! command -v loginctl >/dev/null 2>&1; then
    printf '[%s]\n' "$active_icon"
    exit 0
fi

idle=$(loginctl show-session "$session" -p IdleHint --value 2>/dev/null || printf 'unknown')
case "$idle" in
    yes) printf '[%s](@muted)\n' "$idle_icon" ;;
    no) printf '[%s]\n' "$active_icon" ;;
    *) printf '[%s]\n' "$active_icon" ;;
esac
