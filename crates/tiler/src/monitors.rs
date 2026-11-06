use std::mem::size_of;
use windows::core::BOOL;

use windows::Win32::Devices::Display::{
    DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig,
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_MODE_INFO,
    DISPLAYCONFIG_OUTPUT_TECHNOLOGY_DISPLAYPORT_EMBEDDED, DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INTERNAL,
    DISPLAYCONFIG_OUTPUT_TECHNOLOGY_UDI_EMBEDDED, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_SOURCE_DEVICE_NAME, QDC_ONLY_ACTIVE_PATHS,
};
use windows::Win32::Foundation::{LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, HDC, HMONITOR,
    MONITORINFO, MONITORINFOEXW, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTONULL,
};

use tiler_core::Rect;

use crate::{rect_from, Hwnd};

#[derive(Clone, Debug)]
pub struct MonitorInfo {
    /// Device name, e.g. `\\.\DISPLAY1`. Stable across plug/unplug, unlike HMONITOR.
    pub id: String,
    /// Excludes the taskbar.
    pub work: Rect,
    pub full: Rect,
}

unsafe extern "system" fn collect(hm: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
    let v = unsafe { &mut *(data.0 as *mut Vec<HMONITOR>) };
    v.push(hm);
    BOOL(1)
}

fn info(hm: HMONITOR) -> Option<MonitorInfo> {
    let mut mi = MONITORINFOEXW::default();
    mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    let ok = unsafe { GetMonitorInfoW(hm, &mut mi as *mut MONITORINFOEXW as *mut MONITORINFO) };
    if !ok.as_bool() {
        return None;
    }
    let len = mi
        .szDevice
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(mi.szDevice.len());
    Some(MonitorInfo {
        id: String::from_utf16_lossy(&mi.szDevice[..len]),
        work: rect_from(mi.monitorInfo.rcWork),
        full: rect_from(mi.monitorInfo.rcMonitor),
    })
}

/// All monitors, ordered left→right then top→bottom.
pub fn all() -> Vec<MonitorInfo> {
    let mut handles: Vec<HMONITOR> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(collect),
            LPARAM(&mut handles as *mut _ as isize),
        );
    }
    let mut v: Vec<MonitorInfo> = handles.into_iter().filter_map(info).collect();
    v.sort_by_key(|m| (m.full.x, m.full.y));
    v
}

pub fn of_window(h: Hwnd) -> Option<MonitorInfo> {
    info(unsafe { MonitorFromWindow(h.raw(), MONITOR_DEFAULTTONEAREST) })
}

pub fn at_point(x: i32, y: i32) -> Option<MonitorInfo> {
    let hm = unsafe { MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONULL) };
    if hm.is_invalid() {
        None
    } else {
        info(hm)
    }
}

/// GDI device names (e.g. `\\.\DISPLAY1`) of built-in panels: the laptop screen.
/// Uses the display configuration API, which reports how each output is connected.
pub fn internal_ids() -> Vec<String> {
    let (mut n_paths, mut n_modes) = (0u32, 0u32);
    unsafe {
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut n_paths, &mut n_modes).is_err() {
            return Vec::new();
        }
    }
    let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); n_paths as usize];
    let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); n_modes as usize];
    let r = unsafe {
        QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut n_paths,
            paths.as_mut_ptr(),
            &mut n_modes,
            modes.as_mut_ptr(),
            None,
        )
    };
    if r.is_err() {
        return Vec::new();
    }
    paths.truncate(n_paths as usize);
    paths
        .iter()
        .filter(|p| {
            let t = p.targetInfo.outputTechnology;
            t == DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INTERNAL
                || t == DISPLAYCONFIG_OUTPUT_TECHNOLOGY_DISPLAYPORT_EMBEDDED
                || t == DISPLAYCONFIG_OUTPUT_TECHNOLOGY_UDI_EMBEDDED
        })
        .filter_map(|p| {
            let mut name = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
            name.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
            name.header.size = size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
            name.header.adapterId = p.sourceInfo.adapterId;
            name.header.id = p.sourceInfo.id;
            if unsafe { DisplayConfigGetDeviceInfo(&mut name.header) } != 0 {
                return None;
            }
            let n = name
                .viewGdiDeviceName
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(name.viewGdiDeviceName.len());
            Some(String::from_utf16_lossy(&name.viewGdiDeviceName[..n]))
        })
        .collect()
}
