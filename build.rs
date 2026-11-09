use std::{env, fs, path::PathBuf};

fn u16s(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}
fn u32s(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}
fn align(out: &mut Vec<u8>) {
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}
fn text(value: &str) -> Vec<u8> {
    value.encode_utf16().chain(Some(0)).flat_map(u16::to_le_bytes).collect()
}
fn resource(out: &mut Vec<u8>, kind: u16, id: u16, data: &[u8]) {
    align(out);
    u32s(out, data.len() as u32);
    u32s(out, 32);
    for n in [0xffff, kind, 0xffff, id] {
        u16s(out, n);
    }
    u32s(out, 0);
    u16s(out, 0x1030);
    u16s(out, 0x409);
    u32s(out, 0);
    u32s(out, 0);
    out.extend_from_slice(data);
    align(out);
}
fn block(key: &str, kind: u16, value: &[u8], length: u16, children: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    u16s(&mut out, 0);
    u16s(&mut out, length);
    u16s(&mut out, kind);
    out.extend(text(key));
    align(&mut out);
    out.extend_from_slice(value);
    for child in children {
        align(&mut out);
        out.extend_from_slice(child);
    }
    let length = u16::try_from(out.len()).expect("version resource size");
    out[..2].copy_from_slice(&length.to_le_bytes());
    out
}
fn icon(size: u32) -> Vec<u8> {
    let mut out = Vec::new();
    for value in [40, size, size * 2] {
        u32s(&mut out, value);
    }
    u16s(&mut out, 1);
    u16s(&mut out, 32);
    for value in [0, size * size * 4, 0, 0, 0, 0] {
        u32s(&mut out, value);
    }
    for y in (0..size).rev() {
        for x in 0..size {
            let fx = (x as f32 + 0.5) / size as f32;
            let fy = (y as f32 + 0.5) / size as f32;
            let corner = ((fx - 0.5).abs() - 0.31).max(0.).hypot(((fy - 0.5).abs() - 0.31).max(0.));
            let inside = corner <= 0.14;
            let mut wave = false;
            for (i, height) in [0.10f32, 0.20, 0.32, 0.20, 0.10].iter().enumerate() {
                let center = 0.22 + i as f32 * 0.14;
                wave |= (fx - center).abs() < 0.035 && (fy - 0.5).abs() < *height;
            }
            out.extend_from_slice(if !inside {
                &[0, 0, 0, 0]
            } else if wave {
                &[215, 184, 91, 255]
            } else {
                &[34, 32, 32, 255]
            });
        }
    }
    let stride = size.div_ceil(32) * 4;
    for y in (0..size).rev() {
        let mut row = vec![0; stride as usize];
        for x in 0..size {
            let pixel = 40 + ((size - 1 - y) * size + x) * 4;
            if out[pixel as usize + 3] == 0 {
                row[(x / 8) as usize] |= 0x80 >> (x % 8);
            }
        }
        out.extend(row);
    }
    out
}
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let mut res = Vec::new();
    resource(&mut res, 0, 0, &[]);
    let sizes = [16u32, 24, 32, 48, 64];
    let mut group = vec![0, 0, 1, 0];
    u16s(&mut group, sizes.len() as u16);
    for (i, size) in sizes.iter().enumerate() {
        let data = icon(*size);
        resource(&mut res, 3, i as u16 + 1, &data);
        group.extend_from_slice(&[*size as u8, *size as u8, 0, 0]);
        u16s(&mut group, 1);
        u16s(&mut group, 32);
        u32s(&mut group, data.len() as u32);
        u16s(&mut group, i as u16 + 1);
    }
    resource(&mut res, 14, 1, &group);
    let version = env::var("CARGO_PKG_VERSION").expect("version");
    let parts: Vec<u32> = version.split('.').map(|n| n.parse().expect("numeric version")).collect();
    let mut fixed = Vec::new();
    for n in [
        0xfeef04bd,
        0x10000,
        (parts[0] << 16) | parts[1],
        parts[2] << 16,
        (parts[0] << 16) | parts[1],
        parts[2] << 16,
        0x3f,
        0,
        0x40004,
        1,
        0,
        0,
        0,
    ] {
        u32s(&mut fixed, n);
    }
    let strings: Vec<_> = [
        ("CompanyName", "Taskbar Media contributors"),
        ("FileDescription", "Taskbar Media"),
        ("FileVersion", version.as_str()),
        ("ProductName", "Taskbar Media"),
        ("ProductVersion", version.as_str()),
    ]
    .iter()
    .map(|(key, value)| block(key, 1, &text(value), (value.encode_utf16().count() + 1) as u16, &[]))
    .collect();
    let table = block("040904B0", 1, &[], 0, &strings);
    let string_info = block("StringFileInfo", 1, &[], 0, &[table]);
    let translation = block("Translation", 0, &[9, 4, 0xb0, 4], 4, &[]);
    let var = block("VarFileInfo", 1, &[], 0, &[translation]);
    resource(&mut res, 16, 1, &block("VS_VERSION_INFO", 0, &fixed, 52, &[string_info, var]));
    let resource_path = dir.join("media.res");
    fs::write(&resource_path, res).expect("write resource");
    println!("cargo:rustc-link-arg={}", resource_path.display());
    let manifest = dir.join("media.manifest");
    fs::write(&manifest,r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
 <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3"><security><requestedPrivileges><requestedExecutionLevel level="asInvoker" uiAccess="false"/></requestedPrivileges></security></trustInfo>
 <application xmlns="urn:schemas-microsoft-com:asm.v3"><windowsSettings><dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware><dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness><longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware></windowsSettings></application>
 <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1"><application><supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/></application></compatibility>
</assembly>"#).expect("write manifest");
    for binary in ["taskbar-media", "taskbar-media-ui"] {
        println!("cargo:rustc-link-arg-bin={binary}=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg-bin={binary}=/MANIFESTINPUT:{}", manifest.display());
        println!("cargo:rustc-link-arg-bin={binary}=/MANIFESTUAC:NO");
        println!("cargo:rustc-link-arg-bin={binary}=/DEBUG:NONE");
    }
    println!("cargo:rustc-link-arg-cdylib=/DEBUG:NONE");
    let exports = dir.join("media.def");
    fs::write(&exports, "EXPORTS\nDllGetClassObject PRIVATE\nDllCanUnloadNow PRIVATE\n").expect("COM exports");
    println!("cargo:rustc-link-arg-cdylib=/DEF:{}", exports.display());
}
