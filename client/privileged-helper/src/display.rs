use natsume_local_control_api::ResourceControlError;
use x11rb::{
    connection::{Connection as _, RequestConnection as _},
    protocol::{dpms, randr, xinput, xproto},
    rust_connection::RustConnection,
};

use crate::session::{rejected, unavailable};

fn local_display(display: &str) -> bool {
    display.strip_prefix(':').is_some_and(|display| {
        let (number, screen) = display.split_once('.').unwrap_or((display, "0"));
        [number, screen].into_iter().all(|part| {
            !part.is_empty()
                && part.bytes().all(|b| b.is_ascii_digit())
                && part.parse::<u16>().is_ok()
        })
    })
}

fn vt_property(property: &xproto::GetPropertyReply) -> Option<bool> {
    if property.type_ != u32::from(xproto::AtomEnum::INTEGER)
        || property.format != 32
        || property.value_len != 1
        || property.bytes_after != 0
    {
        return None;
    }
    match property.value32()?.next()? {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

fn input_device_ready(device: &xinput::XIDeviceInfo, node: &xinput::XIGetPropertyReply) -> bool {
    let xinput::XIGetPropertyItems::Data8(value) = &node.items else {
        return false;
    };
    device.enabled
        && matches!(
            device.type_,
            xinput::DeviceType::SLAVE_POINTER | xinput::DeviceType::SLAVE_KEYBOARD
        )
        && node.type_ == u32::from(xproto::AtomEnum::STRING)
        && node.bytes_after == 0
        && usize::try_from(node.num_items).ok() == Some(value.len())
        && value
            .strip_prefix(b"/dev/input/event")
            .is_some_and(|number| !number.is_empty() && number.iter().all(u8::is_ascii_digit))
}

fn has_enabled_physical_input(
    connection: &RustConnection,
) -> Result<bool, Box<dyn std::error::Error>> {
    xinput::xi_query_version(connection, 2, 0)?.reply()?;
    let atom = xproto::intern_atom(connection, true, b"Device Node")?
        .reply()?
        .atom;
    if atom == u32::from(xproto::AtomEnum::NONE) {
        return Ok(false);
    }
    for device in xinput::xi_query_device(connection, 0_u16)?.reply()?.infos {
        let node = xinput::xi_get_property(
            connection,
            device.deviceid,
            false,
            atom,
            xproto::AtomEnum::STRING.into(),
            0,
            256,
        )?
        .reply()?;
        if input_device_ready(&device, &node) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Returns Xorg's VT state after the output probe. The parent checks it against
/// fresh logind facts; this property alone does not prove physical presentation.
pub(crate) fn probe(display: &str) -> Result<bool, ResourceControlError> {
    if !local_display(display) {
        return Err(rejected("desktop probe requires a local X11 display"));
    }
    let result = || -> Result<Option<bool>, Box<dyn std::error::Error>> {
        let (connection, screen) = RustConnection::connect(Some(display))?;
        // Mutter's PowerSaveMode may remain cached at On after a direct X11
        // power change. Query Xorg itself. Disabled DPMS has undefined level.
        if connection
            .extension_information(dpms::X11_EXTENSION_NAME)?
            .is_some()
            && dpms::capable(&connection)?.reply()?.capable
        {
            let power = dpms::info(&connection)?.reply()?;
            if power.state && power.power_level != dpms::DPMSMode::ON {
                return Ok(None);
            }
        }
        let root = connection
            .setup()
            .roots
            .get(screen)
            .ok_or("missing X11 screen")?
            .root;
        let atom = xproto::intern_atom(&connection, true, b"XFree86_has_VT")?
            .reply()?
            .atom;
        let property = xproto::get_property(
            &connection,
            false,
            root,
            atom,
            xproto::AtomEnum::INTEGER,
            0,
            1,
        )?
        .reply()?;
        let Some(has_vt) = vt_property(&property) else {
            return Ok(None);
        };
        let resources = randr::get_screen_resources_current(&connection, root)?.reply()?;
        for output in resources.outputs {
            let info =
                randr::get_output_info(&connection, output, resources.config_timestamp)?.reply()?;
            if info.status != randr::SetConfig::SUCCESS
                || info.connection != randr::Connection::CONNECTED
                || info.crtc == 0
            {
                continue;
            }
            let crtc = randr::get_crtc_info(&connection, info.crtc, resources.config_timestamp)?
                .reply()?;
            if crtc.status == randr::SetConfig::SUCCESS
                && crtc.mode != 0
                && crtc.width > 0
                && crtc.height > 0
                && crtc.outputs.contains(&output)
            {
                // A failed VT handoff can retain has_VT and cached modes while
                // every physical input device is disabled. Virtual core/XTEST
                // devices stay enabled then and cannot establish interactivity.
                // Background Xorgs normally release their physical input.
                if has_vt && !has_enabled_physical_input(&connection)? {
                    return Ok(None);
                }
                return Ok(Some(has_vt));
            }
        }
        Ok(None)
    };
    if let Ok(Some(has_vt)) = result() {
        Ok(has_vt)
    } else {
        Err(unavailable(
            "X11 desktop output or VT observation is unavailable",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{input_device_ready, local_display, vt_property};
    use x11rb::protocol::{xinput, xproto};

    #[test]
    fn virtual_or_disabled_input_cannot_establish_foreground_readiness() {
        let mut device = xinput::XIDeviceInfo {
            deviceid: 8,
            type_: xinput::DeviceType::SLAVE_KEYBOARD,
            attachment: 3,
            enabled: true,
            name: b"AT Translated Set 2 keyboard".to_vec(),
            classes: Vec::new(),
        };
        let mut node = xinput::XIGetPropertyReply {
            sequence: 0,
            length: 0,
            type_: xproto::AtomEnum::STRING.into(),
            bytes_after: 0,
            num_items: 17,
            items: xinput::XIGetPropertyItems::Data8(b"/dev/input/event1".to_vec()),
        };
        assert!(input_device_ready(&device, &node));
        device.enabled = false;
        assert!(!input_device_ready(&device, &node));
        device.enabled = true;
        device.type_ = xinput::DeviceType::FLOATING_SLAVE;
        assert!(!input_device_ready(&device, &node));
        device.type_ = xinput::DeviceType::SLAVE_POINTER;
        assert!(input_device_ready(&device, &node));
        // XTEST devices are enabled slaves but have no Device Node property.
        node.type_ = xproto::AtomEnum::NONE.into();
        node.num_items = 0;
        node.items = xinput::XIGetPropertyItems::InvalidValue(0);
        assert!(!input_device_ready(&device, &node));
    }

    #[test]
    fn physical_input_requires_a_complete_device_node_property()
    -> Result<(), std::num::TryFromIntError> {
        let device = xinput::XIDeviceInfo {
            deviceid: 6,
            type_: xinput::DeviceType::SLAVE_POINTER,
            attachment: 2,
            enabled: true,
            name: Vec::new(),
            classes: Vec::new(),
        };
        for value in [
            "/dev/input/event0",
            "/dev/input/event42",
            "",
            "/dev/input/event",
            "/tmp/event0",
        ] {
            let mut node = xinput::XIGetPropertyReply {
                sequence: 0,
                length: 0,
                type_: xproto::AtomEnum::STRING.into(),
                bytes_after: 0,
                num_items: u32::try_from(value.len())?,
                items: xinput::XIGetPropertyItems::Data8(value.as_bytes().to_vec()),
            };
            assert_eq!(
                input_device_ready(&device, &node),
                value == "/dev/input/event0" || value == "/dev/input/event42"
            );
            node.bytes_after = 1;
            assert!(!input_device_ready(&device, &node));
        }
        Ok(())
    }

    #[test]
    fn vt_observation_requires_one_complete_integer_boolean() {
        let mut reply = xproto::GetPropertyReply {
            format: 32,
            sequence: 0,
            length: 1,
            type_: xproto::AtomEnum::INTEGER.into(),
            bytes_after: 0,
            value_len: 1,
            value: 0_u32.to_ne_bytes().to_vec(),
        };
        assert_eq!(vt_property(&reply), Some(false));
        reply.value = 1_u32.to_ne_bytes().to_vec();
        assert_eq!(vt_property(&reply), Some(true));
        reply.value = 2_u32.to_ne_bytes().to_vec();
        assert_eq!(vt_property(&reply), None);
        reply.value.clear();
        assert_eq!(vt_property(&reply), None);
        reply.value = 1_u32.to_ne_bytes().to_vec();
        reply.bytes_after = 4;
        assert_eq!(vt_property(&reply), None);
        reply.bytes_after = 0;
        reply.type_ = xproto::AtomEnum::CARDINAL.into();
        assert_eq!(vt_property(&reply), None);
        reply.type_ = xproto::AtomEnum::INTEGER.into();
        reply.format = 8;
        assert_eq!(vt_property(&reply), None);
        reply.format = 32;
        reply.value_len = 0;
        assert_eq!(vt_property(&reply), None);
    }

    #[test]
    fn compositor_display_cannot_select_network_or_arbitrary_socket() {
        for value in [":0", ":1.0", ":42.3"] {
            assert!(local_display(value));
        }
        for value in [
            "",
            ":",
            ":0.",
            ":-1",
            ":0.1.2",
            ":65536",
            "host:0",
            "tcp/host:0",
            "unix:/tmp/display",
            "/tmp/.X11-unix/X0",
        ] {
            assert!(!local_display(value), "{value}");
        }
    }
}
