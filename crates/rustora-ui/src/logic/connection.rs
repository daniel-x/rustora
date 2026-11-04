// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `ConnectionLogic.cs`. `ConnectionSwitchModLogic` is not ported: the client only runs RA.

use std::cell::RefCell;
use std::rc::Rc;

use crate::fluent::MessageArg;
use crate::loader::cell;
use crate::logic::{message, message_with, services};
use crate::net::{ConnectionState, Net};
use crate::widget::{Action, AppRequest, ChromeLogic, Ui, WidgetArgs, WidgetId};
use crate::widgets::{LabelWidget, TextFieldWidget};

/// `onRetry`.
pub type OnRetry = Rc<dyn Fn(&mut Ui, String)>;

/// Port of `ConnectionLogic.Connect`.
pub fn connect(ui: &mut Ui, target: &str, password: &str, on_connect: Action, on_abort: Action) {
    ui.requests.push(AppRequest::JoinServer {
        target: target.to_string(),
        password: password.to_string(),
    });
    let net = services(ui).net.clone();
    net.borrow_mut().connection_state = ConnectionState::Connecting;
    let retry_target = target.to_string();
    let (retry_connect, retry_abort) = (on_connect.clone(), on_abort.clone());
    let on_retry: OnRetry = Rc::new(move |ui, password| {
        connect(ui, &retry_target, &password, retry_connect.clone(), retry_abort.clone());
    });
    let args = WidgetArgs::default()
        .with_value("endpoint", target.to_string())
        .with_action("onConnect", on_connect)
        .with_action("onAbort", on_abort)
        .with_value("onRetry", on_retry);
    ui.open_window("CONNECTING_PANEL", args);
}

/// Opens the panel that reports a lost connection.
pub fn open_connection_failed(ui: &mut Ui, password: &str, on_abort: Action, on_retry: OnRetry) {
    let args = WidgetArgs::default()
        .with_value("password", password.to_string())
        .with_action("onAbort", on_abort)
        .with_value("onRetry", on_retry);
    ui.open_window("CONNECTIONFAILED_PANEL", args);
}

struct ConnectionLogic {
    net: Net,
    on_connect: Action,
    on_abort: Action,
    on_retry: OnRetry,
    done: bool,
}

impl ChromeLogic for ConnectionLogic {
    fn tick(&mut self, ui: &mut Ui) {
        if self.done {
            return;
        }
        let (state, password) = {
            let net = self.net.borrow();
            (net.connection_state, net.password.clone())
        };
        if state == ConnectionState::Connecting {
            return;
        }
        self.done = true;
        let (on_connect, on_abort, on_retry) = (self.on_connect.clone(), self.on_abort.clone(), self.on_retry.clone());
        ui.run_after_tick(move |ui| {
            ui.close_window();
            if state == ConnectionState::Connected {
                on_connect(ui);
            } else {
                open_connection_failed(ui, &password, on_abort.clone(), on_retry.clone());
            }
        });
    }
}

pub fn create_connection(ui: &mut Ui, widget: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let on_abort = args.action("onAbort");
    let abort = ui.child(widget, "ABORT_BUTTON");
    let abort_action = on_abort.clone();
    ui.button(abort).on_click = Some(Rc::new(move |ui| {
        ui.requests.push(AppRequest::Disconnect);
        ui.close_window();
        abort_action(ui);
    }));
    let endpoint: String = args.value("endpoint").unwrap_or_default();
    let text = message_with(ui, "label-connecting-to-endpoint", &[("endpoint", MessageArg::Text(&endpoint))]);
    let label = ui.child(widget, "CONNECTING_DESC");
    ui.get::<LabelWidget>(label).get_text = Some(Rc::new(move || text.clone()));
    cell(ConnectionLogic {
        net: services(ui).net.clone(),
        on_connect: args.action("onConnect"),
        on_abort,
        on_retry: args
            .value::<OnRetry>("onRetry")
            .unwrap_or_else(|| Rc::new(|_: &mut Ui, _: String| {})),
        done: false,
    })
}

/// Port of `ConnectionFailedLogic`.
struct ConnectionFailedLogic {
    widget: WidgetId,
    net: Net,
    password_field: Option<WidgetId>,
    connection_error: WidgetId,
    abort: WidgetId,
    retry: WidgetId,
    offset_adjusted: bool,
}

impl ChromeLogic for ConnectionFailedLogic {
    fn tick(&mut self, ui: &mut Ui) {
        let Some(field) = self.password_field else {
            return;
        };
        if self.offset_adjusted || !self.net.borrow().authentication_failed {
            return;
        }
        let offset = ui.node(field).bounds.y - ui.node(self.connection_error).bounds.y;
        ui.node_mut(self.abort).bounds.y += offset;
        ui.node_mut(self.retry).bounds.y += offset;
        let panel = &mut ui.node_mut(self.widget).bounds;
        panel.h += offset;
        panel.y -= offset / 2;
        if let Some(background) = ui.find(self.widget, "CONNECTION_BACKGROUND") {
            ui.node_mut(background).bounds.h += offset;
        }
        self.offset_adjusted = true;
    }
}

pub fn create_connection_failed(ui: &mut Ui, widget: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let net = services(ui).net.clone();
    let on_abort = args.action("onAbort");
    let on_retry: Option<OnRetry> = args.value("onRetry");
    let password: String = args.value("password").unwrap_or_default();

    let abort = ui.child(widget, "ABORT_BUTTON");
    let quit = ui.child(widget, "QUIT_BUTTON");
    let retry = ui.child(widget, "RETRY_BUTTON");
    ui.node_mut(quit).visible = false;
    ui.node_mut(retry).visible = on_retry.is_some();
    let abort_click: Action = Rc::new(move |ui| {
        ui.close_window();
        on_abort(ui);
    });
    ui.button(abort).on_click = Some(abort_click.clone());

    let password_field = ui.find(widget, "PASSWORD");
    let retry_click: Action = {
        let net = net.clone();
        Rc::new(move |ui| {
            let typed = password_field
                .filter(|_| net.borrow().authentication_failed)
                .map(|f| ui.get::<TextFieldWidget>(f).text());
            let pass = typed.unwrap_or_else(|| password.clone());
            ui.close_window();
            if let Some(on_retry) = &on_retry {
                on_retry(ui, pass);
            }
        })
    };
    ui.button(retry).on_click = Some(retry_click.clone());

    let (target, error, title) = {
        let n = net.borrow();
        let error = match (&n.server_error, &n.error_message) {
            (Some(e), _) => message(ui, e),
            (None, Some(e)) => e.clone(),
            (None, None) => message(ui, "label-unknown-error"),
        };
        let title = if n.authentication_failed {
            "label-password-required"
        } else {
            "label-connection-failed"
        };
        (n.target.clone(), error, message(ui, title))
    };
    let desc_text = message_with(ui, "label-could-not-connect-to-target", &[("target", MessageArg::Text(&target))]);
    let desc = ui.child(widget, "CONNECTING_DESC");
    ui.get::<LabelWidget>(desc).get_text = Some(Rc::new(move || desc_text.clone()));
    let connection_error = ui.child(widget, "CONNECTION_ERROR");
    ui.get::<LabelWidget>(connection_error).get_text = Some(Rc::new(move || error.clone()));
    let panel_title = ui.child(widget, "TITLE");
    ui.get::<LabelWidget>(panel_title).get_text = Some(Rc::new(move || title.clone()));

    if let Some(field) = password_field {
        ui.take_keyboard_focus(field);
        let n = net.clone();
        ui.set_visible_fn(field, move || n.borrow().authentication_failed);
        let password_label = ui.child(widget, "PASSWORD_LABEL");
        let n = net.clone();
        ui.set_visible_fn(password_label, move || n.borrow().authentication_failed);
        let mut text_field = ui.get::<TextFieldWidget>(field);
        text_field.on_enter_key = Some(Rc::new(move |ui, _| {
            retry_click(ui);
            true
        }));
        text_field.on_esc_key = Some(Rc::new(move |ui, _| {
            abort_click(ui);
            true
        }));
    }

    cell(ConnectionFailedLogic {
        widget,
        net,
        password_field,
        connection_error,
        abort,
        retry,
        offset_adjusted: false,
    })
}
