//! Plain-old-data geometry vocabulary shared by every protocol surface.
//!
//! All types here are small `Copy` structs with public fields and `#[repr(C)]`
//! layout: they are the currency of the engine⇄host boundary, so their
//! representation is part of the protocol. Keeping them POD (no niches, no
//! interior abstraction) makes them free to pass by value, trivially cacheable
//! in host-side structures-of-arrays, and stable targets for future SIMD work
//! inside the algorithms.
//!
//! Axis conventions: `x`/`width` is the horizontal (inline) axis, `y`/`height`
//! the vertical (block) axis, `y` grows downward. neutron-star is a
//! physical-axis engine — there is no writing-mode abstraction (see the
//! non-goals section of the crate docs); right-to-left `direction` is handled
//! *inside* the algorithms by flipping the main axis, not by logical
//! coordinates in the protocol.

/// A 2D point (or any per-axis pair — see [`CoreStyle::overflow`] which uses
/// `Point<Overflow>`).
///
/// [`CoreStyle::overflow`]: crate::style::CoreStyle::overflow
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(C)]
pub struct Point<T> {
    /// Horizontal component, growing rightward.
    pub x: T,
    /// Vertical component, growing downward.
    pub y: T,
}

impl<T> Point<T> {
    /// Creates a point from its two components.
    #[must_use]
    pub const fn new(x: T, y: T) -> Self {
        Self { x, y }
    }

    /// Applies `f` to both components, producing a new point.
    #[must_use]
    pub fn map<U>(self, mut f: impl FnMut(T) -> U) -> Point<U> {
        Point {
            x: f(self.x),
            y: f(self.y),
        }
    }
}

impl Point<f32> {
    /// The origin: `(0.0, 0.0)`.
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };
}

impl Point<Option<f32>> {
    /// Both components unknown. Used for absent baselines in
    /// [`LayoutOutput::first_baselines`](crate::tree::LayoutOutput::first_baselines).
    pub const NONE: Self = Self { x: None, y: None };
}

/// A 2D size (or any per-axis pair of constraints — `Size<Option<f32>>`,
/// `Size<AvailableSpace>`, `Size<Dimension>` all appear in the protocol).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(C)]
pub struct Size<T> {
    /// Horizontal extent.
    pub width: T,
    /// Vertical extent.
    pub height: T,
}

impl<T> Size<T> {
    /// Creates a size from its two extents.
    #[must_use]
    pub const fn new(width: T, height: T) -> Self {
        Self { width, height }
    }

    /// Applies `f` to both extents, producing a new size.
    #[must_use]
    pub fn map<U>(self, mut f: impl FnMut(T) -> U) -> Size<U> {
        Size {
            width: f(self.width),
            height: f(self.height),
        }
    }

    /// Combines two sizes component-wise with `f`.
    #[must_use]
    pub fn zip_map<U, V>(self, other: Size<U>, mut f: impl FnMut(T, U) -> V) -> Size<V> {
        Size {
            width: f(self.width, other.width),
            height: f(self.height, other.height),
        }
    }

    /// Borrows both extents, producing a per-extent reference pair — the
    /// shape borrowed style accessors return, buildable from any host
    /// storage (the two extents need not be stored contiguously).
    #[must_use]
    pub const fn as_ref(&self) -> Size<&T> {
        Size {
            width: &self.width,
            height: &self.height,
        }
    }
}

impl Size<f32> {
    /// The empty size: `0.0 × 0.0`.
    pub const ZERO: Self = Self {
        width: 0.0,
        height: 0.0,
    };
}

impl Size<Option<f32>> {
    /// Both extents unknown — the usual starting state of
    /// [`LayoutInput::known_dimensions`](crate::tree::LayoutInput::known_dimensions).
    pub const NONE: Self = Self {
        width: None,
        height: None,
    };

    /// Resolves unknown extents from `fallback`.
    #[must_use]
    pub fn unwrap_or(self, fallback: Size<f32>) -> Size<f32> {
        Size {
            width: self.width.unwrap_or(fallback.width),
            height: self.height.unwrap_or(fallback.height),
        }
    }

    /// Keeps known extents, filling unknown ones from `fallback`.
    #[must_use]
    pub fn or(self, fallback: Self) -> Self {
        Size {
            width: self.width.or(fallback.width),
            height: self.height.or(fallback.height),
        }
    }
}

/// Per-edge values for box surrounds: margin, padding, border, inset.
///
/// This is *edge values*, not a rectangle — a `Edges<f32>` padding of
/// `{ left: 4.0, .. }` means "4px of padding on the left edge".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(C)]
pub struct Edges<T> {
    /// Value for the left edge.
    pub left: T,
    /// Value for the right edge.
    pub right: T,
    /// Value for the top edge.
    pub top: T,
    /// Value for the bottom edge.
    pub bottom: T,
}

impl<T> Edges<T> {
    /// The same value on all four edges.
    #[must_use]
    pub fn uniform(value: T) -> Self
    where
        T: Clone,
    {
        Self {
            left: value.clone(),
            right: value.clone(),
            top: value.clone(),
            bottom: value,
        }
    }

    /// Applies `f` to all four edges, producing new edge values.
    #[must_use]
    pub fn map<U>(self, mut f: impl FnMut(T) -> U) -> Edges<U> {
        Edges {
            left: f(self.left),
            right: f(self.right),
            top: f(self.top),
            bottom: f(self.bottom),
        }
    }

    /// Borrows all four edges, producing per-edge references — the shape
    /// borrowed style accessors return, buildable from any host storage
    /// (the four edges need not be stored contiguously).
    #[must_use]
    pub const fn as_ref(&self) -> Edges<&T> {
        Edges {
            left: &self.left,
            right: &self.right,
            top: &self.top,
            bottom: &self.bottom,
        }
    }
}

impl Edges<f32> {
    /// Zero on all four edges.
    pub const ZERO: Self = Self {
        left: 0.0,
        right: 0.0,
        top: 0.0,
        bottom: 0.0,
    };

    /// `left + right` — the total horizontal contribution of these edges.
    #[must_use]
    pub fn horizontal_sum(&self) -> f32 {
        self.left + self.right
    }

    /// `top + bottom` — the total vertical contribution of these edges.
    #[must_use]
    pub fn vertical_sum(&self) -> f32 {
        self.top + self.bottom
    }
}

/// A `start`/`end` pair along one axis.
///
/// Used for grid placements (`Line<GridPlacement>` is the value of
/// `grid-row` / `grid-column`) and, inside future algorithms, for per-axis
/// edge pairs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(C)]
pub struct Line<T> {
    /// The start (top or left, before `direction` flipping) side.
    pub start: T,
    /// The end (bottom or right, before `direction` flipping) side.
    pub end: T,
}

impl<T> Line<T> {
    /// Creates a line from its two sides.
    #[must_use]
    pub const fn new(start: T, end: T) -> Self {
        Self { start, end }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::float_cmp)]

    use super::*;

    #[test]
    fn point_map_transforms_each_component_once() {
        let mut calls = 0;
        let mapped = Point::new(2_i32, 5_i32).map(|value| {
            calls += 1;
            value * value
        });

        assert_eq!(mapped, Point::new(4, 25));
        assert_eq!(calls, 2);
    }

    #[test]
    fn geometry_helpers_preserve_axis_order() {
        let size = Size::new(2, 3).map(|value| value * 10);
        assert_eq!(size, Size::new(20, 30));
        assert_eq!(
            size.zip_map(Size::new(1, 2), |left, right| left + right),
            Size::new(21, 32)
        );

        let edges = Edges::uniform(2.0);
        assert_eq!(edges.horizontal_sum(), 4.0);
        assert_eq!(edges.vertical_sum(), 4.0);
        assert_eq!(Line::new("start", "end").start, "start");
    }
}
