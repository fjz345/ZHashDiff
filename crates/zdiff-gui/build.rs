use std::{env, io::Cursor, path::PathBuf};

use image::{ImageFormat, imageops::FilterType};

const ICON_PNG: &str = "img/icon.png";
/// Sizes Explorer and the taskbar pick from; each is resampled from the 256 px source.
const ICON_SIZES: [u32; 6] = [16, 24, 32, 48, 64, 256];

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
const MOVEABLE_DISCARDABLE: u16 = 0x1010;
const MOVEABLE_PURE_DISCARDABLE: u16 = 0x1030;

fn main() {
    println!("cargo:rerun-if-changed={ICON_PNG}");
    println!("cargo:rerun-if-changed=build.rs");

    // The exe icon is a Windows resource. Other targets only get the window icon (main.rs).
    if env::var("CARGO_CFG_TARGET_OS").unwrap() != "windows" {
        return;
    }
    // link.exe (and lld-link) take a compiled .res directly; GNU ld needs windres to convert it.
    if env::var("CARGO_CFG_TARGET_ENV").unwrap() != "msvc" {
        println!("cargo:warning=exe icon is only embedded for MSVC targets");
        return;
    }

    let source = image::open(ICON_PNG).expect("read img/icon.png");
    let pngs: Vec<Vec<u8>> = ICON_SIZES
        .iter()
        .map(|&size| {
            let resized = image::imageops::resize(&source, size, size, FilterType::Lanczos3);
            let mut png = Vec::new();
            resized
                .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
                .expect("encode icon png");
            png
        })
        .collect();

    let res = PathBuf::from(env::var("OUT_DIR").unwrap()).join("icon.res");
    std::fs::write(&res, icon_res(&ICON_SIZES, &pngs)).expect("write icon.res");
    println!("cargo:rustc-link-arg-bins={}", res.display());
}

/// A compiled resource file holding one icon group: an RT_ICON per image (PNG-compressed
/// entries, supported since Vista) and the RT_GROUP_ICON directory listing them.
fn icon_res(sizes: &[u32], pngs: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    // A .res file starts with an empty entry that marks it as 32-bit.
    res_entry(&mut out, 0, 0, 0, &[]);

    let mut group = Vec::new();
    group.extend(0u16.to_le_bytes()); // reserved
    group.extend(1u16.to_le_bytes()); // type: icon
    group.extend((pngs.len() as u16).to_le_bytes());
    for (i, (&size, png)) in sizes.iter().zip(pngs).enumerate() {
        let id = i as u16 + 1;
        res_entry(&mut out, RT_ICON, id, MOVEABLE_DISCARDABLE, png);
        // 256 does not fit the byte-sized dimensions; 0 stands for it.
        let dim = if size >= 256 { 0 } else { size as u8 };
        group.extend([dim, dim, 0, 0]); // width, height, palette colors, reserved
        group.extend(1u16.to_le_bytes()); // planes
        group.extend(32u16.to_le_bytes()); // bits per pixel
        group.extend((png.len() as u32).to_le_bytes());
        group.extend(id.to_le_bytes());
    }
    res_entry(&mut out, RT_GROUP_ICON, 1, MOVEABLE_PURE_DISCARDABLE, &group);
    out
}

/// One RESOURCEHEADER with ordinal type and name (32 bytes), then the data padded to 4 bytes.
fn res_entry(out: &mut Vec<u8>, type_id: u16, name_id: u16, flags: u16, data: &[u8]) {
    out.extend((data.len() as u32).to_le_bytes());
    out.extend(32u32.to_le_bytes()); // header size
    out.extend([0xFFFF, type_id, 0xFFFF, name_id].map(u16::to_le_bytes).concat());
    out.extend(0u32.to_le_bytes()); // data version
    out.extend(flags.to_le_bytes());
    out.extend(0u16.to_le_bytes()); // language: neutral
    out.extend(0u32.to_le_bytes()); // version
    out.extend(0u32.to_le_bytes()); // characteristics
    out.extend(data);
    out.resize(out.len().next_multiple_of(4), 0);
}
