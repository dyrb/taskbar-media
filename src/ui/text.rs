//! DirectWrite rasterisation on metadata/layout changes; paint only blends cached pixels.
use super::{Buffer, api::*, fill};
use crate::platform::{Error, Result, check, com::Com, ffi::*};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicU32, Ordering};

const FACTORY: Guid = Guid::from_u128(0xb859ee5a_d838_4b5b_a2e8_1adc7d93db48);
const TARGET1: Guid = Guid::from_u128(0x791e8298_3ef3_4230_9880_c9bdecc42064);
const RENDERER: Guid = Guid::from_u128(0xef8a8135_5cc6_45fe_8825_c5a0724eb819);
const SNAPPING: Guid = Guid::from_u128(0xeaf3a2da_ecf4_4d24_b644_b34f6842024b);

#[repr(C)]
struct FactoryVtbl {
    unknown: UnknownVtbl,
    _fonts: [usize; 7],
    params: unsafe extern "system" fn(Raw, *mut Raw) -> i32,
    _loaders: [usize; 4],
    format: unsafe extern "system" fn(Raw, *const u16, Raw, u32, u32, u32, f32, *const u16, *mut Raw) -> i32,
    _typography: usize,
    interop: unsafe extern "system" fn(Raw, *mut Raw) -> i32,
    layout: unsafe extern "system" fn(Raw, *const u16, u32, Raw, f32, f32, *mut Raw) -> i32,
    _compatible: usize,
    ellipsis: unsafe extern "system" fn(Raw, Raw, *mut Raw) -> i32,
}
#[repr(C)]
struct FormatVtbl {
    unknown: UnknownVtbl,
    _alignment: usize,
    paragraph: unsafe extern "system" fn(Raw, u32) -> i32,
    wrapping: unsafe extern "system" fn(Raw, u32) -> i32,
    _reading_flow_tab: [usize; 3],
    trimming: unsafe extern "system" fn(Raw, *const [u32; 3], Raw) -> i32,
    _remaining: [usize; 18],
}
#[repr(C)]
struct LayoutVtbl {
    format: FormatVtbl,
    _styles: [usize; 30],
    draw: unsafe extern "system" fn(Raw, Raw, Raw, f32, f32) -> i32,
    _lines: usize,
    metrics: unsafe extern "system" fn(Raw, *mut Metrics) -> i32,
}
#[repr(C)]
#[derive(Default)]
struct Metrics {
    left: f32,
    top: f32,
    width: f32,
    trailing_width: f32,
    height: f32,
    layout_width: f32,
    layout_height: f32,
    bidi_depth: u32,
    line_count: u32,
}
#[repr(C)]
struct InteropVtbl {
    unknown: UnknownVtbl,
    _fonts: [usize; 4],
    target: unsafe extern "system" fn(Raw, Raw, u32, u32, *mut Raw) -> i32,
}
#[repr(C)]
struct TargetVtbl {
    unknown: UnknownVtbl,
    glyph: unsafe extern "system" fn(Raw, f32, f32, u32, Raw, Raw, u32, *mut Rect) -> i32,
    dc: unsafe extern "system" fn(Raw) -> Raw,
    _get_scale: usize,
    scale: unsafe extern "system" fn(Raw, f32) -> i32,
    _transform_size: [usize; 4],
    _get_antialias: usize,
    antialias: unsafe extern "system" fn(Raw, u32) -> i32,
}
#[repr(C)]
struct RendererVtbl {
    unknown: UnknownVtbl,
    snapping: unsafe extern "system" fn(Raw, Raw, *mut i32) -> i32,
    transform: unsafe extern "system" fn(Raw, Raw, *mut [f32; 6]) -> i32,
    scale: unsafe extern "system" fn(Raw, Raw, *mut f32) -> i32,
    glyph: unsafe extern "system" fn(Raw, Raw, f32, f32, u32, Raw, Raw, Raw) -> i32,
    underline: unsafe extern "system" fn(Raw, Raw, f32, f32, *const [f32; 3], Raw) -> i32,
    strike: unsafe extern "system" fn(Raw, Raw, f32, f32, *const [f32; 3], Raw) -> i32,
    inline: unsafe extern "system" fn(Raw, Raw, f32, f32, Raw, i32, i32, Raw) -> i32,
}
#[repr(C)]
struct InlineVtbl {
    unknown: UnknownVtbl,
    draw: unsafe extern "system" fn(Raw, Raw, Raw, f32, f32, i32, i32, Raw) -> i32,
}
#[repr(C)]
struct Renderer {
    vt: &'static RendererVtbl,
    refs: AtomicU32,
    target: Com,
    params: Com,
    scale: f32,
}
static VT: RendererVtbl = RendererVtbl {
    unknown: UnknownVtbl { query, add_ref, release },
    snapping,
    transform,
    scale,
    glyph,
    underline: decoration,
    strike: decoration,
    inline,
};
unsafe extern "system" fn query(this: Raw, iid: *const Guid, out: *mut Raw) -> i32 {
    if iid.is_null() || out.is_null() {
        return E_POINTER;
    }
    // SAFETY: standard COM caller buffers; all exposed interfaces share one identity.
    unsafe {
        *out = null_mut();
        if [IID_IUNKNOWN, RENDERER, SNAPPING].contains(&*iid) {
            *out = this;
            add_ref(this);
            0
        } else {
            E_NOINTERFACE
        }
    }
}
unsafe extern "system" fn add_ref(this: Raw) -> u32 {
    // SAFETY: one live Renderer allocation is referenced by every interface.
    unsafe { (&*this.cast::<Renderer>()).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}
unsafe extern "system" fn release(this: Raw) -> u32 {
    // SAFETY: the final COM release uniquely owns the allocation and its target.
    unsafe {
        let n = (&*this.cast::<Renderer>()).refs.fetch_sub(1, Ordering::AcqRel) - 1;
        if n == 0 {
            drop(Box::from_raw(this.cast::<Renderer>()));
        }
        n
    }
}
unsafe extern "system" fn snapping(_: Raw, _: Raw, out: *mut i32) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: DirectWrite supplies a writable BOOL.
    unsafe {
        *out = 0;
    }
    0
}
unsafe extern "system" fn transform(_: Raw, _: Raw, out: *mut [f32; 6]) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: DirectWrite supplies a writable DWRITE_MATRIX.
    unsafe {
        *out = [1., 0., 0., 1., 0., 0.];
    }
    0
}
unsafe extern "system" fn scale(this: Raw, _: Raw, out: *mut f32) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: synchronous renderer callback, target DPI is immutable.
    unsafe {
        *out = (&*this.cast::<Renderer>()).scale;
    }
    0
}
unsafe extern "system" fn glyph(this: Raw, _: Raw, x: f32, y: f32, mode: u32, run: Raw, _: Raw, _: Raw) -> i32 {
    // SAFETY: DirectWrite owns the glyph run for this synchronous call; the renderer owns its target.
    unsafe {
        let r = &*this.cast::<Renderer>();
        (r.target.vt::<TargetVtbl>().glyph)(r.target.raw(), x, y, mode, run, r.params.raw(), 0xffffff, null_mut())
    }
}
unsafe extern "system" fn decoration(this: Raw, _: Raw, x: f32, y: f32, spec: *const [f32; 3], _: Raw) -> i32 {
    if spec.is_null() {
        return E_POINTER;
    }
    // SAFETY: underline and strikethrough both begin with width, thickness and offset.
    unsafe {
        let r = &*this.cast::<Renderer>();
        let [w, h, offset] = *spec;
        let left = (x * r.scale).floor() as i32;
        let top = ((y + offset) * r.scale).floor() as i32;
        fill(
            (r.target.vt::<TargetVtbl>().dc)(r.target.raw()),
            Rect {
                left,
                top,
                right: ((x + w) * r.scale).ceil() as i32,
                bottom: top + (h * r.scale).ceil().max(1.) as i32,
            },
            0xffffff,
        );
    }
    0
}
unsafe extern "system" fn inline(
    this: Raw,
    ctx: Raw,
    x: f32,
    y: f32,
    object: Raw,
    sideways: i32,
    rtl: i32,
    effect: Raw,
) -> i32 {
    if object.is_null() {
        return E_POINTER;
    }
    // SAFETY: the layout owns this inline object (including the native ellipsis sign) during Draw.
    unsafe {
        let vt = &**object.cast::<*const InlineVtbl>();
        (vt.draw)(object, ctx, this, x, y, sideways, rtl, effect)
    }
}

pub(super) struct Engine {
    factory: Com,
    interop: Com,
    params: Com,
}
pub(super) struct Raster {
    pub buffer: Buffer,
}
#[derive(Clone, Copy, PartialEq)]
pub(super) struct Style {
    pub dpi: u32,
    pub size: f32,
    pub weight: u32,
    pub color: u32,
    pub wrap: bool,
}
#[derive(Default)]
pub(super) struct Cached {
    key: Option<(Vec<u16>, (i32, i32), Style)>,
    pub raster: Option<Raster>,
}
impl Cached {
    pub fn update(&mut self, engine: Option<&Engine>, text: &[u16], area: Rect, style: Style) {
        let extent = (area.width(), area.height());
        if self.key.as_ref().is_some_and(|(old, size, old_style)| old == text && *size == extent && *old_style == style)
            && (self.raster.is_some() || engine.is_none() || area.width() == 0 || area.height() == 0)
        {
            return;
        }
        self.raster = engine.and_then(|e| e.raster(text, area, style).ok());
        self.key = Some((text.to_vec(), extent, style));
    }
}
impl Engine {
    pub fn new() -> Result<Self> {
        // SAFETY: SDK IDWriteFactory, owned COM results stay on the UI thread.
        unsafe {
            let mut raw = null_mut();
            check(DWriteCreateFactory(0, &FACTORY, &mut raw), "create text factory")?;
            let factory = Com::owned(raw, "DirectWrite factory")?;
            check((factory.vt::<FactoryVtbl>().interop)(factory.raw(), &mut raw), "create text interop")?;
            let interop = Com::owned(raw, "text interop")?;
            check((factory.vt::<FactoryVtbl>().params)(factory.raw(), &mut raw), "create text parameters")?;
            let params = Com::owned(raw, "text parameters")?;
            Ok(Self { factory, interop, params })
        }
    }
    pub fn raster(&self, text: &[u16], area: Rect, style: Style) -> Result<Raster> {
        self.render(text, area, style, false)
    }
    pub fn raster_fit(&self, text: &[u16], area: Rect, style: Style) -> Result<Raster> {
        self.render(text, area, style, true)
    }
    fn render(&self, text: &[u16], mut area: Rect, style: Style, fit: bool) -> Result<Raster> {
        let Style { dpi, size: font_size, weight, color, wrap } = style;
        if area.width() == 0 || area.height() == 0 || text.is_empty() {
            return Err(Error::new("empty text destination", E_BOUNDS));
        }
        let pixels = dpi.max(96) as f32 / 96.;
        // SAFETY: typed SDK vtables, bounded targets and UTF-16 buffers; no borrowed pointers escape Draw.
        unsafe {
            let mut raw = null_mut();
            let factory = self.factory.vt::<FactoryVtbl>();
            check(
                (factory.format)(
                    self.factory.raw(),
                    wide("Segoe UI").as_ptr(),
                    null_mut(),
                    weight,
                    0,
                    5,
                    font_size,
                    wide("").as_ptr(),
                    &mut raw,
                ),
                "create text format",
            )?;
            let format = Com::owned(raw, "text format")?;
            let fv = format.vt::<FormatVtbl>();
            check((fv.paragraph)(format.raw(), if wrap { 0 } else { 2 }), "align text")?;
            check((fv.wrapping)(format.raw(), u32::from(!wrap)), "wrap text")?;
            check((factory.ellipsis)(self.factory.raw(), format.raw(), &mut raw), "create text ellipsis")?;
            let ellipsis = Com::owned(raw, "text ellipsis")?;
            check((fv.trimming)(format.raw(), &[1, 0, 0], ellipsis.raw()), "trim text")?;
            let count = text.len().saturating_sub(usize::from(text.last() == Some(&0))).min(32768);
            check(
                (factory.layout)(
                    self.factory.raw(),
                    text.as_ptr(),
                    count as u32,
                    format.raw(),
                    area.width() as f32 / pixels,
                    area.height() as f32 / pixels,
                    &mut raw,
                ),
                "layout text",
            )?;
            let layout = Com::owned(raw, "text layout")?;
            if fit {
                let mut metrics = Metrics::default();
                check((layout.vt::<LayoutVtbl>().metrics)(layout.raw(), &mut metrics), "measure text")?;
                area.bottom = area.top + ((metrics.height * pixels).ceil() as i32).clamp(1, area.height());
            }
            check(
                (self.interop.vt::<InteropVtbl>().target)(
                    self.interop.raw(),
                    null_mut(),
                    area.width() as u32,
                    area.height() as u32,
                    &mut raw,
                ),
                "create text bitmap",
            )?;
            let target = Com::owned(raw, "text bitmap")?.query(&TARGET1)?;
            let tv = target.vt::<TargetVtbl>();
            check((tv.scale)(target.raw(), pixels), "scale text")?;
            check((tv.antialias)(target.raw(), 1), "antialias transparent text")?;
            let dc = (tv.dc)(target.raw());
            fill(dc, Rect { left: 0, top: 0, right: area.width(), bottom: area.height() }, 0);
            let renderer = Com::owned(
                Box::into_raw(Box::new(Renderer {
                    vt: &VT,
                    refs: AtomicU32::new(1),
                    target,
                    params: self.params.clone(),
                    scale: pixels,
                }))
                .cast(),
                "text renderer",
            )?;
            check((layout.vt::<LayoutVtbl>().draw)(layout.raw(), null_mut(), renderer.raw(), 0., 0.), "draw text")?;
            let buffer = Buffer::new(null_mut(), area.width(), area.height())
                .ok_or(Error::new("allocate text pixels", E_FAIL))?;
            if BitBlt(buffer.dc, 0, 0, area.width(), area.height(), dc, 0, 0, 0x00cc0020) == 0 {
                return Err(Error::last("copy text bitmap"));
            }
            GdiFlush();
            for pixel in
                std::slice::from_raw_parts_mut(buffer.bits.cast::<u32>(), (area.width() * area.height()) as usize)
            {
                let a = *pixel & 255;
                *pixel = premultiply(color, a);
            }
            Ok(Raster { buffer })
        }
    }
}
pub(super) fn premultiply(color: u32, a: u32) -> u32 {
    let r = ((color & 255) * a + 127) / 255;
    let g = (((color >> 8) & 255) * a + 127) / 255;
    let b = (((color >> 16) & 255) * a + 127) / 255;
    (a << 24) | (r << 16) | (g << 8) | b
}
impl Raster {
    pub fn draw(&self, dc: Raw, area: Rect) {
        // SAFETY: immutable cached PBGRA DIB is blended at 1:1 physical pixel size.
        unsafe {
            AlphaBlend(
                dc,
                area.left,
                area.top,
                self.buffer.width,
                self.buffer.height,
                self.buffer.dc,
                0,
                0,
                self.buffer.width,
                self.buffer.height,
                0x01ff0000,
            );
        }
    }
}
#[link(name = "dwrite")]
unsafe extern "system" {
    fn DWriteCreateFactory(kind: u32, iid: *const Guid, factory: *mut Raw) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sdk_offsets_and_real_grayscale_text_are_valid() {
        let p = size_of::<usize>();
        assert_eq!(std::mem::offset_of!(FactoryVtbl, format), 15 * p);
        assert_eq!(std::mem::offset_of!(LayoutVtbl, draw), 58 * p);
        assert_eq!(std::mem::offset_of!(LayoutVtbl, metrics), 60 * p);
        assert_eq!(size_of::<Metrics>(), 36);
        assert_eq!(std::mem::offset_of!(TargetVtbl, antialias), 12 * p);
        let engine = Engine::new().unwrap();
        let raster = engine
            .raster(
                &wide("Music Музыка 日本語"),
                Rect { left: 0, top: 0, right: 160, bottom: 24 },
                Style { dpi: 96, size: 15., weight: 600, color: 0x0080ff, wrap: false },
            )
            .unwrap();
        // SAFETY: owned offscreen DIB; the renderer has completed all native drawing.
        let pixels = unsafe { std::slice::from_raw_parts(raster.buffer.bits.cast::<u32>(), 160 * 24) };
        assert!(pixels.iter().any(|&p| p >> 24 > 0));
        assert!(
            pixels.iter().all(|&p| (p & 255) <= p >> 24 && ((p >> 8) & 255) <= p >> 24 && ((p >> 16) & 255) <= p >> 24)
        );
        let area = Rect { left: 0, top: 0, right: 188, bottom: 40 };
        let style = Style { dpi: 96, size: 14., weight: 600, color: 0xffffff, wrap: true };
        let short = engine.raster_fit(&wide("RENT"), area, style).unwrap();
        let long = engine.raster_fit(&wide("A longer song title that needs more than one line"), area, style).unwrap();
        assert!(short.buffer.height < 24);
        assert!(long.buffer.height > short.buffer.height && long.buffer.height <= 40);
    }
}
