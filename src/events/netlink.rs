use std::{io, os::fd::OwnedFd};

use rustix::net::{
    AddressFamily, SocketFlags, SocketType, bind, netlink::SocketAddrNetlink, socket_with,
};

pub const RTMGRP_LINK: u32 = 1;
pub const RTMGRP_IPV4_IFADDR: u32 = 0x10;
pub const RTMGRP_IPV4_ROUTE: u32 = 0x40;
pub const RTMGRP_IPV6_IFADDR: u32 = 0x100;
pub const RTMGRP_IPV6_ROUTE: u32 = 0x400;

pub const DEFAULT_ROUTE_GROUPS: u32 =
    RTMGRP_LINK | RTMGRP_IPV4_IFADDR | RTMGRP_IPV4_ROUTE | RTMGRP_IPV6_IFADDR | RTMGRP_IPV6_ROUTE;

pub const RTM_NEWLINK: u16 = 16;
pub const RTM_DELLINK: u16 = 17;
pub const RTM_NEWADDR: u16 = 20;
pub const RTM_DELADDR: u16 = 21;
pub const RTM_NEWROUTE: u16 = 24;
pub const RTM_DELROUTE: u16 = 25;

pub const NLMSG_DONE: u16 = 3;
pub const NLMSG_ERROR: u16 = 2;

pub const IFLA_IFNAME: u16 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetlinkEvent {
    pub kind: &'static str,
    pub action: &'static str,
    pub interface_index: Option<u32>,
    pub interface_name: Option<String>,
}

pub fn open_route_socket() -> io::Result<OwnedFd> {
    let fd = socket_with(
        AddressFamily::NETLINK,
        SocketType::RAW,
        SocketFlags::NONBLOCK | SocketFlags::CLOEXEC,
        None,
    )?;
    let addr = SocketAddrNetlink::new(0, DEFAULT_ROUTE_GROUPS);
    bind(&fd, &addr)?;
    Ok(fd)
}

pub fn parse_netlink_messages(buffer: &[u8]) -> Vec<NetlinkEvent> {
    let mut events = Vec::new();
    let mut offset = 0;

    while offset + 16 <= buffer.len() {
        let msg_len =
            u32::from_ne_bytes(buffer[offset..offset + 4].try_into().unwrap_or_default()) as usize;
        if msg_len < 16 || offset + msg_len > buffer.len() {
            break;
        }

        let msg_type = u16::from_ne_bytes(
            buffer[offset + 4..offset + 6]
                .try_into()
                .unwrap_or_default(),
        );
        if msg_type == NLMSG_DONE || msg_type == NLMSG_ERROR {
            break;
        }

        let payload = &buffer[offset + 16..offset + msg_len];
        if let Some(event) = parse_message(msg_type, payload) {
            events.push(event);
        }

        let aligned_len = (msg_len + 3) & !3;
        if aligned_len == 0 {
            break;
        }
        offset += aligned_len;
    }

    events
}

fn parse_message(msg_type: u16, payload: &[u8]) -> Option<NetlinkEvent> {
    match msg_type {
        RTM_NEWLINK | RTM_DELLINK => {
            let action = if msg_type == RTM_NEWLINK {
                "new"
            } else {
                "del"
            };
            let mut if_index = None;
            let mut if_name = None;

            if payload.len() >= 16 {
                let raw_index = i32::from_ne_bytes(payload[4..8].try_into().unwrap_or_default());
                if raw_index > 0 {
                    if_index = Some(raw_index as u32);
                }

                if payload.len() > 16 {
                    if_name = parse_rtattr_ifname(&payload[16..]);
                }
            }

            Some(NetlinkEvent {
                kind: "link",
                action,
                interface_index: if_index,
                interface_name: if_name,
            })
        }
        RTM_NEWADDR | RTM_DELADDR => {
            let action = if msg_type == RTM_NEWADDR {
                "new"
            } else {
                "del"
            };
            let mut if_index = None;
            if payload.len() >= 8 {
                let raw_index = u32::from_ne_bytes(payload[4..8].try_into().unwrap_or_default());
                if raw_index > 0 {
                    if_index = Some(raw_index);
                }
            }
            Some(NetlinkEvent {
                kind: "addr",
                action,
                interface_index: if_index,
                interface_name: None,
            })
        }
        RTM_NEWROUTE | RTM_DELROUTE => {
            let action = if msg_type == RTM_NEWROUTE {
                "new"
            } else {
                "del"
            };
            Some(NetlinkEvent {
                kind: "route",
                action,
                interface_index: None,
                interface_name: None,
            })
        }
        _ => None,
    }
}

fn parse_rtattr_ifname(mut attributes: &[u8]) -> Option<String> {
    while attributes.len() >= 4 {
        let rta_len = u16::from_ne_bytes(attributes[0..2].try_into().unwrap_or_default()) as usize;
        let rta_type = u16::from_ne_bytes(attributes[2..4].try_into().unwrap_or_default());

        if rta_len < 4 || rta_len > attributes.len() {
            break;
        }

        if rta_type == IFLA_IFNAME {
            let value_bytes = &attributes[4..rta_len];
            let trimmed = value_bytes.split(|&b| b == 0).next().unwrap_or(value_bytes);
            if let Ok(name) = std::str::from_utf8(trimmed)
                && !name.is_empty()
            {
                return Some(name.to_string());
            }
        }

        let aligned_len = (rta_len + 3) & !3;
        if aligned_len == 0 || aligned_len > attributes.len() {
            break;
        }
        attributes = &attributes[aligned_len..];
    }
    None
}

pub fn format_netlink_context(subscription_id: &str, event: &NetlinkEvent) -> String {
    let mut message = serde_json::Map::new();
    message.insert(
        "type".to_string(),
        serde_json::Value::String(event.kind.to_string()),
    );
    message.insert(
        "action".to_string(),
        serde_json::Value::String(event.action.to_string()),
    );
    if let Some(index) = event.interface_index {
        message.insert(
            "interface_index".to_string(),
            serde_json::Value::Number(index.into()),
        );
    }
    if let Some(name) = &event.interface_name {
        message.insert(
            "interface_name".to_string(),
            serde_json::Value::String(name.clone()),
        );
    }

    serde_json::json!({
        "version": 1,
        "subscription": subscription_id,
        "source": "netlink",
        "family": "route",
        "message": message,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_newlink_with_interface_name() {
        let mut msg = Vec::new();
        let ifname = b"wlan0\0";
        let rta_len = (4 + ifname.len()) as u16;
        let rta_aligned = (rta_len as usize + 3) & !3;
        let total_len = (16 + 16 + rta_aligned) as u32;

        msg.extend_from_slice(&total_len.to_ne_bytes());
        msg.extend_from_slice(&RTM_NEWLINK.to_ne_bytes());
        msg.extend_from_slice(&0_u16.to_ne_bytes());
        msg.extend_from_slice(&1_u32.to_ne_bytes());
        msg.extend_from_slice(&0_u32.to_ne_bytes());

        msg.push(0);
        msg.push(0);
        msg.extend_from_slice(&1_u16.to_ne_bytes());
        msg.extend_from_slice(&3_i32.to_ne_bytes());
        msg.extend_from_slice(&0_u32.to_ne_bytes());
        msg.extend_from_slice(&0_u32.to_ne_bytes());

        msg.extend_from_slice(&rta_len.to_ne_bytes());
        msg.extend_from_slice(&IFLA_IFNAME.to_ne_bytes());
        msg.extend_from_slice(ifname);
        while msg.len() < total_len as usize {
            msg.push(0);
        }

        let events = parse_netlink_messages(&msg);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            NetlinkEvent {
                kind: "link",
                action: "new",
                interface_index: Some(3),
                interface_name: Some("wlan0".to_string()),
            }
        );
    }

    #[test]
    fn parses_newaddr_message() {
        let mut msg = Vec::new();
        let total_len = (16 + 8) as u32;

        msg.extend_from_slice(&total_len.to_ne_bytes());
        msg.extend_from_slice(&RTM_NEWADDR.to_ne_bytes());
        msg.extend_from_slice(&0_u16.to_ne_bytes());
        msg.extend_from_slice(&1_u32.to_ne_bytes());
        msg.extend_from_slice(&0_u32.to_ne_bytes());

        msg.push(2);
        msg.push(24);
        msg.push(0);
        msg.push(0);
        msg.extend_from_slice(&5_u32.to_ne_bytes());

        let events = parse_netlink_messages(&msg);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            NetlinkEvent {
                kind: "addr",
                action: "new",
                interface_index: Some(5),
                interface_name: None,
            }
        );
    }

    #[test]
    fn ignores_truncated_and_done_messages() {
        assert!(parse_netlink_messages(&[]).is_empty());
        assert!(parse_netlink_messages(&[0; 12]).is_empty());

        let mut done_msg = Vec::new();
        let len = 16_u32;
        done_msg.extend_from_slice(&len.to_ne_bytes());
        done_msg.extend_from_slice(&NLMSG_DONE.to_ne_bytes());
        done_msg.extend_from_slice(&[0; 8]);
        assert!(parse_netlink_messages(&done_msg).is_empty());
    }

    #[test]
    fn formats_netlink_context_json() {
        let event = NetlinkEvent {
            kind: "link",
            action: "new",
            interface_index: Some(2),
            interface_name: Some("eth0".to_string()),
        };
        let context = format_netlink_context("route-change", &event);
        let parsed: serde_json::Value = serde_json::from_str(&context).expect("valid json");
        assert_eq!(parsed["version"], 1);
        assert_eq!(parsed["subscription"], "route-change");
        assert_eq!(parsed["source"], "netlink");
        assert_eq!(parsed["family"], "route");
        assert_eq!(parsed["message"]["type"], "link");
        assert_eq!(parsed["message"]["action"], "new");
        assert_eq!(parsed["message"]["interface_name"], "eth0");
        assert_eq!(parsed["message"]["interface_index"], 2);
    }

    #[test]
    fn opens_valid_route_socket() {
        let socket = open_route_socket();
        assert!(
            socket.is_ok(),
            "cannot open netlink route socket: {socket:?}"
        );
    }
}
