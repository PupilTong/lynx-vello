//! Items into a vello scene.

use super::parse::opens_blend_in;
use super::{Item, LayerClip, VectorDocument};
use crate::vello::Scene;
use crate::vello::peniko::{BlendMode, Compose, Fill, Mix};

/// Encodes `document` into a scene in viewport units. A document with
/// nothing drawable encodes nothing.
pub(crate) fn encode(document: &VectorDocument) -> Scene {
    let mut scene = Scene::new();
    for item in &document.items {
        match item {
            Item::Path {
                shape,
                bounds: _,
                transform,
                fill,
                stroke,
                fill_first,
            } => {
                let fill = |scene: &mut Scene| {
                    if let Some(fill) = fill {
                        scene.fill(
                            fill.rule,
                            *transform,
                            &fill.brush,
                            fill.brush_transform,
                            shape,
                        );
                    }
                };
                let stroke = |scene: &mut Scene| {
                    if let Some(stroke) = stroke {
                        scene.stroke(
                            &stroke.style,
                            *transform,
                            &stroke.brush,
                            stroke.brush_transform,
                            shape,
                        );
                    }
                };
                if *fill_first {
                    fill(&mut scene);
                    stroke(&mut scene);
                } else {
                    stroke(&mut scene);
                    fill(&mut scene);
                }
            }
            Item::PushLayer {
                blend,
                alpha,
                clip,
                transform,
            } => match clip {
                LayerClip::Bounds(bounds) => {
                    scene.push_layer(Fill::NonZero, *blend, *alpha, *transform, bounds);
                }
                LayerClip::Path(shape, rule) => {
                    scene.push_layer(*rule, *blend, *alpha, *transform, shape);
                }
            },
            Item::PushClip {
                shape,
                rule,
                transform,
                isolate,
            } => {
                if *isolate {
                    scene.push_layer(
                        *rule,
                        BlendMode::new(Mix::Normal, Compose::SrcOver),
                        1.0,
                        *transform,
                        shape,
                    );
                } else {
                    scene.push_clip_layer(*rule, *transform, shape);
                }
            }
            Item::Pop => scene.pop_layer(),
        }
    }
    scene
}

/// Whether drawing `document` opens a blend layer with no isolating layer
/// of its own around it, so the layer a draw of the whole image opens must
/// be a full `Normal` layer rather than a clip layer (vello #1198).
pub(crate) fn opens_blend(document: &VectorDocument) -> bool {
    opens_blend_in(&document.items)
}
