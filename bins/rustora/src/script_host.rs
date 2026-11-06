// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The client side of the script globals that reach outside the simulation: map messages, text lines,
//! the camera and the mission text.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use rustora_core::fieldloader::Color;
use rustora_core::world_types::WPos;
use rustora_engine::map::Map;
use rustora_engine::player::PlayerRef;
use rustora_engine::world::World;
use rustora_scripting::host::{MessageArg, ScriptHost};
use rustora_ui::assets::Assets;
use rustora_ui::fluent::{MessageArg as UiArg, Messages};
use rustora_ui::net::TextNotificationPool;

const WHITE: Color = Color::from_argb(255, 255, 255, 255);

/// A text line a script added with `Media.DisplayMessage`, `DisplaySystemMessage` or `Debug`.
pub struct ScriptLine {
    pub pool: TextNotificationPool,
    pub prefix: String,
    pub text: String,
    pub prefix_color: Option<Color>,
}

/// `Map.Open` as a file system, for the `FluentMessages` of a map.
struct MapFs<'a>(&'a Map);

impl rustora_fs::ReadFs for MapFs<'_> {
    fn try_open(&self, filename: &str) -> Option<Vec<u8>> {
        self.0.open(filename)
    }

    fn exists(&self, filename: &str) -> bool {
        self.0.open(filename).is_some()
    }
}

/// The [`ScriptHost`] of the client. The client feeds it the camera position and the local player and drains
/// what the scripts asked for after each world tick.
pub struct ClientScriptHost {
    assets: Rc<RefCell<Assets>>,
    map_messages: RefCell<Option<Messages>>,
    pub local_player: RefCell<Option<PlayerRef>>,
    pub camera: Cell<Option<WPos>>,
    pub camera_request: Cell<Option<WPos>>,
    pub lines: RefCell<Vec<ScriptLine>>,
    pub mission_text: RefCell<(String, Color)>,
}

impl ClientScriptHost {
    pub fn new(assets: Rc<RefCell<Assets>>) -> ClientScriptHost {
        ClientScriptHost {
            assets,
            map_messages: RefCell::new(None),
            local_player: RefCell::new(None),
            camera: Cell::new(None),
            camera_request: Cell::new(None),
            lines: RefCell::new(Vec::new()),
            mission_text: RefCell::new((String::new(), WHITE)),
        }
    }
}

impl ScriptHost for ClientScriptHost {
    /// Loads the `FluentMessages` of the map, the map bundle of `FluentProvider`, and resets the state of the
    /// previous world.
    fn script_context_created(&self, world: &Rc<World>) {
        let map = world.map();
        let files: Vec<String> = map
            .package
            .yaml
            .node_with_key_or_default("FluentMessages")
            .ok()
            .flatten()
            .map(|n| {
                n.value
                    .value_str()
                    .split(',')
                    .map(str::trim)
                    .filter(|f| !f.is_empty())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        *self.map_messages.borrow_mut() = (!files.is_empty()).then(|| Messages::new(&files, &MapFs(map)));
        self.lines.borrow_mut().clear();
        self.camera_request.set(None);
        *self.mission_text.borrow_mut() = (String::new(), WHITE);
    }

    /// `FluentProvider.GetMessage`: the mod messages take precedence over the map messages.
    fn fluent_message(&self, key: &str, args: &[(String, MessageArg)]) -> String {
        let args: Vec<(&str, UiArg)> = args
            .iter()
            .map(|(name, value)| {
                let value = match value {
                    MessageArg::Text(t) => UiArg::Text(t),
                    MessageArg::Number(n) => UiArg::Number(*n as i64),
                };
                (name.as_str(), value)
            })
            .collect();
        if let Some(message) = self.assets.borrow().messages.try_get(key, &args) {
            return message;
        }
        match self.map_messages.borrow().as_ref() {
            Some(map) => map.get_with(key, &args),
            None => key.to_string(),
        }
    }

    fn render_or_local_player(&self, _world: &World) -> Option<PlayerRef> {
        self.local_player.borrow().clone()
    }

    fn add_text_line(&self, kind: &str, prefix: &str, text: &str, color: Option<Color>) {
        let (pool, prefix) = match kind {
            "Mission" => (TextNotificationPool::Mission, prefix),
            "Debug" => (TextNotificationPool::System, "Debug"),
            "Chat" => (TextNotificationPool::Chat, prefix),
            _ => (TextNotificationPool::System, prefix),
        };
        self.lines.borrow_mut().push(ScriptLine {
            pool,
            prefix: prefix.to_string(),
            text: text.to_string(),
            prefix_color: color,
        });
    }

    fn camera_position(&self) -> Option<WPos> {
        self.camera.get()
    }

    fn set_camera_position(&self, pos: WPos) {
        self.camera.set(Some(pos));
        self.camera_request.set(Some(pos));
    }

    fn set_mission_text(&self, text: &str, color: Option<Color>) {
        *self.mission_text.borrow_mut() = (text.to_string(), color.unwrap_or(WHITE));
    }
}
