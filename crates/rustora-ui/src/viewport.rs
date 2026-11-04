// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of the geometry of `Viewport.cs`. Positions are world pixels.

use rustora_game::settings::WorldViewport;

use crate::draw::Rect;

/// Port of `ScrollDirection`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScrollDirection(pub u8);

impl ScrollDirection {
    pub const NONE: ScrollDirection = ScrollDirection(0);
    pub const UP: ScrollDirection = ScrollDirection(1);
    pub const LEFT: ScrollDirection = ScrollDirection(2);
    pub const DOWN: ScrollDirection = ScrollDirection(4);
    pub const RIGHT: ScrollDirection = ScrollDirection(8);

    pub fn includes(self, s: ScrollDirection) -> bool {
        self.0 & s.0 == s.0
    }

    pub fn with(self, s: ScrollDirection, value: bool) -> ScrollDirection {
        if value {
            ScrollDirection(self.0 | s.0)
        } else {
            ScrollDirection(self.0 & !s.0)
        }
    }
}

/// The defaults of `WorldViewportSizes`, which RA does not override.
const DEFAULT_SCALE: f32 = 1.0;
const MAX_ZOOM_SCALE: f32 = 2.0;
const MAX_ZOOM_WINDOW_HEIGHT: i32 = 240;
const ALLOW_NATIVE_ZOOM: bool = true;

fn size_range(distance: WorldViewport) -> (f32, f32) {
    match distance {
        WorldViewport::Close => (480.0, 600.0),
        WorldViewport::Medium => (600.0, 900.0),
        _ => (900.0, 1300.0),
    }
}

/// Port of `CalculateMinimumZoom`.
fn calculate_minimum_zoom(height: f32, min_height: f32, max_height: f32) -> f32 {
    if height <= max_height {
        return 1.0;
    }
    let mut step = 1.0;
    loop {
        let mut test_zoom = 1.0;
        loop {
            let next_zoom = test_zoom + step;
            if height < min_height * next_zoom {
                break;
            }
            test_zoom = next_zoom;
        }
        if height < max_height * test_zoom {
            return test_zoom;
        }
        step /= 2.0;
    }
}

/// Port of `Viewport`.
#[derive(Clone, Debug)]
pub struct Viewport {
    pub center: (i32, i32),
    map_bounds: Rect,
    resolution: (i32, i32),
    zoom: f32,
    pub min_zoom: f32,
    pub max_zoom: f32,
    size: (i32, i32),
}

impl Viewport {
    pub fn new(map_bounds: Rect, resolution: (i32, i32), distance: WorldViewport) -> Viewport {
        let mut viewport = Viewport {
            center: (map_bounds.x + map_bounds.w / 2, map_bounds.y + map_bounds.h / 2),
            map_bounds,
            resolution,
            zoom: 1.0,
            min_zoom: 1.0,
            max_zoom: 2.0,
            size: resolution,
        };
        viewport.update_zooms(distance);
        viewport
    }

    /// Port of `UpdateViewportZooms` with `resetCurrentZoom`.
    fn update_zooms(&mut self, distance: WorldViewport) {
        let height = self.resolution.1 as f32;
        self.min_zoom = if ALLOW_NATIVE_ZOOM && distance == WorldViewport::Native {
            DEFAULT_SCALE
        } else {
            let (min, max) = size_range(distance);
            calculate_minimum_zoom(height, min, max) * DEFAULT_SCALE
        };
        self.max_zoom = (self.min_zoom * MAX_ZOOM_SCALE).min(height * DEFAULT_SCALE / MAX_ZOOM_WINDOW_HEIGHT as f32);
        self.set_zoom(self.min_zoom);
    }

    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    fn set_zoom(&mut self, zoom: f32) {
        self.zoom = zoom;
        self.size = ((self.resolution.0 as f32 / zoom) as i32, (self.resolution.1 as f32 / zoom) as i32);
    }

    /// `TopLeft`.
    pub fn top_left(&self) -> (i32, i32) {
        (self.center.0 - self.size.0 / 2, self.center.1 - self.size.1 / 2)
    }

    pub fn bottom_right(&self) -> (i32, i32) {
        (self.center.0 + self.size.0 / 2, self.center.1 + self.size.1 / 2)
    }

    /// `AdjustZoom` without a center.
    pub fn adjust_zoom(&mut self, dz: f32) {
        self.set_zoom((self.zoom * dz.exp()).clamp(self.min_zoom, self.max_zoom));
    }

    /// `AdjustZoom` that keeps the world position under `view` in place.
    pub fn adjust_zoom_at(&mut self, dz: f32, view: (i32, i32)) {
        let old = self.view_to_world_px(view);
        self.adjust_zoom(dz);
        let new = self.view_to_world_px(view);
        self.center = (self.center.0 + old.0 - new.0, self.center.1 + old.1 - new.1);
    }

    /// `GetBlockedDirections`.
    pub fn blocked_directions(&self) -> ScrollDirection {
        let b = self.map_bounds;
        ScrollDirection::NONE
            .with(ScrollDirection::UP, self.center.1 <= b.y)
            .with(ScrollDirection::LEFT, self.center.0 <= b.x)
            .with(ScrollDirection::DOWN, self.center.1 >= b.bottom())
            .with(ScrollDirection::RIGHT, self.center.0 >= b.right())
    }

    pub fn map_bounds(&self) -> Rect {
        self.map_bounds
    }

    /// `ViewToWorldPx`.
    pub fn view_to_world_px(&self, view: (i32, i32)) -> (i32, i32) {
        let tl = self.top_left();
        ((view.0 as f32 / self.zoom) as i32 + tl.0, (view.1 as f32 / self.zoom) as i32 + tl.1)
    }

    /// `WorldToViewPx`.
    pub fn world_to_view_px(&self, world: (i32, i32)) -> (i32, i32) {
        let tl = self.top_left();
        (
            (self.zoom * (world.0 - tl.0) as f32) as i32,
            (self.zoom * (world.1 - tl.1) as f32) as i32,
        )
    }

    /// `Center` of a screen position.
    pub fn center_on(&mut self, world: (i32, i32)) {
        let b = self.map_bounds;
        self.center = (world.0.clamp(b.x, b.right()), world.1.clamp(b.y, b.bottom()));
    }

    /// `Scroll`, staying inside the map.
    pub fn scroll(&mut self, delta: (f32, f32)) {
        let moved = (
            self.center.0 + (delta.0 / self.zoom) as i32,
            self.center.1 + (delta.1 / self.zoom) as i32,
        );
        self.center_on(moved);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn viewport() -> Viewport {
        Viewport::new(Rect::new(0, 0, 2400, 2400), (1280, 800), WorldViewport::Medium)
    }

    #[test]
    fn minimum_zoom_uses_clean_fractions() {
        assert_eq!(calculate_minimum_zoom(800.0, 600.0, 900.0), 1.0);
        assert_eq!(calculate_minimum_zoom(1080.0, 600.0, 900.0), 1.5);
        assert_eq!(calculate_minimum_zoom(2160.0, 600.0, 900.0), 3.0);
    }

    #[test]
    fn scroll_stops_at_the_map_edge() {
        let mut v = viewport();
        v.scroll((-5000.0, 10.0));
        assert_eq!(v.center, (0, 1210));
        assert!(v.blocked_directions().includes(ScrollDirection::LEFT));
        assert!(!v.blocked_directions().includes(ScrollDirection::UP));
    }

    #[test]
    fn view_and_world_are_inverse_at_integer_zoom() {
        let v = viewport();
        let world = v.view_to_world_px((100, 200));
        assert_eq!(v.world_to_view_px(world), (100, 200));
    }

    #[test]
    fn zoom_at_a_point_keeps_it_in_place() {
        let mut v = viewport();
        let before = v.view_to_world_px((640, 400));
        v.adjust_zoom_at(0.5, (640, 400));
        let after = v.view_to_world_px((640, 400));
        assert!((before.0 - after.0).abs() <= 1 && (before.1 - after.1).abs() <= 1);
    }
}
