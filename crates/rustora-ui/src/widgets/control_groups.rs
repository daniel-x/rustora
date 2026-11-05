// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `ControlGroupsWidget.cs`.

use std::rc::Rc;

use rustora_engine::actor::ActorRef;
use rustora_mods_common::selection::ControlGroups;

use super::impl_as_any;
use crate::input::{KeyEvent, KeyInput};
use crate::logic::ingame::Ingame;
use crate::logic::services;
use crate::widget::{Ui, WidgetId, WidgetKind};

#[derive(Clone, Default)]
pub struct ControlGroupsWidget {
    select_prefix: String,
    create_prefix: String,
    add_to_prefix: String,
    combine_with_prefix: String,
    jump_to_prefix: String,
    ingame: Option<Rc<Ingame>>,
    groups: Option<Rc<ControlGroups>>,
}

/// `Viewport.Center(IEnumerable<Actor>)`: the average center position of the actors.
pub fn center_on_actors(ingame: &Ingame, actors: &[ActorRef]) {
    if actors.is_empty() {
        return;
    }
    let n = actors.len() as i64;
    let (mut x, mut y, mut z) = (0i64, 0i64, 0i64);
    for a in actors {
        let p = a.center_position();
        x += p.x as i64;
        y += p.y as i64;
        z += p.z as i64;
    }
    let (x, y, z) = ((x / n) as i32, (y / n) as i32, (z / n) as i32);
    ingame.viewport.borrow_mut().center_on((x * 24 / 1024, (y - z) * 24 / 1024));
}

impl WidgetKind for ControlGroupsWidget {
    impl_as_any!();

    fn set_field(&mut self, key: &str, value: &str) -> bool {
        let target = match key {
            "SelectGroupKeyPrefix" => &mut self.select_prefix,
            "CreateGroupKeyPrefix" => &mut self.create_prefix,
            "AddToGroupKeyPrefix" => &mut self.add_to_prefix,
            "CombineWithGroupKeyPrefix" => &mut self.combine_with_prefix,
            "JumpToGroupKeyPrefix" => &mut self.jump_to_prefix,
            _ => return false,
        };
        *target = value.to_string();
        true
    }

    fn initialize(&mut self, ui: &mut Ui, _id: WidgetId) {
        self.ingame = services(ui).ingame.borrow().clone();
        self.groups = self.ingame.as_ref().and_then(|i| ControlGroups::of(&i.world));
    }

    fn handle_key(&mut self, ui: &mut Ui, _id: WidgetId, e: &KeyInput) -> bool {
        let (Some(ingame), Some(groups)) = (&self.ingame, &self.groups) else {
            return false;
        };
        if e.event != KeyEvent::Down {
            return false;
        }
        let hotkeys = &ui.assets.borrow().hotkeys;
        let is = |prefix: &str, i: usize| !prefix.is_empty() && hotkeys.is_activated(&format!("{prefix}{:02}", i + 1), e);
        let selection = &ingame.selection;
        for i in 0..groups.count() {
            if is(&self.select_prefix, i) {
                groups.select_control_group(selection, i);
                if e.multi_tap >= 2 {
                    center_on_actors(ingame, &groups.actors_in_control_group(i));
                }
                return true;
            }
            if is(&self.create_prefix, i) {
                groups.create_control_group(selection, i);
                return true;
            }
            if is(&self.add_to_prefix, i) {
                groups.add_selection_to_control_group(selection, i);
                return true;
            }
            if is(&self.combine_with_prefix, i) {
                groups.combine_selection_with_control_group(selection, i);
                return true;
            }
            if is(&self.jump_to_prefix, i) {
                center_on_actors(ingame, &groups.actors_in_control_group(i));
                return true;
            }
        }
        false
    }

    fn ignores_mouse_over(&self) -> bool {
        true
    }

    fn cursor(&self, _default: &str) -> Option<String> {
        None
    }
}
