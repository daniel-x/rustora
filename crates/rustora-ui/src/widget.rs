// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Widget.cs`. Widgets live in an arena and refer to each other by index. Widget types implement
//! [`WidgetKind`]. Each kind sits behind its own `RefCell`, so that its methods can change the tree.

use std::any::Any;
use std::cell::{RefCell, RefMut};
use std::collections::HashMap;
use std::rc::Rc;

use rustora_core::expression::IntegerExpression;

use crate::assets::Assets;
use crate::draw::{DrawContext, Rect};
use crate::input::{KeyInput, MouseEvent, MouseInput};
use crate::loader::WidgetLoader;

/// Index of a widget in the arena.
pub type WidgetId = usize;

pub type Action = Rc<dyn Fn(&mut Ui)>;
pub type Getter<T> = Rc<dyn Fn() -> T>;

/// The per type part of a widget.
pub trait WidgetKind: Any {
    fn as_any(&mut self) -> &mut dyn Any;

    /// Loads one YAML field, returns false if the type has no such field.
    fn set_field(&mut self, _key: &str, _value: &str) -> bool {
        false
    }

    fn initialize(&mut self, _ui: &mut Ui, _id: WidgetId) {}

    fn draw(&mut self, _ui: &mut Ui, _id: WidgetId, _dc: &mut DrawContext) {}

    /// Draws the widget. Types that clip or move their children draw them here and return true.
    fn draw_outer(&mut self, ui: &mut Ui, id: WidgetId, dc: &mut DrawContext) -> bool {
        self.draw(ui, id, dc);
        false
    }

    fn tick(&mut self, _ui: &mut Ui, _id: WidgetId) {}

    fn handle_mouse(&mut self, _ui: &mut Ui, _id: WidgetId, _mi: &MouseInput) -> bool {
        false
    }

    fn handle_key(&mut self, _ui: &mut Ui, _id: WidgetId, _e: &KeyInput) -> bool {
        false
    }

    fn handle_text(&mut self, _ui: &mut Ui, _id: WidgetId, _text: &str) -> bool {
        false
    }

    fn mouse_entered(&mut self, _ui: &mut Ui, _id: WidgetId) {}

    fn mouse_exited(&mut self, _ui: &mut Ui, _id: WidgetId) {}

    /// Returns false to hint that the widget does not want to give up the focus.
    fn yield_mouse_focus(&mut self, ui: &mut Ui, id: WidgetId, _mi: Option<&MouseInput>) -> bool {
        if ui.mouse_focus == Some(id) {
            ui.mouse_focus = None;
        }
        true
    }

    fn yield_keyboard_focus(&mut self, ui: &mut Ui, id: WidgetId) -> bool {
        if ui.keyboard_focus == Some(id) {
            ui.keyboard_focus = None;
        }
        true
    }

    /// The default of `IgnoreMouseOver`.
    fn ignores_mouse_over(&self) -> bool {
        false
    }

    /// Port of `GetCursor`.
    fn cursor(&self, default: &str) -> Option<String> {
        Some(default.to_string())
    }
}

/// The part that all widgets share.
pub struct Widget {
    pub id: Option<String>,
    pub x: Option<IntegerExpression>,
    pub y: Option<IntegerExpression>,
    pub width: Option<IntegerExpression>,
    pub height: Option<IntegerExpression>,
    pub logic: Vec<String>,
    pub visible: bool,
    pub ignore_mouse_over: bool,
    pub ignore_child_mouse_over: bool,
    pub bounds: Rect,
    pub parent: Option<WidgetId>,
    pub children: Vec<WidgetId>,
    pub is_visible: Option<Getter<bool>>,
    pub logic_objects: Vec<Rc<RefCell<dyn ChromeLogic>>>,
    /// The offset of the children relative to the origin of this widget, kept here so that children can read it
    /// while the kind of this widget is borrowed.
    pub child_offset: (i32, i32),
    kind: Rc<RefCell<dyn WidgetKind>>,
}

/// Port of `ChromeLogic`: a controller created by the `Logic` field of a widget.
pub trait ChromeLogic: Any {
    fn tick(&mut self, _ui: &mut Ui) {}
    fn became_hidden(&mut self, _ui: &mut Ui) {}
    fn became_visible(&mut self, _ui: &mut Ui) {}
    fn disposed(&mut self, _ui: &mut Ui) {}
}

/// Port of `WidgetArgs`, with the integer substitutions in a field of their own.
#[derive(Clone, Default)]
pub struct WidgetArgs {
    pub substitutions: HashMap<String, i32>,
    pub values: HashMap<String, Rc<dyn Any>>,
    pub actions: HashMap<String, Action>,
}

impl WidgetArgs {
    pub fn with_action(mut self, name: &str, action: Action) -> WidgetArgs {
        self.actions.insert(name.to_string(), action);
        self
    }

    pub fn with_value<T: Any>(mut self, name: &str, value: T) -> WidgetArgs {
        self.values.insert(name.to_string(), Rc::new(value));
        self
    }

    pub fn value<T: Any + Clone>(&self, name: &str) -> Option<T> {
        self.values.get(name).and_then(|v| v.downcast_ref::<T>()).cloned()
    }

    /// Runs the named action if it exists.
    pub fn action(&self, name: &str) -> Action {
        self.actions.get(name).cloned().unwrap_or_else(|| Rc::new(|_| {}))
    }
}

/// Requests that the application carries out after the current frame.
pub enum AppRequest {
    Quit,
}

/// Port of the static `Ui` class and the widget tree.
pub struct Ui {
    pub assets: Rc<RefCell<Assets>>,
    pub loader: Rc<WidgetLoader>,
    pub root: WidgetId,
    pub resolution: (i32, i32),
    pub mouse_focus: Option<WidgetId>,
    pub keyboard_focus: Option<WidgetId>,
    pub mouse_over: Option<WidgetId>,
    pub requests: Vec<AppRequest>,
    pub services: Rc<dyn Any>,
    window_list: Vec<WidgetId>,
    nodes: Vec<Option<Widget>>,
    free: Vec<WidgetId>,
    run_after_tick: Vec<Action>,
}

struct Container;

impl WidgetKind for Container {
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }

    fn ignores_mouse_over(&self) -> bool {
        true
    }
}

impl Ui {
    pub fn new(assets: Rc<RefCell<Assets>>, loader: Rc<WidgetLoader>, resolution: (i32, i32), services: Rc<dyn Any>) -> Ui {
        let mut ui = Ui {
            assets,
            loader,
            root: 0,
            resolution,
            mouse_focus: None,
            keyboard_focus: None,
            mouse_over: None,
            requests: Vec::new(),
            services,
            window_list: Vec::new(),
            nodes: Vec::new(),
            free: Vec::new(),
            run_after_tick: Vec::new(),
        };
        ui.root = ui.alloc(Rc::new(RefCell::new(Container)));
        ui.node_mut(ui.root).ignore_mouse_over = true;
        ui.node_mut(ui.root).bounds = Rect::new(0, 0, resolution.0, resolution.1);
        ui
    }

    pub fn alloc(&mut self, kind: Rc<RefCell<dyn WidgetKind>>) -> WidgetId {
        let widget = Widget {
            id: None,
            x: None,
            y: None,
            width: None,
            height: None,
            logic: Vec::new(),
            visible: true,
            ignore_mouse_over: false,
            ignore_child_mouse_over: false,
            bounds: Rect::default(),
            parent: None,
            children: Vec::new(),
            is_visible: None,
            logic_objects: Vec::new(),
            child_offset: (0, 0),
            kind,
        };
        match self.free.pop() {
            Some(id) => {
                self.nodes[id] = Some(widget);
                id
            }
            None => {
                self.nodes.push(Some(widget));
                self.nodes.len() - 1
            }
        }
    }

    pub fn node(&self, id: WidgetId) -> &Widget {
        self.nodes[id].as_ref().expect("live widget")
    }

    pub fn node_mut(&mut self, id: WidgetId) -> &mut Widget {
        self.nodes[id].as_mut().expect("live widget")
    }

    pub fn exists(&self, id: WidgetId) -> bool {
        self.nodes.get(id).is_some_and(Option::is_some)
    }

    pub fn kind(&self, id: WidgetId) -> Rc<RefCell<dyn WidgetKind>> {
        self.node(id).kind.clone()
    }

    /// Borrows the type specific part of a widget.
    pub fn get<T: WidgetKind>(&self, id: WidgetId) -> RefMut<'_, T> {
        let cell = &self.node(id).kind;
        RefMut::map(cell.borrow_mut(), |k| {
            k.as_any().downcast_mut::<T>().expect("widget of the requested type")
        })
    }

    /// Port of `GetOrNull`: searches the widget and its descendants.
    pub fn find(&self, id: WidgetId, name: &str) -> Option<WidgetId> {
        let node = self.node(id);
        if node.id.as_deref() == Some(name) {
            return Some(id);
        }
        node.children.iter().find_map(|&c| self.find(c, name))
    }

    /// Port of `Get`.
    pub fn child(&self, id: WidgetId, name: &str) -> WidgetId {
        self.find(id, name)
            .unwrap_or_else(|| panic!("widget {:?} has no child {name}", self.node(id).id))
    }

    pub fn set_visible_fn(&mut self, id: WidgetId, f: impl Fn() -> bool + 'static) {
        self.node_mut(id).is_visible = Some(Rc::new(f));
    }

    pub fn is_visible(&self, id: WidgetId) -> bool {
        let node = self.node(id);
        match &node.is_visible {
            Some(f) => f(),
            None => node.visible,
        }
    }

    pub fn run_after_tick(&mut self, action: impl Fn(&mut Ui) + 'static) {
        self.run_after_tick.push(Rc::new(action));
    }

    pub fn render_origin(&self, id: WidgetId) -> (i32, i32) {
        let node = self.node(id);
        let (px, py) = node.parent.map_or((0, 0), |p| self.child_origin(p));
        (node.bounds.x + px, node.bounds.y + py)
    }

    pub fn child_origin(&self, id: WidgetId) -> (i32, i32) {
        let (x, y) = self.render_origin(id);
        let (dx, dy) = self.node(id).child_offset;
        (x + dx, y + dy)
    }

    pub fn render_bounds(&self, id: WidgetId) -> Rect {
        let (x, y) = self.render_origin(id);
        let b = self.node(id).bounds;
        Rect::new(x, y, b.w, b.h)
    }

    /// Port of `EventBoundsContains`.
    pub fn event_bounds_contains(&self, id: WidgetId, x: i32, y: i32) -> bool {
        if self.render_bounds(id).contains(x, y) {
            return true;
        }
        self.node(id)
            .children
            .iter()
            .any(|&c| self.is_visible(c) && self.event_bounds_contains(c, x, y))
    }

    pub fn has_mouse_focus(&self, id: WidgetId) -> bool {
        self.mouse_focus == Some(id)
    }

    pub fn has_keyboard_focus(&self, id: WidgetId) -> bool {
        self.keyboard_focus == Some(id)
    }

    /// Port of `TakeMouseFocus`.
    pub fn take_mouse_focus(&mut self, id: WidgetId, mi: &MouseInput) -> bool {
        if self.has_mouse_focus(id) {
            return true;
        }
        if let Some(current) = self.mouse_focus {
            let kind = self.kind(current);
            if !kind.borrow_mut().yield_mouse_focus(self, current, Some(mi)) {
                return false;
            }
        }
        self.mouse_focus = Some(id);
        true
    }

    /// Port of `TakeKeyboardFocus`.
    pub fn take_keyboard_focus(&mut self, id: WidgetId) -> bool {
        if self.has_keyboard_focus(id) {
            return true;
        }
        if let Some(current) = self.keyboard_focus {
            let kind = self.kind(current);
            if !kind.borrow_mut().yield_keyboard_focus(self, current) {
                return false;
            }
        }
        self.keyboard_focus = Some(id);
        true
    }

    pub fn add_child(&mut self, parent: WidgetId, child: WidgetId) {
        self.node_mut(child).parent = Some(parent);
        self.node_mut(parent).children.push(child);
    }

    /// Port of `RemoveChild` and `Removed`: disposes the child and all its descendants.
    pub fn remove_child(&mut self, parent: WidgetId, child: WidgetId) {
        self.node_mut(parent).children.retain(|&c| c != child);
        self.dispose(child);
    }

    /// Port of `HideChild`: takes the child out of the tree and keeps it for a later `add_child`.
    pub fn hide_child(&mut self, parent: WidgetId, child: WidgetId) {
        self.node_mut(parent).children.retain(|&c| c != child);
        self.force_yield(child);
    }

    pub fn remove_children(&mut self, id: WidgetId) {
        let children = std::mem::take(&mut self.node_mut(id).children);
        for c in children {
            self.dispose(c);
        }
    }

    fn force_yield(&mut self, id: WidgetId) {
        if self.keyboard_focus == Some(id) {
            self.keyboard_focus = None;
        }
        if self.mouse_focus == Some(id) {
            self.mouse_focus = None;
        }
        if self.mouse_over == Some(id) {
            self.mouse_over = None;
        }
        for c in self.node(id).children.clone() {
            self.force_yield(c);
        }
    }

    fn dispose(&mut self, id: WidgetId) {
        self.force_yield(id);
        for c in self.node(id).children.clone() {
            self.dispose(c);
        }
        for logic in self.node(id).logic_objects.clone() {
            logic.borrow_mut().disposed(self);
        }
        self.nodes[id] = None;
        self.free.push(id);
    }

    /// Port of `Ui.CloseWindow`.
    pub fn close_window(&mut self) {
        if let Some(hidden) = self.window_list.pop() {
            self.remove_child(self.root, hidden);
        }
        if let Some(&restore) = self.window_list.last() {
            self.add_child(self.root, restore);
            for logic in self.node(restore).logic_objects.clone() {
                logic.borrow_mut().became_visible(self);
            }
        }
    }

    /// Port of `Ui.OpenWindow`.
    pub fn open_window(&mut self, name: &str, args: WidgetArgs) -> WidgetId {
        let loader = self.loader.clone();
        let window = loader.load_widget(self, &args, Some(self.root), name);
        if let Some(&current) = self.window_list.last() {
            self.hide_child(self.root, current);
            for logic in self.node(current).logic_objects.clone() {
                logic.borrow_mut().became_hidden(self);
            }
        }
        self.window_list.push(window);
        window
    }

    pub fn current_window(&self) -> Option<WidgetId> {
        self.window_list.last().copied()
    }

    /// Port of `Ui.LoadWidget`.
    pub fn load_widget(&mut self, name: &str, parent: Option<WidgetId>, args: &WidgetArgs) -> WidgetId {
        let loader = self.loader.clone();
        loader.load_widget(self, args, parent, name)
    }

    /// Port of `Ui.ResetAll`.
    pub fn reset_all(&mut self) {
        self.remove_children(self.root);
        self.window_list.clear();
    }

    /// Port of `Ui.Tick`, followed by the actions of `Game.RunAfterTick`.
    pub fn tick(&mut self) {
        self.tick_outer(self.root);
        for action in std::mem::take(&mut self.run_after_tick) {
            action(self);
        }
    }

    fn tick_outer(&mut self, id: WidgetId) {
        if !self.is_visible(id) {
            return;
        }
        let kind = self.kind(id);
        kind.borrow_mut().tick(self, id);
        for c in self.node(id).children.clone() {
            if self.exists(c) {
                self.tick_outer(c);
            }
        }
        if self.exists(id) {
            for logic in self.node(id).logic_objects.clone() {
                logic.borrow_mut().tick(self);
            }
        }
    }

    /// Port of `Ui.Draw`.
    pub fn draw(&mut self, dc: &mut DrawContext) {
        self.draw_outer(self.root, dc);
    }

    pub fn draw_outer(&mut self, id: WidgetId, dc: &mut DrawContext) {
        if self.is_visible(id) {
            let kind = self.kind(id);
            let children_done = kind.borrow_mut().draw_outer(self, id, dc);
            if !children_done {
                self.draw_children(id, dc);
            }
        }
    }

    pub fn draw_children(&mut self, id: WidgetId, dc: &mut DrawContext) {
        for c in self.node(id).children.clone() {
            self.draw_outer(c, dc);
        }
    }

    /// Port of `Ui.HandleInput` for the mouse.
    pub fn handle_input(&mut self, mi: &MouseInput) -> bool {
        let was_over = self.mouse_over;
        if mi.event == MouseEvent::Move {
            self.mouse_over = None;
        }
        let mut handled = false;
        if let Some(focus) = self.mouse_focus {
            handled = self.handle_mouse_outer(focus, mi);
        }
        if !handled {
            handled = self.handle_mouse_outer(self.root, mi);
        }
        if was_over != self.mouse_over {
            if let Some(w) = was_over.filter(|&w| self.exists(w)) {
                let kind = self.kind(w);
                kind.borrow_mut().mouse_exited(self, w);
            }
            if let Some(w) = self.mouse_over {
                let kind = self.kind(w);
                kind.borrow_mut().mouse_entered(self, w);
            }
        }
        handled
    }

    pub fn handle_mouse_outer(&mut self, id: WidgetId, mi: &MouseInput) -> bool {
        if !(self.has_mouse_focus(id) || (self.is_visible(id) && self.event_bounds_contains(id, mi.x, mi.y))) {
            return false;
        }
        let old_over = self.mouse_over;
        for c in self.node(id).children.clone().into_iter().rev() {
            if self.exists(c) && self.handle_mouse_outer(c, mi) {
                return true;
            }
        }
        if !self.exists(id) {
            return true;
        }
        if self.node(id).ignore_child_mouse_over {
            self.mouse_over = old_over;
        }
        if mi.event == MouseEvent::Move && self.mouse_over.is_none() && !self.node(id).ignore_mouse_over {
            self.mouse_over = Some(id);
        }
        let kind = self.kind(id);
        let handled = kind.borrow_mut().handle_mouse(self, id, mi);
        handled
    }

    /// Port of `Ui.HandleKeyPress`.
    pub fn handle_key(&mut self, e: &KeyInput) -> bool {
        let start = self.keyboard_focus.unwrap_or(self.root);
        self.handle_key_outer(start, e)
    }

    fn handle_key_outer(&mut self, id: WidgetId, e: &KeyInput) -> bool {
        if !self.is_visible(id) {
            return false;
        }
        for c in self.node(id).children.clone().into_iter().rev() {
            if self.exists(c) && self.handle_key_outer(c, e) {
                return true;
            }
        }
        if !self.exists(id) {
            return true;
        }
        let kind = self.kind(id);
        let handled = kind.borrow_mut().handle_key(self, id, e);
        handled
    }

    /// Port of `Ui.HandleTextInput`.
    pub fn handle_text(&mut self, text: &str) -> bool {
        let start = self.keyboard_focus.unwrap_or(self.root);
        self.handle_text_outer(start, text)
    }

    fn handle_text_outer(&mut self, id: WidgetId, text: &str) -> bool {
        if !self.is_visible(id) {
            return false;
        }
        for c in self.node(id).children.clone().into_iter().rev() {
            if self.exists(c) && self.handle_text_outer(c, text) {
                return true;
            }
        }
        if !self.exists(id) {
            return true;
        }
        let kind = self.kind(id);
        let handled = kind.borrow_mut().handle_text(self, id, text);
        handled
    }

    /// Port of `GetCursorOuter`.
    pub fn cursor_at(&self, id: WidgetId, x: i32, y: i32) -> Option<String> {
        if !(self.is_visible(id) && self.event_bounds_contains(id, x, y)) {
            return None;
        }
        for &c in self.node(id).children.iter().rev() {
            if let Some(cursor) = self.cursor_at(c, x, y) {
                return Some(cursor);
            }
        }
        if !self.render_bounds(id).contains(x, y) {
            return None;
        }
        let default = self.assets.borrow().metrics.string("DefaultCursor");
        self.node(id).kind.borrow().cursor(&default)
    }
}
