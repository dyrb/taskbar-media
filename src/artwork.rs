//! Bounded system-thumbnail decoding. Uses the supplied WinRT stream, never a filesystem cache.
use crate::platform::async_op::Operation;
use crate::platform::com::Com;
use crate::platform::event::Event;
use crate::platform::ffi::*;
use crate::platform::{Error, Result, check};
use std::ptr::{null, null_mut};
use std::time::Duration;

#[derive(Debug)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}
const MAX_ENCODED: u64 = 8 * 1024 * 1024;
const MAX_PIXELS: u64 = 16_000_000;
const COMPLETE_STREAM: Guid = Guid::from_u128(0x3dddecf4_1d39_58e8_83b1_dbed541c7f35);
const RANDOM_STREAM: Guid = Guid::from_u128(0x905a0fe1_bc53_11df_8c49_001e4fc686da);
const STREAM: Guid = Guid::from_u128(0x0000000c_0000_0000_c000_000000000046);
const WIC_CLASS: Guid = Guid::from_u128(0xcacaf262_9370_4615_a13b_9f5539da4c0a);
const WIC_FACTORY: Guid = Guid::from_u128(0xec5ec8a9_c395_4314_9c77_54d7a935ff70);
const PBGRA: Guid = Guid::from_u128(0x6fddc324_4e03_4bfe_b185_3d77768dc910);

#[repr(C)]
struct ReferenceVtbl {
    inspectable: InspectableVtbl,
    open: unsafe extern "system" fn(Raw, *mut Raw) -> i32,
}
#[repr(C)]
struct RandomVtbl {
    inspectable: InspectableVtbl,
    size: unsafe extern "system" fn(Raw, *mut u64) -> i32,
}
#[repr(C)]
struct StreamVtbl {
    unknown: UnknownVtbl,
    read: unsafe extern "system" fn(Raw, *mut u8, u32, *mut u32) -> i32,
}
#[repr(C)]
struct FactoryVtbl {
    unknown: UnknownVtbl,
    _filename: usize,
    decoder: unsafe extern "system" fn(Raw, Raw, *const Guid, u32, *mut Raw) -> i32,
    _file_handle_component_decoder_encoder_palette: [usize; 5],
    converter: unsafe extern "system" fn(Raw, *mut Raw) -> i32,
    scaler: unsafe extern "system" fn(Raw, *mut Raw) -> i32,
}
#[repr(C)]
struct DecoderVtbl {
    unknown: UnknownVtbl,
    _capability_initialize_container_info_palette_metadata_preview_color_thumbnail_count: [usize; 10],
    frame: unsafe extern "system" fn(Raw, u32, *mut Raw) -> i32,
}
#[repr(C)]
struct SourceVtbl {
    unknown: UnknownVtbl,
    size: unsafe extern "system" fn(Raw, *mut u32, *mut u32) -> i32,
    _format_resolution_palette: [usize; 3],
    copy: unsafe extern "system" fn(Raw, Raw, u32, u32, *mut u8) -> i32,
}
#[repr(C)]
struct ScalerVtbl {
    source: SourceVtbl,
    initialize: unsafe extern "system" fn(Raw, Raw, u32, u32, u32) -> i32,
}
#[repr(C)]
struct ConverterVtbl {
    source: SourceVtbl,
    initialize: unsafe extern "system" fn(Raw, Raw, *const Guid, u32, Raw, f64, u32) -> i32,
}

pub fn read(reference: &Com, stop: &Event) -> Result<Image> {
    read_cancellable(reference, stop, None)
}
pub(crate) fn read_cancellable(reference: &Com, stop: &Event, invalidated: Option<&Event>) -> Result<Image> {
    // SAFETY: exact IRandomAccessStreamReference returned by GSMTC, used only on its owner MTA.
    unsafe {
        let mut raw = null_mut();
        check((reference.vt::<ReferenceVtbl>().open)(reference.raw(), &mut raw), "open thumbnail stream")?;
        let operation = Operation::<Raw>::new(Com::owned(raw, "thumbnail operation")?, COMPLETE_STREAM)?;
        let stream =
            Com::owned(operation.wait_cancellable(stop, invalidated, Duration::from_secs(10))?, "thumbnail stream")?
                .query(&RANDOM_STREAM)?;
        let mut size = 0;
        check((stream.vt::<RandomVtbl>().size)(stream.raw(), &mut size), "thumbnail encoded size")?;
        if size == 0 || size > MAX_ENCODED {
            return Err(Error::new("thumbnail exceeds encoded size limit", E_BOUNDS));
        }
        check(CreateStreamOverRandomAccessStream(stream.raw(), &STREAM, &mut raw), "adapt thumbnail stream")?;
        let adapted = Com::owned(raw, "thumbnail IStream")?;
        let mut bytes = vec![0; size as usize];
        let mut offset = 0;
        while offset < bytes.len() {
            if stop.wait(0)? || invalidated.is_some_and(|e| e.wait(0).unwrap_or(true)) {
                return Err(Error::new("thumbnail cancelled", E_ABORT));
            }
            let mut n = 0;
            let count = (bytes.len() - offset).min(65536) as u32;
            check(
                (adapted.vt::<StreamVtbl>().read)(adapted.raw(), bytes[offset..].as_mut_ptr(), count, &mut n),
                "read thumbnail bytes",
            )?;
            if n == 0 || n > count {
                return Err(Error::new("truncated thumbnail stream", E_FAIL));
            }
            offset += n as usize;
        }
        let image = decode(&bytes, stop)?;
        if invalidated.is_some_and(|e| e.wait(0).unwrap_or(true)) {
            return Err(Error::new("thumbnail superseded", E_ABORT));
        }
        Ok(image)
    }
}

fn dimensions(width: u32, height: u32) -> Result<(u32, u32)> {
    if width == 0 || height == 0 || width as u64 * height as u64 > MAX_PIXELS {
        return Err(Error::new("thumbnail dimensions exceed limit", E_BOUNDS));
    }
    let largest = width.max(height).max(512);
    Ok((((width as u64 * 512) / largest as u64).max(1) as u32, ((height as u64 * 512) / largest as u64).max(1) as u32))
}
fn decode(bytes: &[u8], stop: &Event) -> Result<Image> {
    if bytes.is_empty() || bytes.len() as u64 > MAX_ENCODED {
        return Err(Error::new("thumbnail byte limit", E_BOUNDS));
    }
    // SAFETY: all WIC calls use typed results, checked sizes, and bounded memory streams.
    unsafe {
        let stream = Com::owned(SHCreateMemStream(bytes.as_ptr(), bytes.len() as u32), "thumbnail memory stream")?;
        let mut raw = null_mut();
        check(CoCreateInstance(&WIC_CLASS, null_mut(), 1, &WIC_FACTORY, &mut raw), "create WIC factory")?;
        let factory = Com::owned(raw, "WIC factory")?;
        let vt = factory.vt::<FactoryVtbl>();
        check((vt.decoder)(factory.raw(), stream.raw(), null(), 0, &mut raw), "decode system thumbnail")?;
        let decoder = Com::owned(raw, "WIC decoder")?;
        check((decoder.vt::<DecoderVtbl>().frame)(decoder.raw(), 0, &mut raw), "thumbnail first frame")?;
        let frame = Com::owned(raw, "WIC frame")?;
        let (mut width, mut height) = (0, 0);
        check((frame.vt::<SourceVtbl>().size)(frame.raw(), &mut width, &mut height), "thumbnail dimensions")?;
        let (width, height) = dimensions(width, height)?;
        if stop.wait(0)? {
            return Err(Error::new("thumbnail cancelled", E_ABORT));
        }
        check((vt.scaler)(factory.raw(), &mut raw), "create thumbnail scaler")?;
        let scaler = Com::owned(raw, "WIC scaler")?;
        check(
            (scaler.vt::<ScalerVtbl>().initialize)(scaler.raw(), frame.raw(), width, height, 3),
            "scale system thumbnail",
        )?;
        check((vt.converter)(factory.raw(), &mut raw), "create thumbnail converter")?;
        let converter = Com::owned(raw, "WIC converter")?;
        check(
            (converter.vt::<ConverterVtbl>().initialize)(converter.raw(), scaler.raw(), &PBGRA, 0, null_mut(), 0., 0),
            "convert thumbnail BGRA",
        )?;
        let mut pixels = vec![0; (width * height * 4) as usize];
        check(
            (converter.vt::<SourceVtbl>().copy)(
                converter.raw(),
                null_mut(),
                width * 4,
                pixels.len() as u32,
                pixels.as_mut_ptr(),
            ),
            "copy thumbnail pixels",
        )?;
        if stop.wait(0)? {
            return Err(Error::new("thumbnail cancelled", E_ABORT));
        }
        Ok(Image { width, height, pixels })
    }
}
#[link(name = "shcore")]
unsafe extern "system" {
    fn CreateStreamOverRandomAccessStream(stream: Raw, iid: *const Guid, out: *mut Raw) -> i32;
}
#[link(name = "shlwapi")]
unsafe extern "system" {
    fn SHCreateMemStream(bytes: *const u8, length: u32) -> Raw;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn image_bounds_preserve_ratio_and_reject_bombs() {
        assert_eq!(dimensions(4000, 2000).unwrap(), (512, 256));
        assert_eq!(dimensions(1, 16000000).unwrap(), (1, 512));
        assert!(dimensions(u32::MAX, u32::MAX).is_err());
        assert!(dimensions(0, 1).is_err());
        assert_eq!(std::mem::offset_of!(FactoryVtbl, converter), 10 * 8);
        assert_eq!(std::mem::offset_of!(FactoryVtbl, scaler), 11 * 8);
        assert_eq!(std::mem::offset_of!(DecoderVtbl, frame), 13 * 8);
    }
    #[test]
    fn wic_rejects_truncated_input() {
        let _apartment = crate::platform::com::Apartment::mta().unwrap();
        let stop = Event::new().unwrap();
        assert!(decode(&[0x89, b'P', b'N', b'G', 13, 10, 26, 10], &stop).is_err());
    }
    #[test]
    fn wic_decodes_real_bitmap_pixels_without_a_file() {
        let _apartment = crate::platform::com::Apartment::mta().unwrap();
        let stop = Event::new().unwrap();
        let mut bmp = vec![0u8; 54 + 4 * 4 * 2];
        bmp[..2].copy_from_slice(b"BM");
        let length = bmp.len() as u32;
        bmp[2..6].copy_from_slice(&length.to_le_bytes());
        bmp[10..14].copy_from_slice(&54u32.to_le_bytes());
        bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
        bmp[18..22].copy_from_slice(&4u32.to_le_bytes());
        bmp[22..26].copy_from_slice(&2u32.to_le_bytes());
        bmp[26..28].copy_from_slice(&1u16.to_le_bytes());
        bmp[28..30].copy_from_slice(&32u16.to_le_bytes());
        for p in bmp[54..].chunks_exact_mut(4) {
            p.copy_from_slice(&[20, 40, 60, 0]);
        }
        let image = decode(&bmp, &stop).unwrap();
        assert_eq!((image.width, image.height), (4, 2));
        assert_eq!(image.pixels.len(), 32);
        assert_eq!(&image.pixels[..3], &[20, 40, 60]);
    }
}
