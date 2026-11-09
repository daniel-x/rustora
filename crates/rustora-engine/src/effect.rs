// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! World effects (`IEffect`), such as projectiles.

use std::any::Any;
use std::rc::Rc;

use crate::player::PlayerRef;
use crate::render::Renderable;
use crate::world::World;

pub type EffectRef = Rc<dyn Effect>;

pub trait Effect: Any + crate::persist::PersistDyn {
    fn tick(&self, world: &Rc<World>);
    /// Whether the effect implements `ISync`.
    fn is_sync(&self) -> bool {
        false
    }
    fn sync_hash(&self) -> i32 {
        0
    }
    /// True for effects that only show something and do not change the simulation; save games leave them out.
    fn is_cosmetic(&self) -> bool {
        false
    }
    /// The type name without its module path, for the sync detail dump.
    fn type_name(&self) -> &'static str {
        let full = std::any::type_name::<Self>();
        full.rsplit("::").next().unwrap_or(full)
    }
    /// `IEffect.Render`.
    fn render(&self, _viewer: Option<&PlayerRef>, _out: &mut Vec<Renderable>) {}
    /// `IEffectAboveShroud.RenderAboveShroud`.
    fn render_above_shroud(&self, _viewer: Option<&PlayerRef>, _out: &mut Vec<Renderable>) {}
    /// `IEffectAnnotation.RenderAnnotation` of a `TextAnnotationRenderable` in the `TinyBold` font.
    fn annotation_text(
        &self,
        _viewer: Option<&PlayerRef>,
    ) -> Option<(rustora_core::world_types::WPos, rustora_core::fieldloader::Color, String)> {
        None
    }
    fn as_any(&self) -> &dyn Any;
}
