use anyhow::Result;

#[cfg(windows)]
mod platform {
    use anyhow::{Result, bail};
    use std::mem::size_of;
    use windows_sys::Win32::Devices::Display::{
        DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
        DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME, DISPLAYCONFIG_DEVICE_INFO_HEADER,
        DISPLAYCONFIG_DEVICE_INFO_SET_ADVANCED_COLOR_STATE, DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO,
        DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE,
        DISPLAYCONFIG_TARGET_DEVICE_NAME, DisplayConfigGetDeviceInfo, DisplayConfigSetDeviceInfo,
        GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
    };
    use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, LUID};

    #[derive(Clone, Copy)]
    struct Target {
        adapter: LUID,
        id: u32,
    }

    pub fn get(model: Option<&str>) -> Result<bool> {
        let target = find_target(model)?;
        let mut info = DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO {
            header: header(
                DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
                size_of::<DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO>(),
                target,
            ),
            ..Default::default()
        };
        let result = unsafe { DisplayConfigGetDeviceInfo(&mut info.header) };
        if result != ERROR_SUCCESS as i32 {
            bail!("querying Windows HDR state failed with error {result}");
        }
        // DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO packs supported in bit 0 and
        // enabled in bit 1.
        let flags = unsafe { info.Anonymous.value };
        if flags & 1 == 0 {
            bail!("the selected display does not support Windows HDR");
        }
        Ok(flags & 2 != 0)
    }

    pub fn set(model: Option<&str>, enabled: bool) -> Result<()> {
        let target = find_target(model)?;
        let mut state = DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE {
            header: header(
                DISPLAYCONFIG_DEVICE_INFO_SET_ADVANCED_COLOR_STATE,
                size_of::<DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE>(),
                target,
            ),
            ..Default::default()
        };
        state.Anonymous.value = u32::from(enabled);
        let result = unsafe { DisplayConfigSetDeviceInfo(&state.header) };
        if result != ERROR_SUCCESS as i32 {
            bail!(
                "setting Windows HDR {} failed with error {result}",
                on_off(enabled)
            );
        }
        Ok(())
    }

    fn on_off(enabled: bool) -> &'static str {
        if enabled { "on" } else { "off" }
    }

    fn header(kind: i32, size: usize, target: Target) -> DISPLAYCONFIG_DEVICE_INFO_HEADER {
        DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: kind,
            size: size as u32,
            adapterId: target.adapter,
            id: target.id,
        }
    }

    fn find_target(model: Option<&str>) -> Result<Target> {
        let paths = active_paths()?;
        let needle = model.map(str::to_ascii_lowercase);
        let mut named = Vec::new();

        for path in paths {
            let target = Target {
                adapter: path.targetInfo.adapterId,
                id: path.targetInfo.id,
            };
            let mut info = DISPLAYCONFIG_TARGET_DEVICE_NAME {
                header: header(
                    DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                    size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>(),
                    target,
                ),
                ..Default::default()
            };
            if unsafe { DisplayConfigGetDeviceInfo(&mut info.header) } != ERROR_SUCCESS as i32 {
                continue;
            }
            let name = utf16z(&info.monitorFriendlyDeviceName);
            let matches = needle
                .as_deref()
                .is_none_or(|value| name.to_ascii_lowercase().contains(value));
            if matches {
                named.push((name, target));
            }
        }

        match named.as_slice() {
            [(_, target)] => Ok(*target),
            [] => bail!(
                "no active Windows display matched '{}'",
                model.unwrap_or("the default display")
            ),
            _ => bail!(
                "more than one active Windows display matched '{}': {}",
                model.unwrap_or("the default display"),
                named
                    .iter()
                    .map(|(name, _)| name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    fn active_paths() -> Result<Vec<DISPLAYCONFIG_PATH_INFO>> {
        loop {
            let mut path_count = 0;
            let mut mode_count = 0;
            let result = unsafe {
                GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count)
            };
            if result != ERROR_SUCCESS {
                bail!("enumerating Windows displays failed with error {result}");
            }

            let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
            let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];
            let result = unsafe {
                QueryDisplayConfig(
                    QDC_ONLY_ACTIVE_PATHS,
                    &mut path_count,
                    paths.as_mut_ptr(),
                    &mut mode_count,
                    modes.as_mut_ptr(),
                    std::ptr::null_mut(),
                )
            };
            if result == ERROR_INSUFFICIENT_BUFFER {
                continue;
            }
            if result != ERROR_SUCCESS {
                bail!("querying Windows displays failed with error {result}");
            }
            paths.truncate(path_count as usize);
            return Ok(paths);
        }
    }

    fn utf16z(value: &[u16]) -> String {
        String::from_utf16_lossy(
            &value[..value.iter().position(|c| *c == 0).unwrap_or(value.len())],
        )
    }
}

#[cfg(not(windows))]
mod platform {
    use anyhow::{Result, bail};

    pub fn get(_model: Option<&str>) -> Result<bool> {
        bail!("Windows HDR control is only available on Windows")
    }

    pub fn set(_model: Option<&str>, _enabled: bool) -> Result<()> {
        bail!("Windows HDR control is only available on Windows")
    }
}

pub fn get(model: Option<&str>) -> Result<bool> {
    platform::get(model)
}

pub fn set(model: Option<&str>, enabled: bool) -> Result<()> {
    platform::set(model, enabled)
}
