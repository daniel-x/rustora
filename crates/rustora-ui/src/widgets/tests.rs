// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! Behavior of the widgets without chrome or fonts.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use rustora_fs::ReadFs;

use super::*;
use crate::assets::Assets;
use crate::draw::Rect;
use crate::input::{KeyEvent, KeyInput, Modifiers, MouseButton, MouseEvent, MouseInput};
use crate::loader::{WidgetLoader, cell};
use crate::widget::{Ui, WidgetArgs, WidgetId};

struct NoFiles;

impl ReadFs for NoFiles {
    fn try_open(&self, _filename: &str) -> Option<Vec<u8>> {
        None
    }

    fn exists(&self, _filename: &str) -> bool {
        false
    }
}

fn new_ui() -> Ui {
    let fs: Rc<dyn ReadFs> = Rc::new(NoFiles);
    let loader = WidgetLoader::new(&[], &*fs).expect("no layouts");
    Ui::new(
        Rc::new(RefCell::new(Assets::empty(fs))),
        Rc::new(loader),
        (800, 600),
        Rc::new(crate::logic::Services::default()),
    )
}

fn mouse(event: MouseEvent, button: MouseButton, x: i32, y: i32) -> MouseInput {
    MouseInput {
        event,
        button,
        x,
        y,
        scroll_delta: 0,
        modifiers: Modifiers::default(),
        multi_tap: 1,
    }
}

fn click(ui: &mut Ui, x: i32, y: i32) {
    ui.handle_input(&mouse(MouseEvent::Move, MouseButton::None, x, y));
    ui.handle_input(&mouse(MouseEvent::Down, MouseButton::Left, x, y));
    ui.handle_input(&mouse(MouseEvent::Up, MouseButton::Left, x, y));
}

fn key(ui: &mut Ui, name: &str, modifiers: Modifiers) {
    ui.handle_key(&KeyInput {
        event: KeyEvent::Down,
        key: name.to_string(),
        modifiers,
        multi_tap: 1,
        repeat: false,
    });
}

const CTRL: Modifiers = Modifiers {
    shift: false,
    ctrl: !cfg!(target_os = "macos"),
    alt: false,
    meta: cfg!(target_os = "macos"),
};
const SHIFT: Modifiers = Modifiers {
    shift: true,
    ctrl: false,
    alt: false,
    meta: false,
};

fn add_panel(ui: &mut Ui, height: i32) -> WidgetId {
    let kind = {
        let assets = ui.assets.borrow();
        cell(ScrollPanelWidget::new(&assets))
    };
    let panel = ui.alloc(kind.clone());
    ui.node_mut(panel).bounds = Rect::new(0, 0, 200, height);
    kind.borrow_mut().initialize(ui, panel);
    ui.add_child(ui.root, panel);
    panel
}

fn add_item(ui: &mut Ui, panel: WidgetId, height: i32) -> WidgetId {
    let item = ui.alloc(container());
    ui.node_mut(item).bounds = Rect::new(0, 0, 100, height);
    ui.add_child(panel, item);
    item
}

fn add_button(ui: &mut Ui, bounds: Rect) -> WidgetId {
    let kind = {
        let assets = ui.assets.borrow();
        cell(ButtonWidget::new(&assets))
    };
    let id = ui.alloc(kind);
    ui.node_mut(id).bounds = bounds;
    ui.add_child(ui.root, id);
    id
}

fn add_text_field(ui: &mut Ui) -> WidgetId {
    let kind = {
        let assets = ui.assets.borrow();
        cell(TextFieldWidget::new(&assets, false))
    };
    let id = ui.alloc(kind);
    ui.node_mut(id).bounds = Rect::new(0, 0, 200, 30);
    ui.add_child(ui.root, id);
    assert!(ui.take_keyboard_focus(id));
    id
}

#[test]
fn list_layout_places_children_like_the_csharp_list_layout() {
    let mut ui = new_ui();
    let panel = add_panel(&mut ui, 100);
    let a = add_item(&mut ui, panel, 30);
    let b = add_item(&mut ui, panel, 40);
    assert_eq!(ui.node(a).bounds.y, 2);
    assert_eq!(ui.node(b).bounds.y, 32);
    assert_eq!(ui.content_height(panel), 74);
    ui.remove_child(panel, a);
    assert_eq!(ui.node(b).bounds.y, 2);
    assert_eq!(ui.content_height(panel), 44);
}

#[test]
fn scrolling_clamps_to_the_content() {
    let mut ui = new_ui();
    let panel = add_panel(&mut ui, 100);
    add_item(&mut ui, panel, 300);
    ui.scroll(panel, -1, false);
    assert_eq!(ui.node(panel).child_offset, (0, -50));
    ui.scroll(panel, -100, false);
    assert_eq!(ui.node(panel).child_offset, (0, -204));
    ui.scroll(panel, 100, false);
    assert_eq!(ui.node(panel).child_offset, (0, 0));
}

#[test]
fn removing_children_resets_the_scroll_position() {
    let mut ui = new_ui();
    let panel = add_panel(&mut ui, 100);
    add_item(&mut ui, panel, 300);
    ui.scroll_to_bottom(panel, false);
    ui.remove_children(panel);
    assert_eq!(ui.node(panel).child_offset, (0, 0));
    assert_eq!(ui.content_height(panel), 0);
}

#[test]
fn a_panel_ignores_clicks_on_children_scrolled_out_of_it() {
    let mut ui = new_ui();
    let panel = add_panel(&mut ui, 100);
    let hit = Rc::new(Cell::new(0));
    let button = add_button(&mut ui, Rect::new(0, 0, 100, 50));
    ui.node_mut(ui.root).children.retain(|&c| c != button);
    ui.add_child(panel, button);
    ui.node_mut(button).bounds = Rect::new(0, 150, 100, 50);
    let counter = hit.clone();
    ui.button(button).on_click = Some(Rc::new(move |_| counter.set(counter.get() + 1)));
    click(&mut ui, 50, 160);
    assert_eq!(hit.get(), 0);
    ui.node_mut(button).bounds.y = 10;
    click(&mut ui, 50, 20);
    assert_eq!(hit.get(), 1);
}

#[test]
fn a_click_action_may_change_its_own_button() {
    let mut ui = new_ui();
    let button = add_button(&mut ui, Rect::new(10, 10, 100, 30));
    ui.button(button).on_click = Some(Rc::new(move |ui| ui.button(button).text = "clicked".into()));
    click(&mut ui, 20, 20);
    assert_eq!(ui.button(button).text, "clicked");
    assert!(!ui.button(button).depressed);
}

#[test]
fn a_click_action_may_remove_its_own_button() {
    let mut ui = new_ui();
    let button = add_button(&mut ui, Rect::new(10, 10, 100, 30));
    ui.button(button).on_click = Some(Rc::new(move |ui| ui.remove_child(ui.root, button)));
    click(&mut ui, 20, 20);
    assert!(!ui.exists(button));
}

#[test]
fn a_release_outside_the_button_does_not_click() {
    let mut ui = new_ui();
    let hit = Rc::new(Cell::new(false));
    let button = add_button(&mut ui, Rect::new(10, 10, 100, 30));
    let flag = hit.clone();
    ui.button(button).on_click = Some(Rc::new(move |_| flag.set(true)));
    ui.handle_input(&mouse(MouseEvent::Down, MouseButton::Left, 20, 20));
    ui.handle_input(&mouse(MouseEvent::Move, MouseButton::Left, 500, 500));
    ui.handle_input(&mouse(MouseEvent::Up, MouseButton::Left, 500, 500));
    assert!(!hit.get());
}

#[test]
fn clones_copy_children_and_closures_without_sharing_state() {
    let mut ui = new_ui();
    let template = add_button(&mut ui, Rect::new(1, 2, 30, 40));
    ui.node_mut(template).id = Some("TEMPLATE".into());
    let child = add_button(&mut ui, Rect::new(5, 5, 5, 5));
    ui.node_mut(ui.root).children.retain(|&c| c != child);
    ui.add_child(template, child);
    ui.node_mut(child).id = Some("CHILD".into());
    let copy = ui.clone_widget(template);
    assert_ne!(copy, template);
    assert_eq!(ui.node(copy).bounds, Rect::new(1, 2, 30, 40));
    let child_copy = ui.child(copy, "CHILD");
    assert_ne!(child_copy, child);
    assert_eq!(ui.node(child_copy).parent, Some(copy));
    ui.button(copy).text = "changed".into();
    assert_eq!(ui.button(template).text, "");
}

#[test]
fn scroll_item_setup_shows_the_copy_and_keeps_the_template_hidden() {
    let mut ui = new_ui();
    let kind = {
        let assets = ui.assets.borrow();
        cell(ScrollItemWidget::new(&assets))
    };
    let template = ui.alloc(kind.clone());
    kind.borrow_mut().initialize(&mut ui, template);
    assert!(!ui.is_visible(template));
    assert!(ui.node(template).ignore_child_mouse_over);
    let selected = Rc::new(Cell::new(false));
    let item = ScrollItemWidget::setup(&mut ui, template, Rc::new(|| true), Rc::new(move |_| selected.set(true)));
    assert!(ui.is_visible(item));
    assert!(ui.button(item).on_click.is_some());
    assert!(!ui.is_visible(template));
}

fn add_dropdown(ui: &mut Ui) -> WidgetId {
    let kind = {
        let assets = ui.assets.borrow();
        cell(DropDownButtonWidget::new(&assets))
    };
    let id = ui.alloc(kind);
    ui.node_mut(id).bounds = Rect::new(100, 100, 120, 30);
    ui.add_child(ui.root, id);
    id
}

#[test]
fn a_click_outside_closes_a_drop_down_and_calls_cancel() {
    let mut ui = new_ui();
    let button = add_dropdown(&mut ui);
    let panel = ui.alloc(container());
    ui.node_mut(panel).bounds = Rect::new(0, 0, 120, 200);
    let cancelled = Rc::new(Cell::new(false));
    let flag = cancelled.clone();
    DropDownButtonWidget::attach_panel(&mut ui, button, panel, Some(Rc::new(move |_| flag.set(true))));
    assert_eq!(ui.node(panel).bounds, Rect::new(100, 130, 120, 200));
    assert_eq!(ui.node(ui.root).children.len(), 3);
    click(&mut ui, 700, 500);
    assert!(cancelled.get());
    assert_eq!(ui.node(ui.root).children, vec![button]);
}

#[test]
fn a_drop_down_opens_upward_at_the_bottom_of_the_screen() {
    let mut ui = new_ui();
    let button = add_dropdown(&mut ui);
    ui.node_mut(button).bounds.y = 500;
    let panel = ui.alloc(container());
    ui.node_mut(panel).bounds = Rect::new(0, 0, 120, 200);
    DropDownButtonWidget::attach_panel(&mut ui, button, panel, None);
    assert_eq!(ui.node(panel).bounds.y, 300);
}

#[test]
fn removing_a_drop_down_closes_its_panel() {
    let mut ui = new_ui();
    let button = add_dropdown(&mut ui);
    let panel = ui.alloc(container());
    DropDownButtonWidget::attach_panel(&mut ui, button, panel, None);
    ui.remove_child(ui.root, button);
    assert!(ui.node(ui.root).children.is_empty());
    assert!(!ui.exists(panel));
}

fn text(ui: &Ui, id: WidgetId) -> String {
    ui.get::<TextFieldWidget>(id).text()
}

#[test]
fn text_field_inserts_at_the_cursor_and_respects_the_limit() {
    let mut ui = new_ui();
    let field = add_text_field(&mut ui);
    ui.get::<TextFieldWidget>(field).max_length = 5;
    ui.handle_text("abc");
    key(&mut ui, "LEFT", Modifiers::default());
    ui.handle_text("xyz");
    assert_eq!(text(&ui, field), "abxyc");
    assert_eq!(ui.get::<TextFieldWidget>(field).cursor_position, 4);
}

#[test]
fn text_field_edits_by_word_and_selection() {
    let mut ui = new_ui();
    let field = add_text_field(&mut ui);
    ui.handle_text("one two  three");
    let word = Modifiers {
        alt: cfg!(target_os = "macos"),
        ctrl: !cfg!(target_os = "macos"),
        ..Modifiers::default()
    };
    key(&mut ui, "BACKSPACE", word);
    assert_eq!(text(&ui, field), "one two  ");
    key(&mut ui, "BACKSPACE", word);
    assert_eq!(text(&ui, field), "one ");
    key(&mut ui, "HOME", SHIFT);
    key(&mut ui, "DELETE", Modifiers::default());
    assert_eq!(text(&ui, field), "");
}

#[test]
fn text_field_cuts_and_pastes_the_first_line() {
    let mut ui = new_ui();
    let field = add_text_field(&mut ui);
    ui.handle_text("hello");
    key(&mut ui, "A", CTRL);
    key(&mut ui, "X", CTRL);
    assert_eq!(text(&ui, field), "");
    assert_eq!(ui.clipboard.get(), "hello");
    ui.clipboard.set(" first \nsecond");
    key(&mut ui, "V", CTRL);
    assert_eq!(text(&ui, field), "first");
}

#[test]
fn integer_text_field_drops_other_characters() {
    let mut ui = new_ui();
    let field = add_text_field(&mut ui);
    ui.get::<TextFieldWidget>(field).set_type(TextFieldType::Integer);
    ui.handle_text("1a2");
    assert_eq!(text(&ui, field), "12");
}

#[test]
fn text_field_enter_callback_may_clear_its_field() {
    let mut ui = new_ui();
    let field = add_text_field(&mut ui);
    ui.handle_text("message");
    ui.get::<TextFieldWidget>(field).on_enter_key = Some(Rc::new(move |ui, _| {
        ui.get::<TextFieldWidget>(field).set_text("");
        true
    }));
    key(&mut ui, "RETURN", Modifiers::default());
    assert_eq!(text(&ui, field), "");
}

#[test]
fn text_edited_runs_after_the_field_changed() {
    let mut ui = new_ui();
    let field = add_text_field(&mut ui);
    let seen = Rc::new(RefCell::new(String::new()));
    let copy = seen.clone();
    ui.get::<TextFieldWidget>(field).on_text_edited = Some(Rc::new(move |ui| {
        *copy.borrow_mut() = ui.get::<TextFieldWidget>(field).text();
    }));
    ui.handle_text("abc");
    assert_eq!(*seen.borrow(), "abc");
}

#[test]
fn losing_keyboard_focus_calls_the_field() {
    let mut ui = new_ui();
    let field = add_text_field(&mut ui);
    let lost = Rc::new(Cell::new(false));
    let flag = lost.clone();
    ui.get::<TextFieldWidget>(field).on_lose_focus = Some(Rc::new(move |_| flag.set(true)));
    let other = add_text_field(&mut ui);
    assert!(ui.has_keyboard_focus(other));
    ui.tick();
    assert!(lost.get());
}

#[test]
fn slider_maps_pixels_and_values_both_ways() {
    let slider = SliderWidget::default();
    let rb = Rect::new(0, 0, 120, 20);
    assert_eq!(slider.value_from_px(10, rb), 0.0);
    assert_eq!(slider.value_from_px(110, rb), 1.0);
    assert_eq!(slider.px_from_value(0.5, rb), 60);
}

#[test]
fn dragging_a_slider_reports_clamped_values() {
    let mut ui = new_ui();
    let seen = Rc::new(Cell::new(-1.0));
    let copy = seen.clone();
    let mut slider = SliderWidget::default();
    slider.on_change = Some(Rc::new(move |_, v| copy.set(v)));
    let id = ui.alloc(cell(slider));
    ui.node_mut(id).bounds = Rect::new(0, 0, 120, 20);
    ui.add_child(ui.root, id);
    ui.handle_input(&mouse(MouseEvent::Down, MouseButton::Left, 60, 10));
    assert_eq!(seen.get(), 0.5);
    ui.handle_input(&mouse(MouseEvent::Move, MouseButton::Left, 500, 10));
    assert_eq!(seen.get(), 1.0);
    ui.handle_input(&mouse(MouseEvent::Up, MouseButton::Left, 500, 10));
    assert!(!ui.has_mouse_focus(id));
}

#[test]
fn tooltip_container_loads_after_the_delay_and_ignores_the_mouse() {
    let mut ui = new_ui();
    let id = ui.alloc(cell(TooltipContainerWidget::default()));
    ui.add_child(ui.root, id);
    assert!(!ui.event_bounds_contains(id, 0, 0));
    let token = TooltipContainerWidget::set_tooltip(&mut ui, id, "UNUSED", WidgetArgs::default());
    assert_eq!(token, 1);
    ui.last_move_time = ui.run_time() + 10_000;
    ui.tick();
    assert!(ui.node(id).children.is_empty());
    TooltipContainerWidget::remove_tooltip_token(&mut ui, id, 99);
    TooltipContainerWidget::remove_tooltip(&mut ui, id);
    ui.last_move_time = -10_000;
    ui.tick();
    assert!(ui.node(id).children.is_empty());
}
