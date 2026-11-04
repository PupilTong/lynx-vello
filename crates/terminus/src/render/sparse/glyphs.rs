//! Glyph runs drawn through glifo with vello's flattening.
//!
//! glifo resolves a run to one outline per glyph and hands each to its
//! renderer under that glyph's transform. [`Glyphs`] is that renderer over a
//! `vello_gpu::Scene`: it encodes the outline the way vello encodes a glyph
//! (`PathEncoder`, so closing lines and stroke cap markers are vello's),
//! flattens it with [`super::flatten`] and fills the device-space lines.
//! Everything else glifo asks of it passes through to the scene.

use std::ops::RangeInclusive;

use vello::kurbo::{Affine, BezPath, Rect, Stroke};
use vello::low_level::vello_encoding::{Encoding, Transform};
use vello::peniko::color::{AlphaColor, Srgb};
use vello::peniko::{BlendMode, Fill};
use vello_gpu::{Image, ImageSource, PaintType, RenderState, Scene, Tint};

/// The glyph renderer: a scene, the transforms glifo last set on it, and a
/// scratch encoding for one outline.
pub(super) struct Glyphs<'a> {
    pub(super) scene: &'a mut Scene,
    transform: Affine,
    paint_transform: Affine,
    encoding: Encoding,
}

impl<'a> Glyphs<'a> {
    pub(super) fn new(scene: &'a mut Scene) -> Self {
        Self {
            scene,
            transform: Affine::IDENTITY,
            paint_transform: Affine::IDENTITY,
            encoding: Encoding::new(),
        }
    }

    /// Fills `shape` as vello fills (or with `stroke`, strokes) a glyph
    /// outline under the current transform.
    fn draw(&mut self, shape: &impl vello::kurbo::Shape, stroke: Option<&Stroke>) {
        let encoding = &mut self.encoding;
        encoding.reset();
        encoding.encode_transform(Transform::from_kurbo(&self.transform));
        match stroke {
            Some(stroke) => {
                if !encoding.encode_stroke_style(stroke) {
                    return;
                }
            }
            None => encoding.encode_fill_style(Fill::NonZero),
        }
        if !encoding.encode_shape(shape, stroke.is_none()) {
            return;
        }
        let Some(decoded) = super::decode_paths(encoding).into_iter().next() else {
            return;
        };
        self.scene.set_transform(Affine::IDENTITY);
        self.scene
            .set_paint_transform(self.transform * self.paint_transform);
        self.scene.set_fill_rule(Fill::NonZero);
        self.scene.fill_path(&decoded.path);
        self.scene.set_transform(self.transform);
        self.scene.set_paint_transform(self.paint_transform);
    }
}

impl glifo::DrawSink for Glyphs<'_> {
    fn set_transform(&mut self, t: Affine) {
        self.transform = t;
        self.scene.set_transform(t);
    }

    fn set_paint(&mut self, paint: glifo::AtlasPaint) {
        self.scene.set_paint(paint);
    }

    fn set_paint_transform(&mut self, t: Affine) {
        self.paint_transform = t;
        self.scene.set_paint_transform(t);
    }

    fn fill_path(&mut self, path: &BezPath) {
        self.draw(path, None);
    }

    fn fill_rect(&mut self, rect: &Rect) {
        self.draw(rect, None);
    }

    fn push_clip_layer(&mut self, clip: &BezPath) {
        self.scene.push_clip_layer(clip);
    }

    fn push_clip_path(&mut self, clip: &BezPath) {
        self.scene.push_clip_path(clip);
    }

    fn push_clip_rect(&mut self, clip: &Rect) {
        self.scene.push_clip_rect(clip);
    }

    fn push_blend_layer(&mut self, blend_mode: BlendMode) {
        self.scene.push_blend_layer(blend_mode);
    }

    fn pop_layer(&mut self) {
        self.scene.pop_layer();
    }

    fn pop_clip_path(&mut self) {
        self.scene.pop_clip();
    }

    fn width(&self) -> u16 {
        self.scene.width()
    }

    fn height(&self) -> u16 {
        self.scene.height()
    }
}

impl glifo::GlyphRenderer for Glyphs<'_> {
    type SavedState = RenderState;

    fn save_state(&mut self) -> RenderState {
        self.scene.save_current_state()
    }

    fn restore_state(&mut self, state: RenderState) {
        self.scene.restore_state(state);
    }

    fn stroke_path(&mut self, path: &BezPath) {
        let stroke = self.scene.stroke().clone();
        self.draw(path, Some(&stroke));
    }

    fn set_paint_image(&mut self, image: Image) {
        self.scene.set_paint(image);
    }

    fn set_tint(&mut self, tint: Option<Tint>) {
        self.scene.set_tint(tint);
    }

    fn get_context_color(&self) -> AlphaColor<Srgb> {
        match self.scene.paint() {
            PaintType::Solid(color) => *color,
            _ => vello::peniko::color::palette::css::BLACK,
        }
    }

    fn current_paint(&self) -> &PaintType {
        self.scene.paint()
    }

    fn atlas_image_source(&self, atlas_slot: &glifo::AtlasSlot) -> ImageSource {
        ImageSource::opaque_id(atlas_slot.image_id)
    }

    fn atlas_paint_transform(&self, _atlas_slot: &glifo::AtlasSlot) -> Affine {
        let padding = f64::from(glifo::GLYPH_PADDING);
        Affine::translate((-padding, -padding))
    }
}

/// glifo's backend for a run drawn through [`Glyphs`], without the glyph
/// atlas: every glyph is an outline, as in vello.
pub(super) struct Backend<'a> {
    pub(super) glyphs: Glyphs<'a>,
    pub(super) prep: &'a mut glifo::GlyphPrepCache,
}

impl<'a> glifo::GlyphRunBackend<'a> for Backend<'a> {
    fn atlas_cache(self, _enabled: bool) -> Self {
        self
    }

    fn fill_glyphs<G>(
        mut self,
        run: glifo::GlyphRun<'a>,
        glyphs: G,
    ) -> Result<(), glifo::GlyphRenderError>
    where
        G: Iterator<Item = glifo::Glyph> + Clone,
    {
        let mut renderer = run.build(glyphs, self.prep.as_mut(), glifo::AtlasCacher::Disabled);
        renderer.fill_glyphs(&mut self.glyphs)
    }

    fn stroke_glyphs<G>(
        mut self,
        run: glifo::GlyphRun<'a>,
        glyphs: G,
    ) -> Result<(), glifo::GlyphRenderError>
    where
        G: Iterator<Item = glifo::Glyph> + Clone,
    {
        let mut renderer = run.build(glyphs, self.prep.as_mut(), glifo::AtlasCacher::Disabled);
        // The width is in the units glifo caches the outline at.
        let original = self.glyphs.scene.stroke().clone();
        let mut adjusted = original.clone();
        adjusted.width *= renderer.stroke_adjustment();
        self.glyphs.scene.set_stroke(adjusted);
        let result = renderer.stroke_glyphs(&mut self.glyphs);
        self.glyphs.scene.set_stroke(original);
        result
    }

    fn render_decoration<G>(
        mut self,
        run: glifo::GlyphRun<'a>,
        glyphs: G,
        x_range: RangeInclusive<f32>,
        baseline_y: f32,
        offset: f32,
        size: f32,
        buffer: f32,
    ) where
        G: Iterator<Item = glifo::Glyph> + Clone,
    {
        let mut renderer = run.build(glyphs, self.prep.as_mut(), glifo::AtlasCacher::Disabled);
        renderer.render_decoration(x_range, baseline_y, offset, size, buffer, &mut self.glyphs);
    }
}
