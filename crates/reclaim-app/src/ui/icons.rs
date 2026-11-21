//! Window and tray icons from `assets/icons` (drawn by the owner, "Neon orbit").
//!
//! The tray takes one bitmap and Windows scales it to the notification-area size, so the
//! PNG closest to that size for the system DPI is used. PNGs are decoded on demand: only
//! when the window is created or the tray state changes.

use reclaim_app::vm::TrayState;
use windows_sys::Win32::UI::HiDpi::GetDpiForSystem;

macro_rules! sizes {
    ($state:literal) => {
        [
            (
                16,
                include_bytes!(concat!("../../assets/icons/", $state, "_16.png")).as_slice(),
            ),
            (
                20,
                include_bytes!(concat!("../../assets/icons/", $state, "_20.png")).as_slice(),
            ),
            (
                24,
                include_bytes!(concat!("../../assets/icons/", $state, "_24.png")).as_slice(),
            ),
            (
                32,
                include_bytes!(concat!("../../assets/icons/", $state, "_32.png")).as_slice(),
            ),
            (
                40,
                include_bytes!(concat!("../../assets/icons/", $state, "_40.png")).as_slice(),
            ),
            (
                48,
                include_bytes!(concat!("../../assets/icons/", $state, "_48.png")).as_slice(),
            ),
        ]
    };
}

type Sizes = [(u32, &'static [u8]); 6];

const NORMAL: Sizes = sizes!("normal");
const HIGH: Sizes = sizes!("high");
const WORKING: Sizes = sizes!("working");
const UNAVAILABLE: Sizes = sizes!("unavailable");
const APP_48: &[u8] = include_bytes!("../../assets/icons/app_48.png");

/// Decodes an 8-bit RGBA (or convertible) PNG.
fn decode(bytes: &[u8]) -> Option<slint::Image> {
    let mut d = png::Decoder::new(std::io::Cursor::new(bytes));
    d.set_transformations(
        png::Transformations::normalize_to_color8() | png::Transformations::ALPHA,
    );
    let mut r = d.read_info().ok()?;
    let mut buf = vec![0; r.output_buffer_size()?];
    let info = r.next_frame(&mut buf).ok()?;
    if info.color_type != png::ColorType::Rgba {
        return None;
    }
    let px = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        buf.get(..info.buffer_size())?,
        info.width,
        info.height,
    );
    Some(slint::Image::from_rgba8(px))
}

/// Notification-area icon size in pixels for the system DPI (16 at 100 %).
fn tray_px() -> u32 {
    // SAFETY: no arguments; reads the system DPI.
    let dpi = unsafe { GetDpiForSystem() };
    let dpi = if dpi == 0 { 96 } else { dpi };
    (16 * dpi).div_ceil(96)
}

/// Tray icon for `state`.
pub fn tray(state: TrayState) -> Option<slint::Image> {
    let set = match state {
        TrayState::Normal => &NORMAL,
        TrayState::High => &HIGH,
        TrayState::Working => &WORKING,
        TrayState::Unavailable => &UNAVAILABLE,
    };
    let want = tray_px();
    let (_, bytes) = set
        .iter()
        .find(|(px, _)| *px >= want)
        .unwrap_or(&set[set.len() - 1]);
    decode(bytes)
}

/// Window icon (taskbar and Alt+Tab use it while the window is open).
pub fn app() -> Option<slint::Image> {
    decode(APP_48)
}
