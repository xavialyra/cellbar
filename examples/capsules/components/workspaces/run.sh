#!/bin/sh
set -eu

detect_compositor() {
    # 1. Prioritize current desktop session variable if set and verified
    case "${XDG_CURRENT_DESKTOP:-${XDG_SESSION_DESKTOP:-}}" in
        *niri*|*Niri*)
            if command -v niri >/dev/null 2>&1 && [ -n "${NIRI_SOCKET:-}" ] && [ -S "${NIRI_SOCKET:-}" ]; then
                echo "niri"; return 0
            fi
            ;;
        *hyprland*|*Hyprland*)
            if [ -z "${HYPRLAND_INSTANCE_SIGNATURE:-}" ]; then
                sig=$(ls -1t "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/hypr/" 2>/dev/null | head -n 1 || true)
                [ -n "$sig" ] && export HYPRLAND_INSTANCE_SIGNATURE="$sig"
            fi
            if command -v hyprctl >/dev/null 2>&1 && hyprctl activeworkspace >/dev/null 2>&1; then
                echo "hyprland"; return 0
            fi
            ;;
        *sway*|*Sway*)
            if command -v swaymsg >/dev/null 2>&1 && [ -n "${SWAYSOCK:-}" ] && [ -S "${SWAYSOCK:-}" ]; then
                echo "sway"; return 0
            fi
            ;;
        *mango*|*Mango*)
            if command -v mmsg >/dev/null 2>&1 && [ -n "${MANGO_INSTANCE_SIGNATURE:-}" ] && [ -S "${MANGO_INSTANCE_SIGNATURE:-}" ]; then
                echo "mango"; return 0
            fi
            ;;
    esac

    # 2. Fallback to active socket / live IPC probes
    if command -v niri >/dev/null 2>&1 && [ -n "${NIRI_SOCKET:-}" ] && [ -S "${NIRI_SOCKET:-}" ]; then
        echo "niri"
    elif command -v hyprctl >/dev/null 2>&1; then
        if [ -z "${HYPRLAND_INSTANCE_SIGNATURE:-}" ]; then
            sig=$(ls -1t "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/hypr/" 2>/dev/null | head -n 1 || true)
            [ -n "$sig" ] && export HYPRLAND_INSTANCE_SIGNATURE="$sig"
        fi
        if hyprctl activeworkspace >/dev/null 2>&1; then
            echo "hyprland"
        fi
    elif command -v swaymsg >/dev/null 2>&1 && [ -n "${SWAYSOCK:-}" ] && [ -S "${SWAYSOCK:-}" ]; then
        echo "sway"
    elif command -v mmsg >/dev/null 2>&1 && [ -n "${MANGO_INSTANCE_SIGNATURE:-}" ] && [ -S "${MANGO_INSTANCE_SIGNATURE:-}" ]; then
        echo "mango"
    fi
}

compositor=$(detect_compositor)

if [ "${1:-}" = "--action" ]; then
    arg="${2:-}"
    action="${arg%%,*}"
    target="${arg#*,}"
    [ "$target" = "$arg" ] && target="${3:-}"

    case "$compositor" in
        niri)
            case "$action" in
                view|toggle) niri msg action focus-workspace "$target" ;;
                next) niri msg action focus-workspace-down ;;
                prev) niri msg action focus-workspace-up ;;
            esac
            ;;
        hyprland)
            case "$action" in
                view|toggle) hyprctl dispatch workspace "$target" ;;
                next) hyprctl dispatch workspace e+1 ;;
                prev) hyprctl dispatch workspace e-1 ;;
            esac
            ;;
        sway)
            case "$action" in
                view) swaymsg workspace "$target" ;;
                next) swaymsg workspace next ;;
                prev) swaymsg workspace prev ;;
            esac
            ;;
        mango)
            case "$action" in
                view) mmsg dispatch view,"$target" ;;
                toggle) mmsg dispatch toggleview,"$target" ;;
                next) mmsg dispatch viewtoright ;;
                prev) mmsg dispatch viewtoleft ;;
            esac
            ;;
    esac
    exit 0
fi

hide_empty=false
output=""
scope_style=""
active_style="accent"
active_pad="true"
format_type="numbers"
active_icon="●"
inactive_icon="○"
skipped_icon="·"
skipped_style="ws_skipped"

while [ "$#" -gt 0 ]; do
    case "$1" in
        --hide-empty)
            [ "$#" -ge 2 ] || exit 2
            hide_empty=$2
            shift 2
            ;;
        --output)
            [ "$#" -ge 2 ] || exit 2
            output=$2
            shift 2
            ;;
        --scope-style)
            [ "$#" -ge 2 ] || exit 2
            scope_style=$2
            shift 2
            ;;
        --active-style)
            [ "$#" -ge 2 ] || exit 2
            active_style=$2
            shift 2
            ;;
        --active-pad)
            [ "$#" -ge 2 ] || exit 2
            active_pad=$2
            shift 2
            ;;
        --format)
            [ "$#" -ge 2 ] || exit 2
            format_type=$2
            shift 2
            ;;
        --active-icon)
            [ "$#" -ge 2 ] || exit 2
            active_icon=$2
            shift 2
            ;;
        --inactive-icon)
            [ "$#" -ge 2 ] || exit 2
            inactive_icon=$2
            shift 2
            ;;
        --skipped-icon)
            [ "$#" -ge 2 ] || exit 2
            skipped_icon=$2
            shift 2
            ;;
        --skipped-style)
            [ "$#" -ge 2 ] || exit 2
            skipped_style=$2
            shift 2
            ;;
        --event)
            # Retained for compatibility, direct IPC query takes precedence
            [ "$#" -ge 2 ] || exit 2
            shift 2
            ;;
        *)
            shift
            ;;
    esac
done

# Query compositor IPC for workspace / tag state
data=""

case "$compositor" in
    niri)
        niri_ws=$(niri msg --json workspaces 2>/dev/null || true)
        if [ -n "$niri_ws" ] && command -v jq >/dev/null 2>&1; then
            data=$(jq -n \
                --argjson ws "$niri_ws" \
                --arg output "$output" '
                ($ws | map(select($output == "" or .output == $output or .output == null))) as $matched |
                ([$matched[]?.idx] + [1] | max) as $max_ws |
                [range(1; ($max_ws + 1))] |
                map(
                    . as $i |
                    ([$matched[]? | select(.idx == $i)][0]) as $w |
                    {
                        index: $i,
                        is_active: ($w.is_active // $w.is_focused // false),
                        client_count: (if $w.active_window_id != null or ($w.is_active // false) then 1 else 0 end),
                        is_urgent: ($w.is_urgent // false)
                    }
                ) | { tags: . }
            ' 2>/dev/null || true)
        fi
        ;;
    hyprland)
        hypr_ws=$(hyprctl workspaces -j 2>/dev/null || true)
        hypr_monitors=$(hyprctl monitors -j 2>/dev/null || true)
        hypr_active=$(hyprctl activeworkspace -j 2>/dev/null || true)
        if command -v jq >/dev/null 2>&1; then
            data=$(jq -n \
                --argjson ws "${hypr_ws:-[]}" \
                --argjson monitors "${hypr_monitors:-[]}" \
                --argjson active "${hypr_active:-null}" \
                --arg output "$output" '
                (
                    if $output != "" then
                        ($monitors[]? | select(.name == $output) | .activeWorkspace.id)
                    else
                        ($monitors[]? | select(.focused) | .activeWorkspace.id)
                    end // ($active?.id) // ($monitors[0]?.activeWorkspace.id) // 1
                ) as $act_id |
                (($ws // []) | map(select((.id > 0) and ($output == "" or .monitor == $output or .monitor == null)))) as $matched |
                ([$matched[]?.id] + [$act_id, 1] | max) as $max_ws |
                [range(1; ($max_ws + 1))] |
                map(
                    . as $i |
                    ([$matched[]? | select(.id == $i)][0]) as $w |
                    {
                        index: $i,
                        is_active: ($i == $act_id),
                        client_count: ($w.windows // (if $i == $act_id then 1 else 0 end)),
                        is_urgent: false
                    }
                ) | { tags: . }
            ' 2>/dev/null || true)
        fi
        ;;
    sway)
        sway_ws=$(swaymsg -t get_workspaces 2>/dev/null || true)
        if [ -n "$sway_ws" ] && command -v jq >/dev/null 2>&1; then
            data=$(jq -n \
                --argjson ws "$sway_ws" \
                --arg output "$output" '
                ($ws | map(select($output == "" or .output == $output))) as $matched |
                ([$matched[]?.num] + [1] | max) as $max_ws |
                [range(1; ($max_ws + 1))] |
                map(
                    . as $i |
                    ([$matched[]? | select(.num == $i)][0]) as $w |
                    {
                        index: $i,
                        is_active: ($w.focused // false),
                        client_count: (if $w != null then 1 else 0 end),
                        is_urgent: ($w.urgent // false)
                    }
                ) | { tags: . }
            ' 2>/dev/null || true)
        fi
        ;;
    mango)
        cmd="mmsg get all-tags"
        if [ -n "$output" ]; then
            cmd="mmsg get tags $output"
        fi
        data=$($cmd 2>/dev/null || true)
        ;;
esac

if [ -n "$data" ] && command -v jq >/dev/null 2>&1; then
    formatted=$(printf '%s' "$data" | jq -r \
        --argjson hide "$hide_empty" \
        --arg act "$active_style" \
        --arg pad "$active_pad" \
        --arg fmt "$format_type" \
        --arg act_icon "$active_icon" \
        --arg inact_icon "$inactive_icon" \
        --arg skip_icon "$skipped_icon" \
        --arg skip_style "$skipped_style" '
        (if .all_tags then .all_tags[0].tags else .tags end) // [] |
        . as $tags |
        ([$tags[] | select(.is_active or .client_count > 0) | .index] | max // 1) as $max_idx |
        $tags |
        map(
            select(($hide | not) or .index <= $max_idx) |
            (
                if $fmt == "dots" then
                    { act: $act_icon, inact: $inact_icon }
                else
                    { act: (.index | tostring), inact: (.index | tostring) }
                end
            ) as $chars |
            "#ws:" + (.index | tostring) + "{" + (
                if .is_active then
                    if $pad == "true" then
                        "[ " + $chars.act + " ](@" + $act + ")"
                    else
                        "[" + $chars.act + "](@" + $act + ")"
                    end
                elif .is_urgent then
                    "[!" + $chars.inact + "!](@warning)"
                elif .client_count > 0 then
                    "[" + $chars.inact + "](@muted)"
                else
                    if $pad == "true" then
                        "[ " + $skip_icon + " ](@" + $skip_style + ")"
                    else
                        "[" + $skip_icon + "](@" + $skip_style + ")"
                    end
                end
            ) + "}"
        ) | join(if $pad == "true" then "[ ](@muted)" else "[  ](@muted)" end)
    ' 2>/dev/null || true)

    if [ -n "$formatted" ]; then
        if [ -n "$scope_style" ]; then
            printf '#workspaces(@%s){ [  ]%s[  ] }\n' "$scope_style" "$formatted"
        else
            printf '%s\n' "$formatted"
        fi
        exit 0
    fi
fi

# Exit without output if state cannot be obtained; Cellbar preserves the previous frame
exit 0
