// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `WidgetLoader.cs`.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use rustora_core::expression::IntegerExpression;
use rustora_core::miniyaml::{MiniYaml, MiniYamlNode};
use rustora_fs::ReadFs;

use crate::draw::Rect;
use crate::logic;
use crate::widget::{Ui, WidgetArgs, WidgetId};
use crate::widgets;

/// The named widgets of all chrome layout files.
pub struct WidgetLoader {
    widgets: HashMap<String, MiniYamlNode>,
    /// Widget types, logic classes and fields that exist in the layouts but are not ported yet, with use counts.
    pub missing: RefCell<BTreeMap<String, u32>>,
}

fn parse_bool(value: &str) -> bool {
    value.eq_ignore_ascii_case("true")
}

impl WidgetLoader {
    pub fn new(files: &[String], fs: &dyn ReadFs) -> Result<WidgetLoader, String> {
        let mut widgets = HashMap::new();
        for file in files {
            let data = fs.open(file).map_err(|e| e.to_string())?;
            for w in MiniYaml::from_bytes(&data, file, true).map_err(|e| e.to_string())? {
                let key = w.key.split_once('@').map_or(w.key.as_str(), |k| k.1).to_string();
                if widgets.insert(key, w.clone()).is_some() {
                    return Err(format!("Widget has duplicate key `{}`", w.key));
                }
            }
        }
        Ok(WidgetLoader {
            widgets,
            missing: RefCell::new(BTreeMap::new()),
        })
    }

    fn note_missing(&self, what: String) {
        *self.missing.borrow_mut().entry(what).or_insert(0) += 1;
    }

    /// Loads a widget by name.
    pub fn load_widget(&self, ui: &mut Ui, args: &WidgetArgs, parent: Option<WidgetId>, name: &str) -> WidgetId {
        let node = self
            .widgets
            .get(name)
            .unwrap_or_else(|| panic!("Cannot find widget with Id `{name}`"));
        self.load_node(ui, args, parent, node)
    }

    /// Port of `LoadWidget`.
    pub fn load_node(&self, ui: &mut Ui, args: &WidgetArgs, parent: Option<WidgetId>, node: &MiniYamlNode) -> WidgetId {
        let (type_name, name) = match node.key.split_once('@') {
            Some((t, n)) => (t, Some(n)),
            None => (node.key.as_str(), None),
        };
        let kind = {
            let assets = ui.assets.clone();
            let assets = assets.borrow();
            widgets::create(type_name, &assets).unwrap_or_else(|| {
                self.note_missing(format!("widget {type_name}"));
                widgets::container()
            })
        };
        let ignore_mouse_over = kind.borrow().ignores_mouse_over();
        let id = ui.alloc(kind.clone());
        ui.node_mut(id).ignore_mouse_over = ignore_mouse_over;
        ui.node_mut(id).id = name.map(str::to_string);
        if let Some(parent) = parent {
            ui.add_child(parent, id);
        }

        for field in node.value.nodes.iter().filter(|n| n.key != "Children") {
            self.set_field(ui, id, type_name, field);
        }
        self.initialize(ui, args, id);
        kind.borrow_mut().initialize(ui, id);

        for child in node.value.nodes.iter().filter(|n| n.key == "Children") {
            for c in &child.value.nodes {
                self.load_node(ui, args, Some(id), c);
            }
        }

        let logic_args = node
            .value
            .nodes
            .iter()
            .find(|n| n.key == "Logic")
            .map(|n| n.value.nodes.clone())
            .unwrap_or_default();
        self.post_init(ui, args, id, &logic_args);
        id
    }

    fn set_field(&self, ui: &mut Ui, id: WidgetId, type_name: &str, field: &MiniYamlNode) {
        let value = field.value.value_str();
        let expression = || IntegerExpression::new(value).ok();
        let w = ui.node_mut(id);
        match field.key.as_str() {
            "X" => w.x = expression(),
            "Y" => w.y = expression(),
            "Width" => w.width = expression(),
            "Height" => w.height = expression(),
            "Logic" => w.logic = value.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
            "Visible" => w.visible = parse_bool(value),
            "IgnoreMouseOver" => w.ignore_mouse_over = parse_bool(value),
            "IgnoreChildMouseOver" => w.ignore_child_mouse_over = parse_bool(value),
            key => {
                let kind = ui.kind(id);
                if !kind.borrow_mut().set_field(key, value) {
                    self.note_missing(format!("field {type_name}.{key}"));
                }
            }
        }
    }

    /// Port of `Widget.Initialize`: evaluates the bounds.
    fn initialize(&self, ui: &mut Ui, args: &WidgetArgs, id: WidgetId) {
        let parent = ui
            .node(id)
            .parent
            .map_or(Rect::new(0, 0, ui.resolution.0, ui.resolution.1), |p| ui.node(p).bounds);
        let mut subs = args.substitutions.clone();
        subs.insert("WINDOW_WIDTH".into(), ui.resolution.0);
        subs.insert("WINDOW_HEIGHT".into(), ui.resolution.1);
        subs.insert("PARENT_WIDTH".into(), parent.w);
        subs.insert("PARENT_HEIGHT".into(), parent.h);
        let node = ui.node(id);
        let (ex, ey, ew, eh) = (node.x.clone(), node.y.clone(), node.width.clone(), node.height.clone());
        let eval = |e: &Option<IntegerExpression>, subs: &HashMap<String, i32>| e.as_ref().map_or(0, |e| e.evaluate(subs));
        let w = eval(&ew, &subs);
        let h = eval(&eh, &subs);
        subs.insert("WIDTH".into(), w);
        subs.insert("HEIGHT".into(), h);
        let x = eval(&ex, &subs);
        let y = eval(&ey, &subs);
        ui.node_mut(id).bounds = Rect::new(x, y, w, h);
    }

    /// Port of `Widget.PostInit`: creates the logic objects.
    fn post_init(&self, ui: &mut Ui, args: &WidgetArgs, id: WidgetId, logic_args: &[MiniYamlNode]) {
        for name in ui.node(id).logic.clone() {
            match logic::create(&name, ui, id, args, logic_args) {
                Some(object) => {
                    ui.node_mut(id).logic_objects.push(object);
                }
                None => self.note_missing(format!("logic {name}")),
            }
        }
    }
}

/// Wraps a value as the cell type that the arena stores.
pub fn cell<T>(value: T) -> Rc<RefCell<T>> {
    Rc::new(RefCell::new(value))
}
