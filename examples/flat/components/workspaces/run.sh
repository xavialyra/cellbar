#!/bin/sh
set -eu

if [ "${1:-}" = "--action" ]; then
    arg="${2:-}"
    action="${arg%%,*}"
    target="${arg#*,}"
    [ "$target" = "$arg" ] && target="${3:-}"
    if [ -n "${MANGO_INSTANCE_SIGNATURE:-}" ] && command -v mmsg >/dev/null 2>&1; then
        case "$action" in
            view) mmsg dispatch view,"$target" ;;
            toggle) mmsg dispatch toggleview,"$target" ;;
            next) mmsg dispatch viewtoright ;;
            prev) mmsg dispatch viewtoleft ;;
        esac
    elif [ -n "${HYPRLAND_INSTANCE_SIGNATURE:-}" ] && command -v hyprctl >/dev/null 2>&1; then
        case "$action" in
            view) hyprctl dispatch workspace "$target" ;;
            toggle) hyprctl dispatch workspace "$target" ;;
            next) hyprctl dispatch workspace e+1 ;;
            prev) hyprctl dispatch workspace e-1 ;;
        esac
    elif [ -n "${SWAYSOCK:-}" ] && command -v swaymsg >/dev/null 2>&1; then
        case "$action" in
            view) swaymsg workspace "$target" ;;
            next) swaymsg workspace next ;;
            prev) swaymsg workspace prev ;;
        esac
    fi
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

# Query mangowc IPC directly for the latest state
if command -v mmsg >/dev/null 2>&1 && [ -n "${MANGO_INSTANCE_SIGNATURE:-}" ]; then
    cmd="mmsg get all-tags"
    if [ -n "$output" ]; then
        cmd="mmsg get tags $output"
    fi
    data=$($cmd 2>/dev/null || true)
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
            $tags |
            map(
                select(($hide | not) or .is_active or .client_count > 0 or .is_urgent) |
                (
                    if $fmt == "dots" then
                        { act: $act_icon, inact: $inact_icon }
                    elif $fmt == "circles" or $fmt == "circle_numbers" then
                        ["①","②","③","④","⑤","⑥","⑦","⑧","⑨","⑩"] as $inacts |
                        ["❶","❷","❸","❹","❺","❻","❼","❽","❾","❿"] as $acts |
                        {
                            act: ($acts[.index - 1] // (.index | tostring)),
                            inact: ($inacts[.index - 1] // (.index | tostring))
                        }
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
                printf '#workspaces(@%s){ %s }\n' "$scope_style" "$formatted"
            else
                printf '%s\n' "$formatted"
            fi
            exit 0
        fi
    fi
fi

# Exit without output if state cannot be obtained; Cellbar preserves the previous frame
exit 0
