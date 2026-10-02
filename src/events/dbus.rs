use std::{
    ffi::{CStr, CString},
    sync::mpsc::{self, Receiver, Sender as CommandSender},
    thread::{self, JoinHandle},
};

use calloop::channel::Sender;

use crate::{config::SubscriptionBus, events::Event};

enum Command {
    Register {
        id: u64,
        bus: SubscriptionBus,
        rule: String,
        subscription: String,
    },
    Unregister {
        id: u64,
    },
    Shutdown,
}

pub struct Dispatcher {
    commands: CommandSender<Command>,
    thread: Option<JoinHandle<()>>,
}

unsafe extern "C" {
    fn cellbar_dbus_new(
        callback: extern "C" fn(u64, *const libc::c_char, *const libc::c_char, *mut libc::c_void),
        userdata: *mut libc::c_void,
    ) -> *mut libc::c_void;
    fn cellbar_dbus_wake(ctx: *mut libc::c_void);
    fn cellbar_dbus_add_match(
        ctx: *mut libc::c_void,
        is_system: libc::c_int,
        sub_id: u64,
        subscription: *const libc::c_char,
        match_rule: *const libc::c_char,
    ) -> *mut libc::c_void;
    fn cellbar_dbus_remove_match(ctx: *mut libc::c_void, sub_id: u64);
    fn cellbar_dbus_step(ctx: *mut libc::c_void, timeout_ms: libc::c_int) -> libc::c_int;
    fn cellbar_dbus_free(ctx: *mut libc::c_void);
}

extern "C" fn on_dbus_signal(
    sub_id: u64,
    _subscription: *const libc::c_char,
    json: *const libc::c_char,
    userdata: *mut libc::c_void,
) {
    if json.is_null() || userdata.is_null() {
        return;
    }
    unsafe {
        let sender = &*(userdata as *const Sender<Event>);
        if let Ok(str_slice) = CStr::from_ptr(json).to_str() {
            let _ = sender.send(Event {
                subscription_id: sub_id,
                context: str_slice.to_owned(),
            });
        }
    }
}

pub fn start(event_sender: Sender<Event>) -> Dispatcher {
    let (commands, receiver) = mpsc::channel();
    let thread = thread::Builder::new()
        .name("cellbar-dbus".to_owned())
        .stack_size(128 * 1024)
        .spawn(move || run(receiver, event_sender))
        .expect("failed to start D-Bus dispatcher thread");
    Dispatcher {
        commands,
        thread: Some(thread),
    }
}

impl Dispatcher {
    pub fn register(&self, id: u64, bus: SubscriptionBus, rule: String, subscription: String) {
        let _ = self.commands.send(Command::Register {
            id,
            bus,
            rule,
            subscription,
        });
    }

    pub fn unregister(&self, id: u64) {
        let _ = self.commands.send(Command::Unregister { id });
    }
}

impl Drop for Dispatcher {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(receiver: Receiver<Command>, event_sender: Sender<Event>) {
    let ctx = unsafe {
        cellbar_dbus_new(
            on_dbus_signal,
            &event_sender as *const Sender<Event> as *mut libc::c_void,
        )
    };
    if ctx.is_null() {
        return;
    }

    loop {
        while let Ok(cmd) = receiver.try_recv() {
            match cmd {
                Command::Register {
                    id,
                    bus,
                    rule,
                    subscription,
                } => {
                    let is_system = if bus == SubscriptionBus::System { 1 } else { 0 };
                    let c_sub = CString::new(subscription).unwrap_or_default();
                    let c_rule = CString::new(rule).unwrap_or_default();
                    unsafe {
                        cellbar_dbus_add_match(ctx, is_system, id, c_sub.as_ptr(), c_rule.as_ptr());
                        cellbar_dbus_wake(ctx);
                    }
                }
                Command::Unregister { id } => unsafe {
                    cellbar_dbus_remove_match(ctx, id);
                    cellbar_dbus_wake(ctx);
                },
                Command::Shutdown => {
                    unsafe { cellbar_dbus_free(ctx) };
                    return;
                }
            }
        }

        let r = unsafe { cellbar_dbus_step(ctx, 250) };
        if r < 0 {
            break;
        }
    }

    unsafe { cellbar_dbus_free(ctx) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dbus_dispatcher_starts_and_stops_cleanly() {
        let (sender, _receiver) = calloop::channel::channel();
        let dispatcher = start(sender);
        dispatcher.register(
            1,
            SubscriptionBus::Session,
            "type='signal',interface='org.freedesktop.DBus'".into(),
            "dbus".into(),
        );
        dispatcher.unregister(1);
    }
}
