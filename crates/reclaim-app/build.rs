//! Embeds the application icon (`assets/icons/app.ico`) into `reclaim.exe`.
//!
//! The icon resource is written as a compiled `.res` file directly, so the build needs
//! no resource compiler (`rc.exe` / `llvm-rc`) and no extra build dependency; the MSVC
//! linker takes `.res` inputs as they are.

use std::path::PathBuf;

const ICO: &str = "assets/icons/app.ico";
const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
/// Lowest group id: Explorer shows it as the file icon.
const GROUP_ID: u16 = 1;

fn main() {
    println!("cargo:rerun-if-changed={ICO}");
    println!("cargo:rerun-if-changed=build.rs");
    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if !(windows && msvc) {
        return;
    }
    let ico = std::fs::read(ICO).expect("read app.ico");
    let res = icon_res(&ico).expect("app.ico is not a valid icon file");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("reclaim-icon.res");
    std::fs::write(&out, res).expect("write .res");
    println!("cargo:rustc-link-arg-bin=reclaim={}", out.display());
}

fn u16_at(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(i..i + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

/// One resource entry with ordinal type and name (32-byte header), padded to 4 bytes.
fn entry(out: &mut Vec<u8>, kind: u16, id: u16, flags: u16, data: &[u8]) {
    out.extend((data.len() as u32).to_le_bytes()); // DataSize
    out.extend(32u32.to_le_bytes()); // HeaderSize
    out.extend([0xFF, 0xFF]);
    out.extend(kind.to_le_bytes()); // TYPE
    out.extend([0xFF, 0xFF]);
    out.extend(id.to_le_bytes()); // NAME
    out.extend(0u32.to_le_bytes()); // DataVersion
    out.extend(flags.to_le_bytes()); // MemoryFlags
    out.extend(0u16.to_le_bytes()); // LanguageId (neutral)
    out.extend(0u32.to_le_bytes()); // Version
    out.extend(0u32.to_le_bytes()); // Characteristics
    out.extend(data);
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}

/// `.ico` file → `.res` with one `RT_ICON` per image and an `RT_GROUP_ICON` directory.
fn icon_res(ico: &[u8]) -> Option<Vec<u8>> {
    if u16_at(ico, 0)? != 0 || u16_at(ico, 2)? != 1 {
        return None;
    }
    let count = u16_at(ico, 4)?;
    let mut res = Vec::new();
    // A .res file starts with an empty entry.
    entry(&mut res, 0, 0, 0, &[]);
    let mut group = Vec::new();
    group.extend(0u16.to_le_bytes());
    group.extend(1u16.to_le_bytes());
    group.extend(count.to_le_bytes());
    for i in 0..count {
        let e = 6 + usize::from(i) * 16;
        let head = ico.get(e..e + 8)?; // width, height, colours, reserved, planes, bit count
        let size = u32_at(ico, e + 8)?;
        let offset = u32_at(ico, e + 12)? as usize;
        let image = ico.get(offset..offset + size as usize)?;
        let id = i + 1;
        entry(&mut res, RT_ICON, id, 0x1010, image);
        group.extend(head);
        group.extend(size.to_le_bytes());
        group.extend(id.to_le_bytes());
    }
    entry(&mut res, RT_GROUP_ICON, GROUP_ID, 0x1030, &group);
    Some(res)
}
