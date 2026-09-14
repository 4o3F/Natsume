use std::{
    collections::BTreeMap,
    fs,
    os::unix::{
        fs::{FileTypeExt as _, MetadataExt as _},
        net::UnixStream,
    },
    path::Path,
};

use natsume_local_control_api::ResourceControlError;
use wayland_client::{
    Connection, Dispatch, QueueHandle, WEnum,
    protocol::{wl_output, wl_registry, wl_seat},
};

use crate::session::unavailable;

#[derive(Default)]
struct DisplayState {
    outputs: BTreeMap<u32, bool>,
    seats: BTreeMap<u32, bool>,
}

impl DisplayState {
    fn ready(&self, foreground: bool) -> bool {
        self.outputs.values().any(|ready| *ready)
            && (!foreground || self.seats.values().any(|ready| *ready))
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for DisplayState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        (): &(),
        _connection: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => match interface.as_str() {
                "wl_output" => {
                    state.outputs.insert(name, false);
                    registry.bind::<wl_output::WlOutput, _, _>(name, version.min(2), queue, name);
                }
                "wl_seat" => {
                    state.seats.insert(name, false);
                    registry.bind::<wl_seat::WlSeat, _, _>(name, version.min(5), queue, name);
                }
                _ => {}
            },
            wl_registry::Event::GlobalRemove { name } => {
                state.outputs.remove(&name);
                state.seats.remove(&name);
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_output::WlOutput, u32> for DisplayState {
    fn event(
        state: &mut Self,
        _output: &wl_output::WlOutput,
        event: wl_output::Event,
        name: &u32,
        _connection: &Connection,
        _queue: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Mode {
            flags: WEnum::Value(flags),
            width,
            height,
            ..
        } = event
            && flags.contains(wl_output::Mode::Current)
        {
            state.outputs.insert(*name, width > 0 && height > 0);
        }
    }
}

impl Dispatch<wl_seat::WlSeat, u32> for DisplayState {
    fn event(
        state: &mut Self,
        _seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        name: &u32,
        _connection: &Connection,
        _queue: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities { capabilities } = event {
            let ready = matches!(capabilities, WEnum::Value(capabilities)
                if capabilities.intersects(wl_seat::Capability::Keyboard | wl_seat::Capability::Pointer));
            state.seats.insert(*name, ready);
        }
    }
}

fn connect(directory: &Path, uid: u32, pid: i32) -> Option<UnixStream> {
    fs::read_dir(directory)
        .ok()?
        .filter_map(Result::ok)
        .find_map(|entry| {
            let name = entry.file_name();
            let suffix = name.to_str()?.strip_prefix("wayland-")?;
            if suffix.is_empty()
                || !suffix.bytes().all(|byte| byte.is_ascii_digit())
                || !entry.file_type().ok()?.is_socket()
                || entry.metadata().ok()?.uid() != uid
            {
                return None;
            }
            let stream = UnixStream::connect(entry.path()).ok()?;
            let peer = rustix::net::sockopt::socket_peercred(&stream).ok()?;
            (peer.uid.as_raw() == uid && peer.pid.as_raw_nonzero().get() == pid).then_some(stream)
        })
}

/// Checks the live compositor's Wayland protocol under the fixed role UID.
/// This is display/input capability, not proof of every application's pixels.
/// Blocking protocol I/O stays in the disposable child, bounded by its parent.
pub(crate) fn probe(uid: u32, pid: i32, foreground: bool) -> Result<(), ResourceControlError> {
    let directory = format!("/run/user/{uid}");
    let socket = connect(Path::new(&directory), uid, pid)
        .ok_or_else(|| unavailable("the current Wayland compositor socket is unavailable"))?;
    let connection = Connection::from_socket(socket)
        .map_err(|_| unavailable("Wayland compositor connection failed"))?;
    let mut queue = connection.new_event_queue();
    let _registry = connection.display().get_registry(&queue.handle(), ());
    let mut state = DisplayState::default();
    // Globals arrive first, then their bound output modes and input capabilities.
    // No surface or window is created by this probe.
    for _ in 0..2 {
        queue
            .roundtrip(&mut state)
            .map_err(|_| unavailable("Wayland compositor is not responding"))?;
    }
    if state.ready(foreground) {
        Ok(())
    } else {
        Err(unavailable(
            "Wayland output or input capability is unavailable",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    #[test]
    fn paused_background_input_does_not_prevent_activation_but_foreground_needs_input() {
        let mut state = DisplayState::default();
        assert!(!state.ready(false));
        state.outputs.insert(1, true);
        assert!(state.ready(false));
        assert!(!state.ready(true));
        state.seats.insert(2, true);
        assert!(state.ready(true));
        state.outputs.remove(&1);
        assert!(!state.ready(true));
        assert!(!state.ready(false));
    }

    #[test]
    fn socket_name_does_not_substitute_for_compositor_identity()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let _listener = UnixListener::bind(directory.path().join("wayland-0"))?;
        let uid = rustix::process::getuid().as_raw();
        let pid = i32::try_from(std::process::id())?;
        assert!(connect(directory.path(), uid, pid).is_some());
        assert!(connect(directory.path(), uid, 0).is_none());
        Ok(())
    }
}
