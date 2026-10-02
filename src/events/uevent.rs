use std::{io, os::fd::OwnedFd};

use rustix::net::{
    AddressFamily, SocketFlags, SocketType, bind,
    netlink::{self, SocketAddrNetlink},
    socket_with,
};

pub const UEVENT_GROUP: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Uevent {
    pub action: String,
    pub subsystem: Option<String>,
    pub devpath: String,
    pub devname: Option<String>,
    pub sequence: Option<u64>,
}

pub fn open_socket() -> io::Result<OwnedFd> {
    let fd = socket_with(
        AddressFamily::NETLINK,
        SocketType::RAW,
        SocketFlags::NONBLOCK | SocketFlags::CLOEXEC,
        Some(netlink::KOBJECT_UEVENT),
    )?;
    bind(&fd, &SocketAddrNetlink::new(0, UEVENT_GROUP))?;
    Ok(fd)
}

pub fn parse_message(bytes: &[u8]) -> Option<Uevent> {
    let mut fields = bytes.split(|byte| *byte == 0);
    let header = std::str::from_utf8(fields.next()?).ok()?;
    let (header_action, header_path) = header.split_once('@')?;

    let mut action = None;
    let mut subsystem = None;
    let mut devpath = None;
    let mut devname = None;
    let mut sequence = None;

    for field in fields {
        let Ok(field) = std::str::from_utf8(field) else {
            continue;
        };
        let Some((key, value)) = field.split_once('=') else {
            continue;
        };
        match key {
            "ACTION" => action = Some(value.to_owned()),
            "SUBSYSTEM" => subsystem = Some(value.to_owned()),
            "DEVPATH" => devpath = Some(value.to_owned()),
            "DEVNAME" => devname = Some(value.to_owned()),
            "SEQNUM" => sequence = value.parse().ok(),
            _ => {}
        }
    }

    let action = action.unwrap_or_else(|| header_action.to_owned());
    let devpath = devpath.unwrap_or_else(|| header_path.to_owned());
    if action.is_empty() || devpath.is_empty() {
        return None;
    }

    Some(Uevent {
        action,
        subsystem,
        devpath,
        devname,
        sequence,
    })
}

pub fn format_event_context(subscription: &str, event: &Uevent) -> String {
    let mut message = serde_json::Map::new();
    message.insert(
        "action".to_owned(),
        serde_json::Value::String(event.action.clone()),
    );
    message.insert(
        "devpath".to_owned(),
        serde_json::Value::String(event.devpath.clone()),
    );
    if let Some(subsystem) = &event.subsystem {
        message.insert(
            "subsystem".to_owned(),
            serde_json::Value::String(subsystem.clone()),
        );
    }
    if let Some(devname) = &event.devname {
        message.insert(
            "devname".to_owned(),
            serde_json::Value::String(devname.clone()),
        );
    }
    if let Some(sequence) = event.sequence {
        message.insert(
            "sequence".to_owned(),
            serde_json::Value::Number(sequence.into()),
        );
    }

    serde_json::json!({
        "version": 1,
        "source": "uevent",
        "subscription": subscription,
        "message": message,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_power_supply_change() {
        let event = parse_message(
            b"change@/devices/LNXSYSTM:00/PNP0C0A:00/power_supply/BAT0\0ACTION=change\0DEVPATH=/devices/LNXSYSTM:00/PNP0C0A:00/power_supply/BAT0\0SUBSYSTEM=power_supply\0POWER_SUPPLY_STATUS=Charging\0SEQNUM=1234\0",
        )
        .expect("valid uevent");

        assert_eq!(
            event,
            Uevent {
                action: "change".into(),
                subsystem: Some("power_supply".into()),
                devpath: "/devices/LNXSYSTM:00/PNP0C0A:00/power_supply/BAT0".into(),
                devname: None,
                sequence: Some(1234),
            }
        );
    }

    #[test]
    fn ignores_invalid_message_and_unknown_fields() {
        assert!(parse_message(b"invalid\0ACTION=change\0").is_none());
        let event = parse_message(b"add@/devices/test\0SUBSYSTEM=backlight\0UNKNOWN=value\0")
            .expect("valid uevent");
        assert_eq!(event.action, "add");
        assert_eq!(event.subsystem.as_deref(), Some("backlight"));
    }

    #[test]
    fn formats_normalized_context() {
        let event = Uevent {
            action: "change".into(),
            subsystem: Some("power_supply".into()),
            devpath: "/devices/test".into(),
            devname: Some("BAT0".into()),
            sequence: Some(42),
        };
        let value: serde_json::Value =
            serde_json::from_str(&format_event_context("battery", &event)).expect("JSON");

        assert_eq!(value["source"], "uevent");
        assert_eq!(value["subscription"], "battery");
        assert_eq!(value["message"]["subsystem"], "power_supply");
        assert_eq!(value["message"]["sequence"], 42);
    }
}
