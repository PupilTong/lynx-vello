//! Plain-old-data geometry vocabulary shared by every protocol surface.

/// A 2D point (or any per-axis pair — see
/// [`CoreStyle::overflow`](crate::style::CoreStyle::overflow), which uses
/// `Point<Overflow>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(C)]
pub struct Point<T> {
    pub x: T,
    pub y: T,
}

impl<T> Point<T> {
    #[must_use]
    pub const fn new(x: T, y: T) -> Self {
        Self { x, y }
    }

    #[must_use]
    pub fn map<U>(self, mut f: impl FnMut(T) -> U) -> Point<U> {
        Point {
            x: f(self.x),
            y: f(self.y),
        }
    }
}

impl Point<f32> {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };
}

impl Point<Option<f32>> {
    pub const NONE: Self = Self { x: None, y: None };
}

/// A 2D size (or any per-axis pair of constraints — `Size<Option<f32>>`,
/// `Size<AvailableSpace>`, `Size<Dimension>` all appear in the protocol).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(C)]
pub struct Size<T> {
    pub width: T,
    pub height: T,
}

impl<T> Size<T> {
    #[must_use]
    pub const fn new(width: T, height: T) -> Self {
        Self { width, height }
    }

    #[must_use]
    pub fn map<U>(self, mut f: impl FnMut(T) -> U) -> Size<U> {
        Size {
            width: f(self.width),
            height: f(self.height),
        }
    }

    #[must_use]
    pub fn zip_map<U, V>(self, other: Size<U>, mut f: impl FnMut(T, U) -> V) -> Size<V> {
        Size {
            width: f(self.width, other.width),
            height: f(self.height, other.height),
        }
    }

    #[must_use]
    pub const fn as_ref(&self) -> Size<&T> {
        Size {
            width: &self.width,
            height: &self.height,
        }
    }
}

impl Size<f32> {
    pub const ZERO: Self = Self {
        width: 0.0,
        height: 0.0,
    };
}

impl Size<Option<f32>> {
    pub const NONE: Self = Self {
        width: None,
        height: None,
    };

    #[must_use]
    pub fn unwrap_or(self, fallback: Size<f32>) -> Size<f32> {
        Size {
            width: self.width.unwrap_or(fallback.width),
            height: self.height.unwrap_or(fallback.height),
        }
    }

    #[must_use]
    pub fn or(self, fallback: Self) -> Self {
        Size {
            width: self.width.or(fallback.width),
            height: self.height.or(fallback.height),
        }
    }
}

/// Per-edge values for box surrounds: margin, padding, border, inset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(C)]
pub struct Edges<T> {
    pub left: T,
    pub right: T,
    pub top: T,
    pub bottom: T,
}

impl<T> Edges<T> {
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

    #[must_use]
    pub fn map<U>(self, mut f: impl FnMut(T) -> U) -> Edges<U> {
        Edges {
            left: f(self.left),
            right: f(self.right),
            top: f(self.top),
            bottom: f(self.bottom),
        }
    }

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
    pub const ZERO: Self = Self {
        left: 0.0,
        right: 0.0,
        top: 0.0,
        bottom: 0.0,
    };

    #[must_use]
    pub fn horizontal_sum(&self) -> f32 {
        self.left + self.right
    }

    #[must_use]
    pub fn vertical_sum(&self) -> f32 {
        self.top + self.bottom
    }
}

/// A `start`/`end` pair along one axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(C)]
pub struct Line<T> {
    pub start: T,
    pub end: T,
}

impl<T> Line<T> {
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
