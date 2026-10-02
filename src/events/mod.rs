pub(crate) mod dbus;
pub(crate) mod netlink;
pub(crate) mod pipewire;
pub(crate) mod system;
pub(crate) mod uevent;
pub(crate) mod wayland;

use calloop::channel::{self, Channel, Sender};

#[derive(Debug, Clone)]
pub struct Event {
    pub subscription_id: u64,
    pub context: String,
}

pub fn channel<T>() -> (Sender<T>, Channel<T>) {
    channel::channel()
}
