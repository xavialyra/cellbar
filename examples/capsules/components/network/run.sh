#!/bin/sh
set -eu

online_icon='󰤨'
offline_icon='󰤭'
ethernet_icon='󰈀'
wifi_icons='󰤯,󰤟,󰤢,󰤥,󰤨'
event=null
scope_style=''
icon_style='blue'
show_speed='false'

while [ "$#" -gt 0 ]; do
    case "$1" in
        --online-icon)
            [ "$#" -ge 2 ] || exit 2
            online_icon=$2
            shift 2
            ;;
        --offline-icon)
            [ "$#" -ge 2 ] || exit 2
            offline_icon=$2
            shift 2
            ;;
        --ethernet-icon)
            [ "$#" -ge 2 ] || exit 2
            ethernet_icon=$2
            shift 2
            ;;
        --wifi-icons)
            [ "$#" -ge 2 ] || exit 2
            wifi_icons=$2
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
        --show-speed)
            [ "$#" -ge 2 ] || exit 2
            show_speed=$2
            shift 2
            ;;
        --event)
            [ "$#" -ge 2 ] || exit 2
            event=$2
            shift 2
            ;;
        *)
            shift
            ;;
    esac
done

calc_speed() {
    dev="$1"
    [ -n "$dev" ] || return 0
    state_file="/tmp/cellbar-net-speed-${dev}.state"
    now=$(date +%s 2>/dev/null || true)
    [ -n "$now" ] || return 0
    
    bytes=$(awk -v target="$dev:" '$1 == target { print $2, $10; exit }' /proc/net/dev 2>/dev/null || true)
    if [ -z "$bytes" ]; then
        printf '  0K/s'
        return 0
    fi
    
    cur_rx=$(printf '%s' "$bytes" | awk '{print $1}')
    cur_tx=$(printf '%s' "$bytes" | awk '{print $2}')
    
    speed_text="  0K/s"
    if [ -f "$state_file" ]; then
        last_info=$(cat "$state_file" 2>/dev/null || true)
        last_time=$(printf '%s' "$last_info" | awk '{print $1}')
        last_rx=$(printf '%s' "$last_info" | awk '{print $2}')
        
        if [ -n "$last_time" ] && [ -n "$last_rx" ] && [ "$last_time" -gt 0 ] 2>/dev/null; then
            speed_text=$(awk -v now="$now" -v lt="$last_time" -v cr="$cur_rx" -v lr="$last_rx" 'BEGIN {
                dt = now - lt
                if (dt <= 0) dt = 1
                rx_rate = (cr >= lr) ? (cr - lr) / dt : 0
                if (rx_rate <= 0) {
                    printf "  0K/s"
                } else if (rx_rate < 102400) {
                    printf "%3dK/s", int(rx_rate / 1024)
                } else if (rx_rate < 1048576) {
                    printf "%3dK/s", int(rx_rate / 1024)
                } else if (rx_rate < 10485760) {
                    printf "%.1fM/s", rx_rate / 1048576
                } else {
                    printf "%3dM/s", int(rx_rate / 1048576)
                }
            }' 2>/dev/null || printf '  0K/s')
        fi
    fi
    
    printf '%s %s %s\n' "$now" "$cur_rx" "$cur_tx" > "$state_file" 2>/dev/null || true
    printf '%s' "${speed_text:-  0K/s}"
}

emit_icon() {
    icon_val="$1"
    speed_val=""
    if [ "$show_speed" = "true" ] && [ -n "${target_iface:-}" ]; then
        speed_val=$(calc_speed "$target_iface")
    elif [ "$show_speed" = "true" ] && [ -z "${target_iface:-}" ]; then
        speed_val="  --  "
    fi

    if [ -n "$speed_val" ]; then
        if [ -n "$scope_style" ]; then
            printf '#network(@%s){ [  ][%s ](@%s)[%s](@subtext)[  ] }\n' "$scope_style" "$icon_val" "$icon_style" "$speed_val"
        else
            printf '[%s ](@network)[%s](@subtext)\n' "$icon_val" "$speed_val"
        fi
    else
        if [ -n "$scope_style" ]; then
            printf '#network(@%s){ [  ][%s](@%s)[  ] }\n' "$scope_style" "$icon_val" "$icon_style"
        else
            printf '[%s](@network)\n' "$icon_val"
        fi
    fi
}

if ! command -v ip >/dev/null 2>&1; then
    emit_icon "$offline_icon"
    exit 0
fi

is_interface_online() {
    dev="$1"
    [ -n "$dev" ] || return 1
    [ -d "/sys/class/net/$dev" ] || return 1

    state=$(cat "/sys/class/net/$dev/operstate" 2>/dev/null || printf 'unknown')
    if [ "$state" = 'up' ]; then
        return 0
    fi

    carrier=$(cat "/sys/class/net/$dev/carrier" 2>/dev/null || printf '0')
    if [ "$carrier" = '1' ]; then
        return 0
    fi

    if [ "$state" = 'unknown' ]; then
        flags=$(cat "/sys/class/net/$dev/flags" 2>/dev/null || printf '0x0')
        if [ $((flags & 1)) -ne 0 ]; then
            return 0
        fi
    fi

    return 1
}

is_wireless() {
    dev="$1"
    [ -n "$dev" ] || return 1
    [ -d "/sys/class/net/$dev/wireless" ] || [ -d "/sys/class/net/$dev/phy80211" ]
}

get_wifi_percent() {
    dev="$1"
    if [ -r /proc/net/wireless ]; then
        info=$(awk -v target="$dev" 'NR > 2 && $1 ~ /:/ {
            d = $1
            sub(":", "", d)
            if (d == target) {
                sub("\\.", "", $3)
                sub("\\.", "", $4)
                print $3, $4
                exit
            }
        }' /proc/net/wireless)
        if [ -n "$info" ]; then
            quality=$(printf '%s' "$info" | awk '{print $1}')
            level=$(printf '%s' "$info" | awk '{print $2}')
            if [ -n "$quality" ] && [ "$quality" -gt 0 ] 2>/dev/null; then
                if [ "$quality" -le 70 ]; then
                    pct=$((quality * 100 / 70))
                else
                    pct=$quality
                fi
                if [ "$pct" -gt 100 ]; then
                    pct=100
                fi
                printf '%s\n' "$pct"
                return
            elif [ -n "$level" ] && [ "$level" -lt 0 ] 2>/dev/null; then
                pct=$(((level + 100) * 2))
                if [ "$pct" -lt 0 ]; then
                    pct=0
                fi
                if [ "$pct" -gt 100 ]; then
                    pct=100
                fi
                printf '%s\n' "$pct"
                return
            fi
        fi
    fi

    if command -v iw >/dev/null 2>&1; then
        dbm=$(iw dev "$dev" link 2>/dev/null | awk '/signal:/ {print $2; exit}')
        if [ -n "$dbm" ]; then
            pct=$(((dbm + 100) * 2))
            if [ "$pct" -lt 0 ]; then
                pct=0
            fi
            if [ "$pct" -gt 100 ]; then
                pct=100
            fi
            printf '%s\n' "$pct"
            return
        fi
    fi

    printf '100\n'
}

render_wifi_icon() {
    pct="$1"
    old_ifs="$IFS"
    IFS=','
    # shellcheck disable=SC2086
    set -- $wifi_icons
    IFS="$old_ifs"
    count=$#

    if [ "$count" -eq 0 ]; then
        emit_icon "$online_icon"
        return
    fi

    if [ "$pct" -ge 100 ]; then
        idx=$count
    else
        idx=$((pct * count / 100 + 1))
    fi
    if [ "$idx" -gt "$count" ]; then
        idx=$count
    fi

    eval "selected=\${$idx}"
    emit_icon "${selected:-$online_icon}"
}

default_iface=$(ip -o route show default 2>/dev/null |
    awk '{for (i = 1; i <= NF; i++) if ($i == "dev") { print $(i + 1); exit }}')

target_iface=""
if [ -n "$default_iface" ] && is_interface_online "$default_iface"; then
    target_iface="$default_iface"
fi

if [ -z "$target_iface" ]; then
    for dev in $(ip -o link show up 2>/dev/null | awk -F': ' '{print $2}' | cut -d'@' -f1); do
        [ "$dev" = "lo" ] && continue
        if is_interface_online "$dev"; then
            target_iface="$dev"
            break
        fi
    done
fi

if [ -z "$target_iface" ]; then
    emit_icon "$offline_icon"
    exit 0
fi

if is_wireless "$target_iface"; then
    pct=$(get_wifi_percent "$target_iface")
    render_wifi_icon "$pct"
    exit 0
fi

if [ -n "$default_iface" ] && [ "$default_iface" != "$target_iface" ] && is_wireless "$default_iface" && is_interface_online "$default_iface"; then
    pct=$(get_wifi_percent "$default_iface")
    render_wifi_icon "$pct"
    exit 0
fi

emit_icon "$ethernet_icon"
