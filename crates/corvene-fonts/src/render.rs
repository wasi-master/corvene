//! Glyphs and shaped text rasterised with tiny-skia, at the device pixels
//! they will cover (the UI asks for its scale factor's size, so nothing is
//! resampled on screen).
//!
//! Outlines come from skrifa (glyf, CFF, CFF2, at any variation location),
//! colour glyphs are painted from `COLR` v0 / v1 with palette 0 (sweep
//! gradients drawn as their middle colour), bitmap glyphs (`sbix`, `CBDT`)
//! are drawn from their PNG strikes. Text is shaped with harfrust.

use harfrust::{Feature, ShaperData, ShaperInstance, UnicodeBuffer, Variation};
use skrifa::color::{
    Brush, ColorPainter, ColorStop, CompositeMode, Extend, Transform as ColorTransform,
};
use skrifa::instance::{LocationRef, NormalizedCoord, Size};
use skrifa::metrics::BoundingBox;
use skrifa::raw::types::{GlyphId, Tag};
use skrifa::{FontRef, MetadataProvider};
use tiny_skia::{
    BlendMode, Color, FillRule, GradientStop, LinearGradient, Mask, Paint, Path, PathBuilder,
    Pixmap, PixmapPaint, Point, RadialGradient, Rect, Shader, SpreadMode, Transform,
};

use crate::outline::{GlyphPath, PathCmd};

/// Largest raster side; bigger requests are clamped (and scaled down).
pub const MAX_SIDE: u32 = 8192;

/// A colour, not premultiplied.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Ink {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Ink {
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    fn color(self) -> Color {
        Color::from_rgba8(self.r, self.g, self.b, self.a)
    }
}

/// Pixels, premultiplied RGBA, row by row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl Raster {
    fn from_pixmap(p: Pixmap) -> Self {
        Self {
            width: p.width(),
            height: p.height(),
            data: p.take(),
        }
    }

    /// An empty 1×1 raster.
    pub fn empty() -> Self {
        Self {
            width: 1,
            height: 1,
            data: vec![0; 4],
        }
    }

    /// Straight-alpha BGRA, what GPUI's image sprites take.
    pub fn bgra(&self) -> Vec<u8> {
        let mut out = self.data.clone();
        for px in out.as_chunks_mut::<4>().0 {
            let a = u32::from(px[3]);
            if a != 0 && a != 255 {
                for c in &mut px[..3] {
                    *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
                }
            }
            px.swap(0, 2);
        }
        out
    }

    /// Encoded as PNG.
    pub fn png(&self) -> Option<Vec<u8>> {
        let pixmap = Pixmap::from_vec(
            self.data.clone(),
            tiny_skia::IntSize::from_wh(self.width, self.height)?,
        )?;
        pixmap.encode_png().ok()
    }

    /// Whether any pixel is drawn.
    pub fn is_blank(&self) -> bool {
        self.data.as_chunks::<4>().0.iter().all(|p| p[3] == 0)
    }
}

fn pixmap(width: f32, height: f32) -> Pixmap {
    let w = (width.ceil() as u32).clamp(1, MAX_SIDE);
    let h = (height.ceil() as u32).clamp(1, MAX_SIDE);
    // 1×1 always fits
    Pixmap::new(w, h).unwrap_or_else(|| Pixmap::new(1, 1).expect("1x1 pixmap"))
}

/// A tiny-skia path of `path`, mapped by `t`.
fn sk_path(path: &GlyphPath, t: Transform) -> Option<Path> {
    let mut b = PathBuilder::new();
    for cmd in &path.cmds {
        match *cmd {
            PathCmd::MoveTo(x, y) => b.move_to(x, y),
            PathCmd::LineTo(x, y) => b.line_to(x, y),
            PathCmd::QuadTo(cx, cy, x, y) => b.quad_to(cx, cy, x, y),
            PathCmd::CubicTo(a, c, d, e, x, y) => b.cubic_to(a, c, d, e, x, y),
            PathCmd::Close => b.close(),
        }
    }
    b.finish()?.transform(t)
}

/// Fills `path` (font units, y up) into `pixmap` with `t` mapping font
/// units to pixels.
pub fn fill_path(pixmap: &mut Pixmap, path: &GlyphPath, t: Transform, ink: Ink) {
    let Some(p) = sk_path(path, Transform::identity()) else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color(ink.color());
    paint.anti_alias = true;
    pixmap.fill_path(&p, &paint, FillRule::Winding, t, None);
}

/// Strokes `path`'s outline (for the old glyph under a changed one).
pub fn stroke_path(pixmap: &mut Pixmap, path: &GlyphPath, t: Transform, ink: Ink, width_px: f32) {
    let Some(p) = sk_path(path, t) else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color(ink.color());
    paint.anti_alias = true;
    let stroke = tiny_skia::Stroke {
        width: width_px,
        ..Default::default()
    };
    pixmap.stroke_path(&p, &paint, &stroke, Transform::identity(), None);
}

/// Normalised coordinates for user-space axis values (`wght` = 700…);
/// axes not named keep their default.
pub fn location(font: &FontRef, variations: &[(Tag, f32)]) -> Vec<NormalizedCoord> {
    font.axes()
        .location(variations.iter().copied())
        .coords()
        .to_vec()
}

/// The font's vertical extent in font units: ascender (positive) and
/// descender (negative), from `hhea` / typo metrics.
fn extent(font: &FontRef, coords: &[NormalizedCoord]) -> (f32, f32, f32) {
    let m = font.metrics(Size::unscaled(), LocationRef::new(coords));
    let (mut asc, mut desc) = (m.ascent, m.descent);
    if asc - desc <= 0.0 {
        asc = f32::from(m.units_per_em) * 0.8;
        desc = -f32::from(m.units_per_em) * 0.2;
    }
    (asc, desc, f32::from(m.units_per_em.max(1)))
}

/// How a glyph tile is laid out: the em size, and where the baseline sits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tile {
    /// Cell size in pixels.
    pub width: u32,
    pub height: u32,
    /// Pixels per em.
    pub em: f32,
    /// Baseline from the top, in pixels.
    pub baseline: f32,
    /// Font units → pixels.
    pub scale: f32,
}

/// A cell `width`×`height` pixels for glyphs of `font`: the em fitted so
/// ascender to descender fills `fill` of the height, baseline shared by
/// every glyph.
pub fn tile(
    font: &FontRef,
    coords: &[NormalizedCoord],
    width: u32,
    height: u32,
    fill: f32,
) -> Tile {
    let (asc, desc, upm) = extent(font, coords);
    let scale = (height as f32 * fill) / (asc - desc);
    let pad = (height as f32 - (asc - desc) * scale) / 2.0;
    Tile {
        width,
        height,
        em: upm * scale,
        baseline: pad + asc * scale,
        scale,
    }
}

/// Glyph `gid` drawn in `tile`, centred on its advance width.
pub fn render_glyph(
    font: &FontRef,
    gid: u32,
    coords: &[NormalizedCoord],
    tile: Tile,
    ink: Ink,
) -> Raster {
    let mut pm = pixmap(tile.width as f32, tile.height as f32);
    let id = GlyphId::new(gid);
    let advance = font
        .glyph_metrics(Size::unscaled(), LocationRef::new(coords))
        .advance_width(id)
        .unwrap_or(0.0);
    let x = (tile.width as f32 - advance * tile.scale) / 2.0;
    draw_glyph(
        &mut pm,
        font,
        gid,
        coords,
        x,
        tile.baseline,
        tile.scale,
        ink,
    );
    Raster::from_pixmap(pm)
}

/// Glyphs drawn one after another in `tile` (each `(gid, x)` with `x` its
/// pen position in font units), the run centred: a substitution's input
/// or output, a kerning pair.
pub fn render_run(
    font: &FontRef,
    run: &[(u32, f32)],
    coords: &[NormalizedCoord],
    tile: Tile,
    ink: Ink,
) -> Raster {
    let mut pm = pixmap(tile.width as f32, tile.height as f32);
    let metrics = font.glyph_metrics(Size::unscaled(), LocationRef::new(coords));
    let end = run
        .iter()
        .map(|&(gid, x)| x + metrics.advance_width(GlyphId::new(gid)).unwrap_or(0.0))
        .fold(0.0f32, f32::max);
    let left = (tile.width as f32 - end * tile.scale) / 2.0;
    for &(gid, x) in run {
        draw_glyph(
            &mut pm,
            font,
            gid,
            coords,
            left + x * tile.scale,
            tile.baseline,
            tile.scale,
            ink,
        );
    }
    Raster::from_pixmap(pm)
}

/// The advance width of `gid`, font units.
pub fn advance(font: &FontRef, gid: u32, coords: &[NormalizedCoord]) -> f32 {
    font.glyph_metrics(Size::unscaled(), LocationRef::new(coords))
        .advance_width(GlyphId::new(gid))
        .unwrap_or(0.0)
}

/// Two rasters compared pixel by pixel, the way type designers overlay
/// proofs: ink only the old one has in `removed`, ink only the new one has
/// in `added`, ink both have in `same`. Coverage counts as ink above half.
/// The rasters line up at their top left corners.
pub fn highlight(old: &Raster, new: &Raster, removed: Ink, added: Ink, same: Ink) -> Raster {
    let width = old.width.max(new.width);
    let height = old.height.max(new.height);
    let alpha = |r: &Raster, x: u32, y: u32| -> u8 {
        if x < r.width && y < r.height {
            r.data[((y * r.width + x) * 4 + 3) as usize]
        } else {
            0
        }
    };
    let mut data = vec![0u8; (width * height * 4) as usize];
    for y in 0..height {
        for x in 0..width {
            let (a, b) = (alpha(old, x, y), alpha(new, x, y));
            let (ink, cover) = match (a > 127, b > 127) {
                (true, true) => (same, a.max(b)),
                (true, false) => (removed, a),
                (false, true) => (added, b),
                // antialiased edges keep their coverage in the colour of the
                // side that has more of it
                (false, false) if a > b => (removed, a),
                (false, false) => (added, b),
            };
            let cover = u32::from(cover) * u32::from(ink.a) / 255;
            let i = ((y * width + x) * 4) as usize;
            let pre = |c: u8| (u32::from(c) * cover / 255) as u8;
            data[i] = pre(ink.r);
            data[i + 1] = pre(ink.g);
            data[i + 2] = pre(ink.b);
            data[i + 3] = cover as u8;
        }
    }
    Raster {
        width,
        height,
        data,
    }
}

/// An outline (from a font source) drawn in a cell the way
/// [`render_glyph`] draws compiled glyphs.
#[allow(clippy::too_many_arguments)]
pub fn render_outline(
    path: &GlyphPath,
    advance: f32,
    ascender: f32,
    descender: f32,
    width: u32,
    height: u32,
    fill: f32,
    ink: Ink,
) -> Raster {
    let span = (ascender - descender).max(1.0);
    let scale = (height as f32 * fill) / span;
    let baseline = (height as f32 - span * scale) / 2.0 + ascender * scale;
    let x = (width as f32 - advance * scale) / 2.0;
    let mut pm = pixmap(width as f32, height as f32);
    fill_path(
        &mut pm,
        path,
        Transform::from_row(scale, 0.0, 0.0, -scale, x, baseline),
        ink,
    );
    Raster::from_pixmap(pm)
}

/// Draws glyph `gid` with its origin at (`x`, `baseline`) pixels:
/// colour layers, a bitmap strike or the outline, whichever the font has.
#[allow(clippy::too_many_arguments)]
fn draw_glyph(
    pm: &mut Pixmap,
    font: &FontRef,
    gid: u32,
    coords: &[NormalizedCoord],
    x: f32,
    baseline: f32,
    scale: f32,
    ink: Ink,
) {
    let id = GlyphId::new(gid);
    let t = Transform::from_row(scale, 0.0, 0.0, -scale, x, baseline);
    if let Some(color) = font.color_glyphs().get(id) {
        let mut painter = Painter::new(pm.width(), pm.height(), font, coords, t, ink);
        if color.paint(LocationRef::new(coords), &mut painter).is_ok() {
            painter.finish(pm);
            return;
        }
    }
    if draw_bitmap(pm, font, id, x, baseline, scale) {
        return;
    }
    let path = crate::report::draw(font, gid, coords);
    fill_path(pm, &path, t, ink);
}

/// Draws an `sbix` / `CBDT` PNG strike of `id`; false when there is none.
fn draw_bitmap(
    pm: &mut Pixmap,
    font: &FontRef,
    id: GlyphId,
    x: f32,
    baseline: f32,
    scale: f32,
) -> bool {
    let strikes = font.bitmap_strikes();
    if strikes.is_empty() {
        return false;
    }
    let upm = f32::from(
        font.metrics(Size::unscaled(), LocationRef::default())
            .units_per_em,
    );
    let ppem = upm * scale;
    let Some(glyph) = strikes.glyph_for_size(Size::new(ppem), id) else {
        return false;
    };
    let skrifa::bitmap::BitmapData::Png(png) = glyph.data else {
        return false;
    };
    let Ok(img) = Pixmap::decode_png(png) else {
        return false;
    };
    let s = ppem / glyph.ppem_x.max(1.0);
    let ox = x + glyph.bearing_x * s;
    let oy = baseline - glyph.bearing_y * s;
    pm.draw_pixmap(
        0,
        0,
        img.as_ref(),
        &PixmapPaint {
            quality: tiny_skia::FilterQuality::Bicubic,
            ..Default::default()
        },
        Transform::from_row(s, 0.0, 0.0, s, ox, oy),
        None,
    );
    true
}

/// `COLR` painting onto tiny-skia: a stack of transforms, clip masks and
/// layers.
struct Painter<'f> {
    font: &'f FontRef<'f>,
    coords: &'f [NormalizedCoord],
    width: u32,
    height: u32,
    ink: Ink,
    palette: Vec<Color>,
    transforms: Vec<Transform>,
    clips: Vec<Option<Mask>>,
    layers: Vec<Pixmap>,
}

impl<'f> Painter<'f> {
    fn new(
        width: u32,
        height: u32,
        font: &'f FontRef<'f>,
        coords: &'f [NormalizedCoord],
        base: Transform,
        ink: Ink,
    ) -> Self {
        let palette = font
            .color_palettes()
            .get(0)
            .map(|p| {
                p.colors()
                    .iter()
                    .map(|c| Color::from_rgba8(c.red, c.green, c.blue, c.alpha))
                    .collect()
            })
            .unwrap_or_default();
        Self {
            font,
            coords,
            width,
            height,
            ink,
            palette,
            transforms: vec![base],
            clips: vec![None],
            layers: vec![pixmap(width as f32, height as f32)],
        }
    }

    fn transform(&self) -> Transform {
        self.transforms.last().copied().unwrap_or_default()
    }

    fn color(&self, index: u16, alpha: f32) -> Color {
        let mut c = if index == 0xFFFF {
            self.ink.color()
        } else {
            self.palette
                .get(index as usize)
                .copied()
                .unwrap_or(Color::BLACK)
        };
        c.apply_opacity(alpha);
        c
    }

    fn stops(&self, stops: &[ColorStop]) -> Vec<GradientStop> {
        stops
            .iter()
            .map(|s| GradientStop::new(s.offset, self.color(s.palette_index, s.alpha)))
            .collect()
    }

    fn push_mask(&mut self, path: Option<Path>) {
        let mut mask = match self.clips.last() {
            Some(Some(m)) => m.clone(),
            _ => {
                let Some(mut m) = Mask::new(self.width, self.height) else {
                    self.clips.push(None);
                    return;
                };
                m.data_mut().fill(255);
                m
            }
        };
        match path {
            Some(p) => mask.intersect_path(&p, FillRule::Winding, true, Transform::identity()),
            None => mask.data_mut().fill(0),
        }
        self.clips.push(Some(mask));
    }

    fn finish(mut self, target: &mut Pixmap) {
        if let Some(layer) = self.layers.pop() {
            target.draw_pixmap(
                0,
                0,
                layer.as_ref(),
                &PixmapPaint::default(),
                Transform::identity(),
                None,
            );
        }
    }
}

fn to_sk(t: ColorTransform) -> Transform {
    Transform::from_row(t.xx, t.yx, t.xy, t.yy, t.dx, t.dy)
}

fn spread(e: Extend) -> SpreadMode {
    match e {
        Extend::Repeat => SpreadMode::Repeat,
        Extend::Reflect => SpreadMode::Reflect,
        _ => SpreadMode::Pad,
    }
}

fn blend(mode: CompositeMode) -> BlendMode {
    match mode {
        CompositeMode::Clear => BlendMode::Clear,
        CompositeMode::Src => BlendMode::Source,
        CompositeMode::Dest => BlendMode::Destination,
        CompositeMode::DestOver => BlendMode::DestinationOver,
        CompositeMode::SrcIn => BlendMode::SourceIn,
        CompositeMode::DestIn => BlendMode::DestinationIn,
        CompositeMode::SrcOut => BlendMode::SourceOut,
        CompositeMode::DestOut => BlendMode::DestinationOut,
        CompositeMode::SrcAtop => BlendMode::SourceAtop,
        CompositeMode::DestAtop => BlendMode::DestinationAtop,
        CompositeMode::Xor => BlendMode::Xor,
        CompositeMode::Plus => BlendMode::Plus,
        CompositeMode::Screen => BlendMode::Screen,
        CompositeMode::Overlay => BlendMode::Overlay,
        CompositeMode::Darken => BlendMode::Darken,
        CompositeMode::Lighten => BlendMode::Lighten,
        CompositeMode::ColorDodge => BlendMode::ColorDodge,
        CompositeMode::ColorBurn => BlendMode::ColorBurn,
        CompositeMode::HardLight => BlendMode::HardLight,
        CompositeMode::SoftLight => BlendMode::SoftLight,
        CompositeMode::Difference => BlendMode::Difference,
        CompositeMode::Exclusion => BlendMode::Exclusion,
        CompositeMode::Multiply => BlendMode::Multiply,
        CompositeMode::HslHue => BlendMode::Hue,
        CompositeMode::HslSaturation => BlendMode::Saturation,
        CompositeMode::HslColor => BlendMode::Color,
        CompositeMode::HslLuminosity => BlendMode::Luminosity,
        _ => BlendMode::SourceOver,
    }
}

impl ColorPainter for Painter<'_> {
    fn push_transform(&mut self, t: ColorTransform) {
        let next = self.transform().pre_concat(to_sk(t));
        self.transforms.push(next);
    }

    fn pop_transform(&mut self) {
        if self.transforms.len() > 1 {
            self.transforms.pop();
        }
    }

    fn push_clip_glyph(&mut self, glyph_id: GlyphId) {
        let outline = crate::report::draw(self.font, glyph_id.to_u32(), self.coords);
        let path = sk_path(&outline, self.transform());
        self.push_mask(path);
    }

    fn push_clip_box(&mut self, b: BoundingBox) {
        let path = Rect::from_ltrb(b.x_min, b.y_min, b.x_max, b.y_max)
            .map(PathBuilder::from_rect)
            .and_then(|p| p.transform(self.transform()));
        self.push_mask(path);
    }

    fn pop_clip(&mut self) {
        if self.clips.len() > 1 {
            self.clips.pop();
        }
    }

    fn fill(&mut self, brush: Brush<'_>) {
        let t = self.transform();
        let shader = match brush {
            Brush::Solid {
                palette_index,
                alpha,
            } => Shader::SolidColor(self.color(palette_index, alpha)),
            Brush::LinearGradient {
                p0,
                p1,
                color_stops,
                extend,
            } => LinearGradient::new(
                Point::from_xy(p0.x, p0.y),
                Point::from_xy(p1.x, p1.y),
                self.stops(color_stops),
                spread(extend),
                t,
            )
            .unwrap_or(Shader::SolidColor(
                color_stops
                    .first()
                    .map(|s| self.color(s.palette_index, s.alpha))
                    .unwrap_or(Color::TRANSPARENT),
            )),
            Brush::RadialGradient {
                c0,
                c1,
                r1,
                color_stops,
                extend,
                ..
            } => RadialGradient::new(
                Point::from_xy(c0.x, c0.y),
                Point::from_xy(c1.x, c1.y),
                r1,
                self.stops(color_stops),
                spread(extend),
                t,
            )
            .unwrap_or(Shader::SolidColor(
                color_stops
                    .last()
                    .map(|s| self.color(s.palette_index, s.alpha))
                    .unwrap_or(Color::TRANSPARENT),
            )),
            // tiny-skia has no sweep gradient: its middle colour
            Brush::SweepGradient { color_stops, .. } => Shader::SolidColor(
                color_stops
                    .get(color_stops.len() / 2)
                    .map(|s| self.color(s.palette_index, s.alpha))
                    .unwrap_or(Color::TRANSPARENT),
            ),
        };
        let paint = Paint {
            shader,
            anti_alias: true,
            ..Default::default()
        };
        let Some(rect) = Rect::from_xywh(0.0, 0.0, self.width as f32, self.height as f32) else {
            return;
        };
        let mask = self.clips.last().and_then(|m| m.as_ref());
        if let Some(layer) = self.layers.last_mut() {
            layer.fill_rect(rect, &paint, Transform::identity(), mask);
        }
    }

    fn push_layer(&mut self, _mode: CompositeMode) {
        self.layers
            .push(pixmap(self.width as f32, self.height as f32));
    }

    fn pop_layer_with_mode(&mut self, mode: CompositeMode) {
        if self.layers.len() < 2 {
            return;
        }
        if let (Some(top), Some(below)) = (self.layers.pop(), self.layers.last_mut()) {
            below.draw_pixmap(
                0,
                0,
                top.as_ref(),
                &PixmapPaint {
                    blend_mode: blend(mode),
                    ..Default::default()
                },
                Transform::identity(),
                None,
            );
        }
    }
}

/// A glyph placed by the shaper, in font units relative to the line start.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shaped {
    pub gid: u32,
    /// Byte offset of the cluster in the text.
    pub cluster: u32,
    pub x_advance: f32,
    pub y_advance: f32,
    pub x_offset: f32,
    pub y_offset: f32,
}

/// Shapes `text` (one line) with `features` (tag, value: 0 off, 1 on,
/// n alternate) at the user-space `variations`.
pub fn shape(
    font: &FontRef,
    text: &str,
    features: &[(Tag, u32)],
    variations: &[(Tag, f32)],
) -> Vec<Shaped> {
    let data = ShaperData::new(font);
    let instance = ShaperInstance::from_variations(
        font,
        variations
            .iter()
            .map(|&(tag, value)| Variation { tag, value }),
    );
    let shaper = data
        .shaper(font)
        .instance(Some(&instance))
        .point_size(None)
        .build();
    let mut buffer = UnicodeBuffer::new();
    buffer.push_str(text);
    buffer.guess_segment_properties();
    let features: Vec<Feature> = features
        .iter()
        .map(|&(tag, value)| Feature::new(tag, value, ..))
        .collect();
    let out = shaper.shape(buffer, &features);
    out.glyph_infos()
        .iter()
        .zip(out.glyph_positions())
        .map(|(i, p)| Shaped {
            gid: i.glyph_id,
            cluster: i.cluster,
            x_advance: p.x_advance as f32,
            y_advance: p.y_advance as f32,
            x_offset: p.x_offset as f32,
            y_offset: p.y_offset as f32,
        })
        .collect()
}

/// Lines of text to set, each at its own size.
#[derive(Clone, Debug, PartialEq)]
pub struct TextBlock {
    pub text: String,
    /// Pixels per em.
    pub size: f32,
}

/// How [`render_text`] sets its blocks.
#[derive(Clone, Debug, PartialEq)]
pub struct TextOptions {
    pub features: Vec<(Tag, u32)>,
    pub variations: Vec<(Tag, f32)>,
    /// Wrap width in pixels (the raster is this wide).
    pub width: f32,
    /// Line height as a multiple of the font's ascender-to-descender span.
    pub line_height: f32,
    /// Space between blocks, pixels.
    pub block_gap: f32,
    /// Margin around everything, pixels.
    pub padding: f32,
    pub ink: Ink,
}

/// A glyph's box in the raster (pixels), for outlines and hit testing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlacedGlyph {
    pub gid: u32,
    pub cluster: u32,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Text set by [`render_text`].
#[derive(Clone, Debug, PartialEq)]
pub struct TextLayout {
    pub raster: Raster,
    pub glyphs: Vec<PlacedGlyph>,
}

/// Breaks shaped glyphs into lines no wider than `max` font units, at
/// spaces where possible.
fn wrap(text: &str, glyphs: &[Shaped], max: f32) -> Vec<std::ops::Range<usize>> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut width = 0.0;
    let mut last_break: Option<usize> = None;
    let mut i = 0;
    while i < glyphs.len() {
        let g = &glyphs[i];
        let is_space = text
            .get(g.cluster as usize..)
            .and_then(|s| s.chars().next())
            .is_some_and(char::is_whitespace);
        if !is_space && i > start && width + g.x_advance > max {
            // break at the last space, dropping it, or before this glyph
            let (end, next) = match last_break {
                Some(b) if b > start => (b, b + 1),
                _ => (i, i),
            };
            lines.push(start..end);
            start = next;
            width = glyphs[start..i].iter().map(|g| g.x_advance).sum();
            last_break = None;
            continue;
        }
        if is_space {
            last_break = Some(i);
        }
        width += g.x_advance;
        i += 1;
    }
    if start < glyphs.len() || lines.is_empty() {
        lines.push(start..glyphs.len());
    }
    lines
}

/// Sets `blocks` in `font`, wrapped to `opts.width`.
pub fn render_text(font: &FontRef, blocks: &[TextBlock], opts: &TextOptions) -> TextLayout {
    let coords = location(font, &opts.variations);
    let (asc, desc, upm) = extent(font, &coords);
    let inner = (opts.width - 2.0 * opts.padding).max(1.0);
    // lay out first, to know the height
    struct Line {
        glyphs: Vec<Shaped>,
        scale: f32,
        baseline: f32,
    }
    let mut lines: Vec<Line> = Vec::new();
    let mut y = opts.padding;
    for (bi, block) in blocks.iter().enumerate() {
        if bi > 0 {
            y += opts.block_gap;
        }
        let scale = block.size / upm;
        let line_h = (asc - desc) * scale * opts.line_height;
        for para in block.text.split('\n') {
            let shaped = shape(font, para, &opts.features, &opts.variations);
            for range in wrap(para, &shaped, inner / scale) {
                let lead = (line_h - (asc - desc) * scale) / 2.0;
                lines.push(Line {
                    glyphs: shaped[range].to_vec(),
                    scale,
                    baseline: y + lead + asc * scale,
                });
                y += line_h;
                if y > MAX_SIDE as f32 {
                    break;
                }
            }
        }
    }
    let height = y + opts.padding;
    let mut pm = pixmap(opts.width, height);
    let mut placed = Vec::new();
    let metrics = font.glyph_metrics(Size::unscaled(), LocationRef::new(&coords));
    for line in &lines {
        let mut x = opts.padding;
        for g in &line.glyphs {
            let gx = x + g.x_offset * line.scale;
            let gy = line.baseline - g.y_offset * line.scale;
            draw_glyph(&mut pm, font, g.gid, &coords, gx, gy, line.scale, opts.ink);
            let advance = metrics
                .advance_width(GlyphId::new(g.gid))
                .unwrap_or(g.x_advance);
            placed.push(PlacedGlyph {
                gid: g.gid,
                cluster: g.cluster,
                x: gx,
                y: gy - asc * line.scale,
                width: advance * line.scale,
                height: (asc - desc) * line.scale,
            });
            x += g.x_advance * line.scale;
        }
    }
    TextLayout {
        raster: Raster::from_pixmap(pm),
        glyphs: placed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shaped(advances: &[f32], text: &str) -> Vec<Shaped> {
        let mut cluster = 0;
        advances
            .iter()
            .zip(text.chars())
            .map(|(&a, c)| {
                let s = Shaped {
                    gid: 1,
                    cluster,
                    x_advance: a,
                    y_advance: 0.0,
                    x_offset: 0.0,
                    y_offset: 0.0,
                };
                cluster += c.len_utf8() as u32;
                s
            })
            .collect()
    }

    #[test]
    fn wraps_at_spaces() {
        let text = "aa bb cc";
        let g = shaped(&[1.0; 8], text);
        assert_eq!(wrap(text, &g, 5.0), vec![0..5, 6..8]);
        assert_eq!(wrap(text, &g, 100.0), vec![0..8]);
    }

    #[test]
    fn wraps_long_words_anywhere() {
        let text = "abcdefgh";
        let g = shaped(&[1.0; 8], text);
        assert_eq!(wrap(text, &g, 3.0), vec![0..3, 3..6, 6..8]);
    }

    #[test]
    fn highlight_colours_each_side() {
        let raster = |a: u8| Raster {
            width: 1,
            height: 1,
            data: vec![0, 0, 0, a],
        };
        let (r, g, k) = (
            Ink::rgba(255, 0, 0, 255),
            Ink::rgba(0, 255, 0, 255),
            Ink::rgba(0, 0, 0, 255),
        );
        assert_eq!(
            highlight(&raster(255), &raster(0), r, g, k).data,
            [255, 0, 0, 255]
        );
        assert_eq!(
            highlight(&raster(0), &raster(255), r, g, k).data,
            [0, 255, 0, 255]
        );
        assert_eq!(
            highlight(&raster(255), &raster(255), r, g, k).data,
            [0, 0, 0, 255]
        );
        assert_eq!(
            highlight(&raster(0), &raster(0), r, g, k).data,
            [0, 0, 0, 0]
        );
    }

    #[test]
    fn fills_outlines() {
        let mut path = GlyphPath::default();
        use skrifa::outline::OutlinePen;
        path.move_to(0.0, 0.0);
        path.line_to(100.0, 0.0);
        path.line_to(100.0, 100.0);
        path.line_to(0.0, 100.0);
        path.close();
        let r = render_outline(
            &path,
            100.0,
            100.0,
            0.0,
            20,
            20,
            1.0,
            Ink::rgba(0, 0, 0, 255),
        );
        assert!(!r.is_blank());
        assert_eq!((r.width, r.height), (20, 20));
        // the centre is covered
        assert_eq!(r.data[(10 * 20 + 10) * 4 + 3], 255);
    }
}
