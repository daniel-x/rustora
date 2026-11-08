// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! `IRenderable` as plain data. The simulation crates describe what to draw, and the client turns it into quads.

use std::rc::Rc;

use rustora_core::world_types::{WAngle, WPos, WVec};

use crate::actor::ActorRef;
use crate::player::PlayerRef;
use crate::sequence::{Animation, AnimationWithOffset, Sequence};

/// `PaletteReference`: a named palette, or the player palette `name + player.InternalName`.
#[derive(Clone)]
pub struct PaletteRef {
    pub name: Rc<str>,
    pub player: Option<PlayerRef>,
}

impl PaletteRef {
    pub fn named(name: &str) -> PaletteRef {
        PaletteRef {
            name: Rc::from(name),
            player: None,
        }
    }

    pub fn player(base: &str, player: &PlayerRef) -> PaletteRef {
        PaletteRef {
            name: Rc::from(base),
            player: Some(player.clone()),
        }
    }

    /// `RenderSprites.AnimationWrapper.CachePalette` with the owner of the actor.
    pub fn resolve(name: Option<&str>, is_player_palette: bool, owner: &PlayerRef) -> PaletteRef {
        let name = name.unwrap_or("player");
        if is_player_palette {
            PaletteRef::player(name, owner)
        } else {
            PaletteRef::named(name)
        }
    }
}

/// `SpriteRenderable`. The client resolves `sequence`, `frame` and `facing` to a sprite.
#[derive(Clone)]
pub struct SpriteRenderable {
    pub sequence: Rc<Sequence>,
    pub frame: i32,
    pub facing: WAngle,
    pub pos: WPos,
    pub offset: WVec,
    pub z_offset: i32,
    pub palette: PaletteRef,
    pub alpha: f32,
    /// `TintModifiers.ReplaceColor`: draws this RGB color instead of the sprite colors.
    pub replace_color: Option<[f32; 3]>,
    pub is_decoration: bool,
}

/// `IRenderable` kinds. Sorted by `sort_key` before drawing.
#[derive(Clone)]
pub enum Renderable {
    Sprite(SpriteRenderable),
    /// A world space line of `width` world units. Colors are premultiplied RGBA.
    Line {
        from: WPos,
        to: WPos,
        width: f32,
        from_color: [f32; 4],
        to_color: [f32; 4],
    },
    /// One subtile of a terrain template in the terrain palette, as `Bridge` draws it.
    Tile {
        tile: crate::map::TerrainTile,
        pos: WPos,
        z_offset: i32,
    },
    /// Parts drawn together at one sort position, as `ContrailRenderable` and `TeslaZapRenderable` do.
    Group {
        pos: WPos,
        z_offset: i32,
        items: Vec<Renderable>,
    },
}

impl Renderable {
    pub fn pos(&self) -> WPos {
        match self {
            Renderable::Sprite(s) => s.pos + s.offset,
            Renderable::Line { from, .. } => *from,
            Renderable::Tile { pos, .. } => *pos,
            Renderable::Group { pos, .. } => *pos,
        }
    }

    pub fn z_offset(&self) -> i32 {
        match self {
            Renderable::Sprite(s) => s.z_offset,
            Renderable::Line { .. } => 0,
            Renderable::Tile { z_offset, .. } => *z_offset,
            Renderable::Group { z_offset, .. } => *z_offset,
        }
    }

    /// `WorldRenderer.RenderableZPositionComparisonKey` without the sequence's own `ZOffset`, which the client adds.
    pub fn sort_key(&self) -> i32 {
        let p = self.pos();
        p.y + p.z + self.z_offset()
    }
}

/// `World.FogObscures` for the render player `viewer`.
pub fn fog_obscures(viewer: Option<&PlayerRef>, pos: WPos) -> bool {
    viewer.is_some_and(|v| !v.shroud().is_visible_wpos(pos))
}

/// `Animation.Render`.
pub fn render_animation(anim: &Animation, pos: WPos, offset: WVec, z_offset: i32, palette: &PaletteRef, out: &mut Vec<Renderable>) {
    let Some(sequence) = anim.current_sequence() else {
        return;
    };
    out.push(Renderable::Sprite(SpriteRenderable {
        sequence,
        frame: anim.current_frame(),
        facing: (anim.facing_func)(),
        pos,
        offset,
        z_offset,
        palette: palette.clone(),
        alpha: 1.0,
        replace_color: None,
        is_decoration: anim.is_decoration.get(),
    }));
}

/// `AnimationWithOffset.Render`.
pub fn render_animation_with_offset(anim: &AnimationWithOffset, self_: &ActorRef, palette: &PaletteRef, out: &mut Vec<Renderable>) {
    let center = self_.center_position();
    let offset = anim.offset_func.as_ref().map_or_else(Default::default, |f| f());
    let z = anim.zoffset_func.as_ref().map_or(0, |f| f(center + offset));
    render_animation(&anim.animation, center, offset, z, palette, out);
}

/// `IRender`.
pub trait IRender {
    fn render(&self, self_: &ActorRef, out: &mut Vec<Renderable>);
}

/// A shape in world pixel coordinates, drawn by `IRenderAboveWorld` traits with the world RGBA color renderer.
/// Colors are premultiplied RGBA.
pub enum WorldPixelShape {
    /// `RgbaColorRenderer.FillRect` from the top left corner.
    Rect { x: f32, y: f32, w: f32, h: f32, color: [f32; 4] },
    /// `RgbaColorRenderer.DrawLine`.
    Line {
        from: [f32; 2],
        to: [f32; 2],
        width: f32,
        color: [f32; 4],
    },
}

/// The client state that `IRenderAboveWorld` traits read from the `WorldRenderer` and `Game`.
pub struct WorldRenderView {
    /// `Viewport.CenterLocation` in world pixels.
    pub center: (i32, i32),
    /// The smallest zoom of the viewport, as `INotifyViewportZoomExtentsChanged` passes it.
    pub min_zoom: f32,
    /// `Renderer.NativeResolution`.
    pub native_resolution: (u32, u32),
    /// `Game.RunTime` in milliseconds.
    pub run_time: i64,
}

/// `ITerrainLighting`: the light of the world at a position, multiplied into terrain and sprite colors.
pub trait ITerrainLighting {
    fn tint_at(&self, pos: WPos) -> [f32; 3];
}

/// `IRenderAboveWorld`: drawn after the actors and effects, below the shroud.
pub trait IRenderAboveWorld {
    fn render_above_world(&self, self_: &ActorRef, view: &WorldRenderView, out: &mut Vec<WorldPixelShape>);
}

/// `IRenderModifier`.
pub trait IRenderModifier {
    fn modify_render(&self, self_: &ActorRef, renderables: &mut Vec<Renderable>);
}

/// `Actor.Render`.
pub fn render_actor(self_: &ActorRef, out: &mut Vec<Renderable>) {
    let mut own = Vec::new();
    for r in self_.traits_implementing::<dyn IRender>() {
        r.render(self_, &mut own);
    }
    for m in self_.traits_implementing::<dyn IRenderModifier>() {
        m.modify_render(self_, &mut own);
    }
    out.append(&mut own);
}
