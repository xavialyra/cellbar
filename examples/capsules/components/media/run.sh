#!/bin/sh
set -eu

icon='󰝚'
cover=''
event=null
scope_style=''
cover_width='2'
cover_align='left'
text_max_width='40'
play_icon=''
pause_icon='󰏤'
playing_style='peach'
paused_style='muted'
while [ "$#" -gt 0 ]; do
    case "$1" in
        --icon)
            [ "$#" -ge 2 ] || exit 2
            icon=$2
            shift 2
            ;;
        --scope-style)
            [ "$#" -ge 2 ] || exit 2
            scope_style=$2
            shift 2
            ;;
        --cover-width)
            [ "$#" -ge 2 ] || exit 2
            cover_width=$2
            shift 2
            ;;
        --cover-align)
            [ "$#" -ge 2 ] || exit 2
            cover_align=$2
            shift 2
            ;;
        --text-max-width)
            [ "$#" -ge 2 ] || exit 2
            text_max_width=$2
            shift 2
            ;;
        --play-icon)
            [ "$#" -ge 2 ] || exit 2
            play_icon=$2
            shift 2
            ;;
        --pause-icon)
            [ "$#" -ge 2 ] || exit 2
            pause_icon=$2
            shift 2
            ;;
        --playing-style)
            [ "$#" -ge 2 ] || exit 2
            playing_style=$2
            shift 2
            ;;
        --paused-style)
            [ "$#" -ge 2 ] || exit 2
            paused_style=$2
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

case "$text_max_width" in
    ''|*[!0-9]*|0) printf 'text-max-width must be a positive integer\n' >&2; exit 2 ;;
esac

escape_markup() {
    printf '%s' "$1" | awk '{
        if (NR > 1) printf " "
        for (i = 1; i <= length($0); i++) {
            char = substr($0, i, 1)
            if (char == "\\" || char == "[" || char == "]") printf "%s%s", "\\", char
            else printf "%s", char
        }
    }'
}

status=''
artist=''
title=''
art_url=''
metadata=''

# PropertiesChanged usually carries only changed fields. Keep partial metadata
# updates usable; periodic refreshes and owner changes use playerctl as fallback.
if [ "$event" != 'null' ] && command -v jq >/dev/null 2>&1; then
    status=$(printf '%s' "$event" | jq -r '.message.body[1].PlaybackStatus // empty' 2>/dev/null || true)
    artist=$(printf '%s' "$event" | jq -r '
        .message.body[1].Metadata["xesam:artist"] // empty
        | if type == "array" then .[0] // empty else . end
    ' 2>/dev/null || true)
    title=$(printf '%s' "$event" | jq -r '
        .message.body[1].Metadata["xesam:title"] // empty
        | if type == "string" then . else empty end
    ' 2>/dev/null || true)
    art_url=$(printf '%s' "$event" | jq -r '.message.body[1].Metadata["mpris:artUrl"] // empty' 2>/dev/null || true)
fi

if [ -z "$status" ] || [ -z "$title" ]; then
    if command -v playerctl >/dev/null 2>&1; then
        # Query status and both metadata fields in one playerctl process.
        value=$(playerctl metadata --format '{{status}}|{{artist}}|{{title}}' 2>/dev/null || true)
        IFS='|' read -r player_status player_artist player_title <<EOF
$value
EOF
        [ -z "$status" ] && [ -n "$player_status" ] && status=$player_status
        [ -n "$player_artist" ] && artist=$player_artist
        [ -n "$player_title" ] && title=$player_title
    fi
fi

if [ -z "$status" ] && command -v playerctl >/dev/null 2>&1; then
    status=$(playerctl status 2>/dev/null || true)
fi

is_playing=false
case "$(printf '%s' "$status" | tr '[:upper:]' '[:lower:]')" in
    playing)
        is_playing=true
        ;;
    *)
        is_playing=false
        ;;
esac

if [ "$is_playing" = true ]; then
    status_icon="$play_icon"
    status_style="$playing_style"
    text_style="subtext"
else
    status_icon="$pause_icon"
    status_style="$paused_style"
    text_style="$paused_style"
fi

if [ -n "$artist" ] && [ -n "$title" ]; then
    metadata="$artist - $title"
elif [ -n "$title" ]; then
    metadata="$title"
elif [ -n "$artist" ]; then
    metadata="$artist"
fi

# When no media is present, output empty line so the widget stays hidden.
if [ -z "$metadata" ]; then
    printf '\n'
    exit 0
fi

if [ -z "$art_url" ] && command -v playerctl >/dev/null 2>&1; then
    art_url=$(playerctl metadata --format '{{mpris:artUrl}}' 2>/dev/null || true)
fi

# Resolve local file:// cover art when available.
case "$art_url" in
    file://localhost/*)
        candidate=/${art_url#file://localhost/}
        [ -f "$candidate" ] && cover=$candidate
        ;;
    file:///*)
        candidate=/${art_url#file:///}
        [ -f "$candidate" ] && cover=$candidate
        ;;
    /*)
        [ -f "$art_url" ] && cover=$art_url
        ;;
esac

# Some players expose album art as https:// URLs. Download it in the provider
# process so the renderer still receives a local file path.
case "$art_url" in
    http://*|https://*)
        cache_root=${XDG_CACHE_HOME:-${HOME:-/tmp}/.cache}
        cache_dir=$cache_root/cellbar
        mkdir -p "$cache_dir" 2>/dev/null || true
        temporary="$cache_dir/media-cover.$$"
        if { command -v curl >/dev/null 2>&1 \
            && curl -LfsS --max-time 1.5 "$art_url" -o "$temporary" 2>/dev/null \
            && [ -s "$temporary" ]; } \
            || { command -v wget >/dev/null 2>&1 \
            && wget -q -T 2 -O "$temporary" "$art_url" \
            && [ -s "$temporary" ]; }; then
            if command -v cksum >/dev/null 2>&1; then
                set -- $(cksum < "$temporary")
                cache_file=$cache_dir/media-cover-$1-$2
                if [ -f "$cache_file" ]; then
                    rm -f "$temporary"
                    touch "$cache_file"
                else
                    mv "$temporary" "$cache_file"
                fi
                cover=$cache_file
                find "$cache_dir" -maxdepth 1 -name 'media-cover-*' -type f -mmin +1440 -delete 2>/dev/null || true
            else
                rm -f "$temporary"
            fi
        else
            rm -f "$temporary"
        fi
        ;;
esac

metadata=$(escape_markup "$metadata")
# Limit the combined artist/title without truncating adjacent padding or art.
text_style="$text_style max=$text_max_width left"

if [ -n "$cover" ] && [ -f "$cover" ]; then
    cover=$(escape_markup "$cover")
    img="![${cover}](${cover_width} cover circle ${cover_align})"
    if [ -n "$status_icon" ]; then
        if [ -n "$scope_style" ]; then
            printf '#player(@%s){ [  ]%s [ %s ](@%s)[%s](@%s max_width=25)[  ] }\n' \
                "$scope_style" "$img" "$status_icon" "$status_style" "$metadata" "$text_style"
        else
            printf '#player{ %s [ %s ](@%s)[%s](@%s max_width=25) }\n' \
                "$img" "$status_icon" "$status_style" "$metadata" "$text_style"
        fi
    else
        if [ -n "$scope_style" ]; then
            printf '#player(@%s){ [  ]%s [%s](@%s max_width=25)[  ] }\n' \
                "$scope_style" "$img" "$metadata" "$text_style"
        else
            printf '#player{ %s [ %s ](@%s max_width=25) }\n' \
                "$img" "$metadata" "$text_style"
        fi
    fi
else
    display_icon="$icon"
    [ -n "$status_icon" ] && display_icon="$status_icon"
    if [ -n "$scope_style" ]; then
        printf '#player(@%s){ [  ][%s ](@%s)[%s](@%s max_width=25)[  ] }\n' \
            "$scope_style" "$display_icon" "$status_style" "$metadata" "$text_style"
    else
        printf '#player{ [%s ](@%s)[%s](@%s max_width=25) }\n' \
            "$display_icon" "$status_style" "$metadata" "$text_style"
    fi
fi
