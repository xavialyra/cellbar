#!/bin/sh
set -eu

device='@DEFAULT_AUDIO_SINK@'
output='percent'
show_mute=false
icon=''
icons='󰕿,󰖀,󰕾'
mute_icon='󰝟'
scope_style=''
icon_style='peach'

while [ "$#" -gt 0 ]; do
    case "$1" in
        --device)
            [ "$#" -ge 2 ] || exit 2
            device=$2
            shift 2
            ;;
        --output)
            [ "$#" -ge 2 ] || exit 2
            output=$2
            shift 2
            ;;
        --show-mute)
            [ "$#" -ge 2 ] || exit 2
            show_mute=$2
            shift 2
            ;;
        --icon)
            [ "$#" -ge 2 ] || exit 2
            icon=$2
            shift 2
            ;;
        --icons)
            [ "$#" -ge 2 ] || exit 2
            icons=$2
            shift 2
            ;;
        --mute-icon)
            [ "$#" -ge 2 ] || exit 2
            mute_icon=$2
            shift 2
            ;;
        --scope-style)
            [ "$#" -ge 2 ] || exit 2
            scope_style=$2
            shift 2
            ;;
        --icon-style)
            [ "$#" -ge 2 ] || exit 2
            icon_style=$2
            shift 2
            ;;
        *)
            printf 'unknown argument: %s\n' "$1" >&2
            exit 2
            ;;
    esac
done

emit_volume() {
    fallback_icon="$icon"
    [ -z "$fallback_icon" ] && fallback_icon='󰕾'
    if ! value=$(wpctl get-volume "$device" 2>/dev/null); then
        printf '[%s ?](@volume)\n' "$fallback_icon"
        return
    fi

    percent=$(printf '%s\n' "$value" | awk '{printf "%.0f%%", $2 * 100}')
    vol_num=$(printf '%s\n' "$value" | awk '{printf "%.0f", $2 * 100}')

    is_muted=false
    case "$value" in
        *MUTED*) is_muted=true ;;
    esac

    current_icon=""
    if [ "$is_muted" = true ] && [ -n "$mute_icon" ]; then
        current_icon="$mute_icon"
    elif [ -n "$icons" ]; then
        current_icon=$(printf '%s' "$icons" | awk -F',' -v v="$vol_num" '{
            if (NF == 0) exit
            idx = int(v * NF / 100) + 1
            if (idx > NF) idx = NF
            if (idx < 1) idx = 1
            printf "%s", $idx
        }')
    fi
    [ -z "$current_icon" ] && current_icon="$fallback_icon"

    case "$output" in
        percent) text=$percent ;;
        label) text="Volume $percent" ;;
        *) printf 'unknown output: %s\n' "$output" >&2; return 2 ;;
    esac

    case "$value:$show_mute" in
        *MUTED*:true)
            if [ -n "$scope_style" ]; then
                printf '#muted(@%s){ [  ][%s](@muted) [ muted][  ] }\n' "$scope_style" "$current_icon"
            else
                printf '#muted{ [%s %s muted](@muted) }\n' "$current_icon" "$text"
            fi
            ;;
        *)
            if [ -n "$scope_style" ]; then
                printf '#volume(@%s){ [  ][%s ](@%s)[%s][  ] }\n' "$scope_style" "$current_icon" "$icon_style" "$text"
            else
                printf '[%s %s](@volume)\n' "$current_icon" "$text"
            fi
            ;;
    esac
}

emit_volume
