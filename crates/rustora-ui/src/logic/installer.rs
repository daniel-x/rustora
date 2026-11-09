// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `ModContentLogic.cs`, `ModContentPromptLogic.cs`, `ModContentSourceTooltipLogic.cs`,
//! `DownloadPackageLogic.cs` and `InstallFromSourceLogic.cs`. Worker threads do the slow parts and send events that the
//! logic classes read in `tick`, because Fluent messages and widgets exist only on the user interface thread.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

use rustora_fs::Platform;
use rustora_client::installer::{
    Availability, ModContent, ModDownload, ModSource, StatusArg, StatusMessage, extract_download, find_source_path, install_from_source,
    package_tooltip, sha1_file, source_availability, write_install_log,
};
use rustora_net::web;

use crate::fluent::MessageArg;
use crate::loader::cell;
use crate::logic::{cosmetic_random, message};
use crate::widget::{Action, AppRequest, ChromeLogic, Getter, Ui, WidgetArgs, WidgetId};
use crate::widgets::{CheckboxWidget, ImageWidget, LabelWidget, ProgressBarWidget};

const SIZE_SUFFIXES: [&str; 4] = ["bytes", "KB", "MB", "GB"];

/// The arguments that every installer panel shares.
pub struct InstallerContext {
    pub content: Arc<ModContent>,
    pub platform: Platform,
}

fn status_text(ui: &Ui, status: &StatusMessage) -> String {
    let args: Vec<(&str, MessageArg)> = status
        .args
        .iter()
        .map(|(name, value)| {
            let value = match value {
                StatusArg::Text(t) => MessageArg::Text(t),
                StatusArg::Number(n) => MessageArg::Number(*n),
            };
            (*name, value)
        })
        .collect();
    ui.assets.borrow().messages.get_with(status.key, &args)
}

fn context(args: &WidgetArgs) -> Rc<InstallerContext> {
    args.value("content").expect("the installer panels receive the content")
}

fn text_getter(text: &Rc<RefCell<String>>) -> Getter<String> {
    let text = text.clone();
    Rc::new(move || text.borrow().clone())
}

fn request_initialize_mod(target: &str) -> Action {
    let target = target.to_string();
    Rc::new(move |ui| ui.requests.push(AppRequest::InitializeMod(target.clone())))
}

/// Port of `ModContentLogic`.
pub fn create_mod_content(ui: &mut Ui, _widget: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let ctx = context(args);
    let reload = request_initialize_mod(&ctx.content.target_mod);
    let panel_args = WidgetArgs::default().with_value("content", ctx.clone());
    if ctx.content.required_content_installed(&ctx.platform) {
        ui.open_window("CONTENT_PANEL", panel_args.with_action("onCancel", reload));
    } else {
        ui.open_window("CONTENT_PROMPT_PANEL", panel_args.with_action("continueLoading", reload));
    }
    cell(crate::logic::NoLogic)
}

/// The shared part of the constructors of the prompt and the package list: fits the header and makes room for it.
fn fit_header(ui: &mut Ui, panel: WidgetId) -> i32 {
    let header = ui.child(panel, "HEADER_LABEL");
    LabelWidget::increase_height_to_fit_current_text(ui, header);
    let height = ui.node(header).bounds.h;
    let bounds = &mut ui.node_mut(panel).bounds;
    bounds.h += height;
    bounds.y -= height / 2;
    height
}

/// Port of `ModContentPromptLogic`.
pub fn create_prompt(ui: &mut Ui, widget: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let ctx = context(args);
    let continue_loading = args.action("continueLoading");
    let installed = Rc::new(Cell::new(ctx.content.required_content_installed(&ctx.platform)));
    let header_height = fit_header(ui, widget);

    let advanced = ui.child(widget, "ADVANCED_BUTTON");
    ui.node_mut(advanced).bounds.y += header_height;
    let (advanced_ctx, advanced_installed) = (ctx.clone(), installed.clone());
    ui.button(advanced).on_click = Some(Rc::new(move |ui| {
        let (ctx, installed) = (advanced_ctx.clone(), advanced_installed.clone());
        let recheck: Action = Rc::new(move |_| installed.set(ctx.content.required_content_installed(&ctx.platform)));
        let panel_args = WidgetArgs::default()
            .with_value("content", advanced_ctx.clone())
            .with_action("onCancel", recheck);
        ui.open_window("CONTENT_PANEL", panel_args);
    }));

    let quick = ui.child(widget, "QUICK_BUTTON");
    let quick_download = ctx.content.quick_download.clone();
    let has_quick = quick_download.is_some();
    ui.set_visible_fn(quick, move || has_quick);
    ui.node_mut(quick).bounds.y += header_height;
    let (quick_ctx, quick_continue) = (ctx.clone(), continue_loading.clone());
    ui.button(quick).on_click = Some(Rc::new(move |ui| {
        let key = quick_download.as_deref().unwrap_or_default();
        let download = quick_ctx
            .content
            .download(key)
            .unwrap_or_else(|| panic!("Mod QuickDownload `{key}` definition not found."))
            .clone();
        let panel_args = WidgetArgs::default()
            .with_value("content", quick_ctx.clone())
            .with_value("download", download)
            .with_action("onSuccess", quick_continue.clone());
        ui.open_window("PACKAGE_DOWNLOAD_PANEL", panel_args);
    }));

    let quit = ui.child(widget, "QUIT_BUTTON");
    ui.node_mut(quit).bounds.y += header_height;
    let continue_text = message(ui, "button-continue");
    let quit_text = message(ui, "button-quit");
    let text_installed = installed.clone();
    ui.button(quit).get_text = Some(Rc::new(move || {
        if text_installed.get() {
            continue_text.clone()
        } else {
            quit_text.clone()
        }
    }));
    ui.button(quit).on_click = Some(Rc::new(move |ui| {
        if installed.get() {
            continue_loading(ui);
        } else {
            ui.requests.push(AppRequest::Quit);
        }
    }));
    ui.run_after_tick(|ui| ui.reset_tooltips());
    cell(crate::logic::NoLogic)
}

/// Port of `ModContentInstallerLogic`.
struct PackageListLogic {
    ctx: Rc<InstallerContext>,
    scroll_panel: WidgetId,
    template: WidgetId,
    source_available: Rc<Cell<bool>>,
}

impl PackageListLogic {
    fn populate(&self, ui: &mut Ui) {
        ui.remove_children(self.scroll_panel);
        for package in &self.ctx.content.packages {
            let container = ui.clone_widget(self.template);
            let title = message(ui, &package.title);
            let title_widget = ui.child(container, "TITLE");
            ui.label(title_widget).get_text = Some(Rc::new(move || title.clone()));

            let required_widget = ui.child(container, "REQUIRED");
            let required = package.required;
            ui.set_visible_fn(required_widget, move || required);

            let mut seen = HashSet::new();
            let titles: Vec<String> = package
                .sources
                .iter()
                .filter_map(|s| self.ctx.content.source(s))
                .map(|s| s.title.clone())
                .filter(|t| seen.insert(t.clone()))
                .collect();
            let source_list = titles.join("\n");
            let has_source = !source_list.is_empty();
            let source_widget = ui.child(container, "SOURCE");
            ui.get::<ImageWidget>(source_widget).get_tooltip_text = Some(Rc::new(move || source_list.clone()));
            ui.set_visible_fn(source_widget, move || has_source);

            let installed = package.is_installed(&self.ctx.platform);
            let download = package.download.as_deref().and_then(|d| self.ctx.content.download(d)).cloned();
            let download_enabled = !installed && download.is_some();
            let download_button = ui.child(container, "DOWNLOAD");
            ui.set_visible_fn(download_button, move || download_enabled);
            if let Some(download) = download.filter(|_| download_enabled) {
                let ctx = self.ctx.clone();
                ui.button(download_button).on_click = Some(Rc::new(move |ui| {
                    let panel_args = WidgetArgs::default()
                        .with_value("content", ctx.clone())
                        .with_value("download", download.clone());
                    ui.open_window("PACKAGE_DOWNLOAD_PANEL", panel_args);
                }));
            }

            let installed_widget = ui.child(container, "INSTALLED");
            ui.set_visible_fn(installed_widget, move || installed);
            let requires_source = ui.child(container, "REQUIRES_SOURCE");
            ui.set_visible_fn(requires_source, move || !installed && !download_enabled);
            if !has_source {
                let manual = message(ui, "button-manual-install");
                ui.label(requires_source).get_text = Some(Rc::new(move || manual.clone()));
            }
            ui.add_child(self.scroll_panel, container);
        }
        self.source_available.set(
            self.ctx
                .content
                .packages
                .iter()
                .any(|p| !p.sources.is_empty() && !p.is_installed(&self.ctx.platform)),
        );
    }
}

impl ChromeLogic for PackageListLogic {
    fn became_visible(&mut self, ui: &mut Ui) {
        self.populate(ui);
    }
}

pub fn create_package_list(ui: &mut Ui, widget: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let ctx = context(args);
    let on_cancel = args.action("onCancel");
    let scroll_panel = ui.child(widget, "PACKAGES");
    let template = ui.child(scroll_panel, "PACKAGE_TEMPLATE");
    ui.hide_child(scroll_panel, template);
    let header_height = fit_header(ui, widget);
    ui.node_mut(scroll_panel).bounds.y += header_height;

    let source_available = Rc::new(Cell::new(false));
    let source_button = ui.child(widget, "CHECK_SOURCE_BUTTON");
    ui.node_mut(source_button).bounds.y += header_height;
    let visible = source_available.clone();
    ui.set_visible_fn(source_button, move || visible.get());
    let source_ctx = ctx.clone();
    ui.button(source_button).on_click = Some(Rc::new(move |ui| {
        ui.open_window(
            "SOURCE_INSTALL_PANEL",
            WidgetArgs::default().with_value("content", source_ctx.clone()),
        );
    }));

    let back = ui.child(widget, "BACK_BUTTON");
    ui.node_mut(back).bounds.y += header_height;
    ui.button(back).on_click = Some(Rc::new(move |ui| {
        ui.close_window();
        on_cancel(ui);
    }));

    let logic = PackageListLogic {
        ctx,
        scroll_panel,
        template,
        source_available,
    };
    logic.populate(ui);
    ui.run_after_tick(|ui| ui.reset_tooltips());
    cell(logic)
}

/// Port of `ModContentSourceTooltipLogic`.
pub fn create_source_tooltip(ui: &mut Ui, widget: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let sources = ui.child(widget, "SOURCES");
    let template = ui.child(sources, "SOURCE_TEMPLATE");
    ui.hide_child(sources, template);
    let description = ui.child(widget, "DESCRIPTION");
    let font = ui.label(template).font.clone();
    let text = args.value::<Getter<String>>("getText").map_or_else(String::new, |f| f());
    let description_text = {
        let key = ui.label(description).text.clone().unwrap_or_default();
        message(ui, &key)
    };
    let description_font = ui.label(description).font.clone();
    let mut max_width = ui.measure(&description_font, &description_text).0;
    let side_margin = ui.node(description).bounds.x;
    let bottom_margin = ui.node(sources).bounds.h;
    for line in text.split('\n') {
        let label = ui.clone_widget(template);
        let title = line.to_string();
        ui.label(label).get_text = Some(Rc::new(move || title.clone()));
        let height = ui.node(sources).bounds.h;
        let width = ui.measure(&font, line).0;
        let bounds = &mut ui.node_mut(label).bounds;
        bounds.y = height;
        bounds.w = width;
        max_width = max_width.max(width + bounds.x);
        let label_height = bounds.h;
        ui.add_child(sources, label);
        ui.node_mut(sources).bounds.h += label_height;
    }
    let (sources_y, sources_h) = {
        let bounds = ui.node(sources).bounds;
        (bounds.y, bounds.h)
    };
    let bounds = &mut ui.node_mut(widget).bounds;
    bounds.w = 2 * side_margin + max_width;
    bounds.h = sources_y + bottom_margin + sources_h;
    cell(crate::logic::NoLogic)
}

enum DownloadEvent {
    Host(String),
    Status(StatusMessage),
    Progress(i32),
    Indeterminate(bool),
    Error(StatusMessage),
    Success,
}

/// The removal of the temporary file on every way out of the download.
struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    host.split(':').next().unwrap_or(host).to_string()
}

fn download_status(host: &str, read: u64, total: Option<u64>) -> StatusMessage {
    let magnitude = |value: u64| (((value.max(1) as f64).log(1024.0)) as usize).min(SIZE_SUFFIXES.len() - 1);
    let scaled = |value: u64, mag: usize| format!("{:.2}", value as f32 / (1u64 << (mag * 10)) as f32);
    match total {
        Some(total) if total > 0 => {
            let mag = magnitude(total);
            StatusMessage::new("label-downloading-from-progress")
                .text("host", host)
                .text("received", scaled(read, mag))
                .text("total", scaled(total, mag))
                .text("suffix", SIZE_SUFFIXES[mag])
                .number("progress", (100 * read / total) as i64)
        }
        _ => {
            let mag = magnitude(read);
            StatusMessage::new("label-downloading-from")
                .text("host", host)
                .text("received", scaled(read, mag))
                .text("suffix", SIZE_SUFFIXES[mag])
        }
    }
}

fn run_download(download: &ModDownload, platform: &Platform, random: usize, cancelled: &AtomicBool, events: &Sender<DownloadEvent>) {
    let send = |event| {
        let _ = events.send(event);
    };
    let url = match &download.mirror_list {
        Some(list) => {
            send(DownloadEvent::Status(StatusMessage::new("label-fetching-mirror-list")));
            let mirrors = web::get(list).unwrap_or_default();
            let mirrors: Vec<&str> = mirrors.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
            if mirrors.is_empty() {
                send(DownloadEvent::Error(StatusMessage::new("label-mirror-selection-failed")));
                return;
            }
            mirrors[random % mirrors.len()].to_string()
        }
        None => download.url.clone().unwrap_or_default(),
    };
    let host = host_of(&url);
    send(DownloadEvent::Host(host.clone()));
    send(DownloadEvent::Status(download_status(&host, 0, None)));

    let temp = TempFile(std::env::temp_dir().join(format!("rustora-download-{}-{random}.tmp", std::process::id())));
    let result = File::create(&temp.0)
        .map_err(|e| web::DownloadError::Failed(e.to_string()))
        .and_then(|mut file| {
            web::download(&url, &mut file, cancelled, &|read, total| {
                if let Some(total) = total.filter(|t| *t > 0) {
                    send(DownloadEvent::Progress((100 * read / total) as i32));
                    send(DownloadEvent::Indeterminate(false));
                }
                send(DownloadEvent::Status(download_status(&host, read, total)));
            })
        });
    match result {
        Ok(()) => {}
        Err(web::DownloadError::Cancelled) => return,
        Err(e) => {
            write_install_log(platform, &format!("Download from {url} failed: {e:?}"));
            send(DownloadEvent::Error(StatusMessage::new("label-download-failed")));
            return;
        }
    }

    if let Some(expected) = &download.sha1 {
        send(DownloadEvent::Indeterminate(true));
        send(DownloadEvent::Status(StatusMessage::new("label-verifying-archive")));
        if !sha1_file(&temp.0).is_ok_and(|hash| hash.eq_ignore_ascii_case(expected)) {
            send(DownloadEvent::Error(StatusMessage::new("label-archive-validation-failed")));
            return;
        }
    }

    send(DownloadEvent::Indeterminate(true));
    send(DownloadEvent::Status(StatusMessage::new("label-extracting-archive")));
    let on_entry = |entry: &str| {
        send(DownloadEvent::Status(
            StatusMessage::new("label-extracting-archive-entry").text("entry", entry),
        ));
    };
    match extract_download(download, &temp.0, platform, &on_entry) {
        Ok(()) => send(DownloadEvent::Success),
        Err(_) => send(DownloadEvent::Error(StatusMessage::new("label-archive-extraction-failed"))),
    }
}

/// Port of `DownloadPackageLogic`.
struct DownloadLogic {
    ctx: Rc<InstallerContext>,
    download: ModDownload,
    on_success: Action,
    status_label: WidgetId,
    retry_visible: Rc<Cell<bool>>,
    status: Rc<RefCell<String>>,
    percentage: Rc<Cell<i32>>,
    indeterminate: Rc<Cell<bool>>,
    host: Option<String>,
    cancelled: Arc<AtomicBool>,
    events: Option<Receiver<DownloadEvent>>,
}

impl DownloadLogic {
    fn start(&mut self) {
        self.percentage.set(0);
        self.indeterminate.set(true);
        self.host = None;
        self.retry_visible.set(false);
        let (sender, receiver) = channel();
        self.events = Some(receiver);
        self.cancelled = Arc::new(AtomicBool::new(false));
        let (download, platform, cancelled) = (self.download.clone(), self.ctx.platform.clone(), self.cancelled.clone());
        let random = cosmetic_random(usize::MAX);
        std::thread::spawn(move || run_download(&download, &platform, random, &cancelled, &sender));
    }

    fn set_status(&self, ui: &mut Ui, text: &str) {
        let font = ui.label(self.status_label).font.clone();
        let width = ui.node(self.status_label).bounds.w;
        *self.status.borrow_mut() = ui.truncate_text(&font, text, width);
    }
}

/// A one shot request of the retry button, which runs on the next tick where the logic can restart the worker.
#[derive(Clone, Default)]
struct RetryRequest(Rc<Cell<bool>>);

struct DownloadLogicWithRetry {
    logic: DownloadLogic,
    retry_request: RetryRequest,
}

impl ChromeLogic for DownloadLogicWithRetry {
    fn tick(&mut self, ui: &mut Ui) {
        let logic = &mut self.logic;
        if self.retry_request.0.replace(false) {
            logic.start();
        }
        let events: Vec<DownloadEvent> = logic.events.as_ref().map_or_else(Vec::new, |r| r.try_iter().collect());
        for event in events {
            match event {
                DownloadEvent::Host(host) => logic.host = Some(host),
                DownloadEvent::Status(status) => {
                    let text = status_text(ui, &status);
                    logic.set_status(ui, &text);
                }
                DownloadEvent::Progress(percentage) => logic.percentage.set(percentage),
                DownloadEvent::Indeterminate(value) => logic.indeterminate.set(value),
                DownloadEvent::Error(status) => {
                    let host = logic.host.clone().unwrap_or_else(|| message(ui, "label-unknown-host"));
                    let text = format!("{host}: Error: {}", status_text(ui, &status));
                    logic.indeterminate.set(false);
                    logic.percentage.set(100);
                    logic.set_status(ui, &text);
                    logic.retry_visible.set(true);
                    logic.events = None;
                }
                DownloadEvent::Success => {
                    logic.events = None;
                    let on_success = logic.on_success.clone();
                    ui.run_after_tick(move |ui| {
                        ui.close_window();
                        on_success(ui);
                    });
                    return;
                }
            }
        }
    }

    fn disposed(&mut self, _ui: &mut Ui) {
        self.logic.cancelled.store(true, Ordering::Relaxed);
    }
}

pub fn create_download(ui: &mut Ui, widget: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let ctx = context(args);
    let download: ModDownload = args.value("download").expect("the download panel receives a download");
    let title_label = ui.child(widget, "TITLE");
    let title = ui
        .assets
        .borrow()
        .messages
        .get_with("label-downloading", &[("title", MessageArg::Text(&download.title))]);
    ui.label(title_label).get_text = Some(Rc::new(move || title.clone()));

    let percentage = Rc::new(Cell::new(0));
    let indeterminate = Rc::new(Cell::new(true));
    let bar = ui.child(widget, "PROGRESS_BAR");
    {
        let (percentage, indeterminate) = (percentage.clone(), indeterminate.clone());
        let mut bar = ui.get::<ProgressBarWidget>(bar);
        bar.get_percentage = Some(Rc::new(move || percentage.get()));
        bar.is_indeterminate = Some(Rc::new(move || indeterminate.get()));
    }
    let status = Rc::new(RefCell::new(String::new()));
    let status_label = ui.child(widget, "STATUS_LABEL");
    ui.label(status_label).get_text = Some(text_getter(&status));

    let retry_visible = Rc::new(Cell::new(false));
    let retry = ui.child(widget, "RETRY_BUTTON");
    let visible = retry_visible.clone();
    ui.set_visible_fn(retry, move || visible.get());
    let retry_request = RetryRequest::default();
    let request = retry_request.clone();
    ui.button(retry).on_click = Some(Rc::new(move |_| request.0.set(true)));

    let cancel = ui.child(widget, "CANCEL_BUTTON");
    ui.button(cancel).on_click = Some(Rc::new(|ui| ui.close_window()));

    let mut logic = DownloadLogic {
        ctx,
        download,
        on_success: args.action("onSuccess"),
        status_label,
        retry_visible,
        status,
        percentage,
        indeterminate,
        host: None,
        cancelled: Arc::new(AtomicBool::new(false)),
        events: None,
    };
    logic.start();
    cell(DownloadLogicWithRetry { logic, retry_request })
}

enum InstallEvent {
    Status(StatusMessage),
    Found {
        path: String,
        source: usize,
        packages: Vec<usize>,
    },
    NotFound {
        game_sources: Vec<String>,
        digital_installs: Vec<String>,
    },
    Installed,
    Failed,
}

fn detect_sources(content: &ModContent, platform: &Platform, events: &Sender<InstallEvent>) {
    for (index, source) in content.sources.iter().enumerate() {
        let _ = events.send(InstallEvent::Status(
            StatusMessage::new("label-searching-source-for").text("title", source.title.clone()),
        ));
        let Some(path) = find_source_path(source) else {
            continue;
        };
        write_install_log(
            platform,
            &format!(
                "Using installer `{}: {}` of type `{}`:",
                source.key, source.title, source.source_type
            ),
        );
        let packages: Vec<usize> = content
            .packages
            .iter()
            .enumerate()
            .filter(|(_, p)| p.sources.contains(&source.key) && !p.is_installed(platform))
            .map(|(i, _)| i)
            .collect();
        if !packages.is_empty() {
            let _ = events.send(InstallEvent::Found {
                path,
                source: index,
                packages,
            });
            return;
        }
    }
    let mut game_sources = Vec::new();
    let mut digital_installs = Vec::new();
    let missing = content
        .packages
        .iter()
        .filter(|p| !p.is_installed(platform))
        .flat_map(|p| &p.sources);
    for source in missing.filter_map(|key| content.source(key)) {
        let group = match source_availability(source) {
            Availability::GameSource => &mut game_sources,
            Availability::DigitalInstall => &mut digital_installs,
            Availability::Unavailable => continue,
        };
        if !group.contains(&source.title) {
            group.push(source.title.clone());
        }
    }
    let _ = events.send(InstallEvent::NotFound {
        game_sources,
        digital_installs,
    });
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Progress,
    Message,
    List,
}

/// The state of `InstallFromSourceLogic`, shared with the button actions.
struct InstallState {
    ctx: Rc<InstallerContext>,
    panel: WidgetId,
    primary: WidgetId,
    primary_visible: Rc<Cell<bool>>,
    secondary: WidgetId,
    progress_container: WidgetId,
    progress_label: WidgetId,
    message_container: WidgetId,
    list_container: WidgetId,
    list_panel: WidgetId,
    list_header_template: WidgetId,
    label_list_template: WidgetId,
    checkbox_list_template: WidgetId,
    mode: Rc<Cell<Mode>>,
    title: Rc<RefCell<String>>,
    progress_text: Rc<RefCell<String>>,
    message_text: Rc<RefCell<String>>,
    list_text: Rc<RefCell<String>>,
    events: RefCell<Option<Receiver<InstallEvent>>>,
    selected: Rc<RefCell<Vec<(String, bool)>>>,
    retry_install: RefCell<Option<(String, usize)>>,
}

impl InstallState {
    fn set_mode(&self, ui: &mut Ui, mode: Mode, container: WidgetId) {
        self.mode.set(mode);
        let delta = ui.node(container).bounds.h - ui.node(self.panel).bounds.h;
        ui.node_mut(self.primary).bounds.y += delta;
        ui.node_mut(self.secondary).bounds.y += delta;
        let bounds = &mut ui.node_mut(self.panel).bounds;
        bounds.y -= delta / 2;
        bounds.h += delta;
    }

    fn set_buttons(self: &Rc<Self>, ui: &mut Ui, primary_key: &str, on_primary: Action, secondary_key: &str) {
        let primary_text = message(ui, primary_key);
        ui.button(self.primary).get_text = Some(Rc::new(move || primary_text.clone()));
        ui.button(self.primary).on_click = Some(on_primary);
        self.primary_visible.set(true);
        let secondary_text = message(ui, secondary_key);
        ui.button(self.secondary).get_text = Some(Rc::new(move || secondary_text.clone()));
        ui.button(self.secondary).on_click = Some(Rc::new(|ui| ui.close_window()));
        ui.button(self.secondary).disabled = false;
        ui.run_after_tick(|ui| ui.reset_tooltips());
    }

    fn show_progress(&self, ui: &mut Ui, title_key: &str) {
        *self.title.borrow_mut() = message(ui, title_key);
        self.set_mode(ui, Mode::Progress, self.progress_container);
    }

    fn detect(self: &Rc<Self>, ui: &mut Ui) {
        self.show_progress(ui, "label-checking-sources");
        *self.progress_text.borrow_mut() = message(ui, "label-detecting-sources");
        let me = self.clone();
        self.set_buttons(ui, "button-retry", Rc::new(move |ui| me.detect(ui)), "button-back");
        let (sender, receiver) = channel();
        *self.events.borrow_mut() = Some(receiver);
        let (content, platform) = (self.ctx.content.clone(), self.ctx.platform.clone());
        std::thread::spawn(move || detect_sources(&content, &platform, &sender));
    }

    fn install(self: &Rc<Self>, ui: &mut Ui, path: String, source: usize) {
        self.show_progress(ui, "label-installing-content");
        self.progress_text.borrow_mut().clear();
        self.primary_visible.set(false);
        ui.button(self.secondary).disabled = true;
        ui.run_after_tick(|ui| ui.reset_tooltips());
        let selected: Vec<String> = self
            .selected
            .borrow()
            .iter()
            .filter(|(_, on)| *on)
            .map(|(k, _)| k.clone())
            .collect();
        let (sender, receiver) = channel();
        *self.events.borrow_mut() = Some(receiver);
        let (content, platform) = (self.ctx.content.clone(), self.ctx.platform.clone());
        let retry_path = path.clone();
        std::thread::spawn(move || {
            let on_message = |status: StatusMessage| {
                let _ = sender.send(InstallEvent::Status(status));
            };
            let source: &ModSource = &content.sources[source];
            let result = install_from_source(source, Path::new(&path), &selected, &platform, &on_message);
            let _ = sender.send(if result.is_ok() {
                InstallEvent::Installed
            } else {
                InstallEvent::Failed
            });
        });
        self.retry_install.replace(Some((retry_path, source)));
    }

    fn show_packages(self: &Rc<Self>, ui: &mut Ui, path: String, source_index: usize, packages: &[usize]) {
        let content = self.ctx.content.clone();
        let source = &content.sources[source_index];
        *self.selected.borrow_mut() = packages
            .iter()
            .map(|&i| (content.packages[i].identifier.clone(), content.packages[i].required))
            .collect();
        *self.title.borrow_mut() = source.title.clone();
        *self.list_text.borrow_mut() = message(ui, "label-content-package-installation");
        ui.remove_children(self.list_panel);
        for &index in packages {
            let package = &content.packages[index];
            let container = ui.clone_widget(self.checkbox_list_template);
            let checkbox = ui.child(container, "PACKAGE_CHECKBOX");
            let title = message(ui, &package.title);
            let required = package.required;
            let identifier = package.identifier.clone();
            let (checked_ids, clicked_ids) = (self.selected.clone(), self.selected.clone());
            let clicked_id = identifier.clone();
            {
                let mut widget = ui.get::<CheckboxWidget>(checkbox);
                widget.button.get_text = Some(Rc::new(move || title.clone()));
                widget.button.is_disabled = Some(Rc::new(move || required));
                widget.is_checked = Some(Rc::new(move || checked_ids.borrow().iter().any(|(k, on)| *k == identifier && *on)));
                widget.button.on_click = Some(Rc::new(move |_| {
                    if let Some(entry) = clicked_ids.borrow_mut().iter_mut().find(|(k, _)| *k == clicked_id) {
                        entry.1 = !entry.1;
                    }
                }));
            }
            let tooltip = package_tooltip(source, &package.identifier).filter(|t| !t.trim().is_empty());
            let icon = ui.child(container, "PACKAGE_INFO");
            let has_tooltip = tooltip.is_some();
            ui.get::<ImageWidget>(icon).get_tooltip_text = Some(Rc::new(move || tooltip.clone().unwrap_or_default()));
            ui.set_visible_fn(icon, move || has_tooltip);
            ui.add_child(self.list_panel, container);
        }
        self.set_mode(ui, Mode::List, self.list_container);
        let me = self.clone();
        self.set_buttons(
            ui,
            "button-continue",
            Rc::new(move |ui| me.install(ui, path.clone(), source_index)),
            "button-cancel",
        );
    }

    fn show_not_found(self: &Rc<Self>, ui: &mut Ui, game_sources: &[String], digital_installs: &[String]) {
        *self.title.borrow_mut() = message(ui, "label-game-content-not-found");
        *self.list_text.borrow_mut() = message(ui, "label-alternative-content-sources");
        ui.remove_children(self.list_panel);
        for (key, group) in [("label-game-sources", game_sources), ("label-digital-installs", digital_installs)] {
            if group.is_empty() {
                continue;
            }
            let header = ui.clone_widget(self.list_header_template);
            let group_title = message(ui, key);
            let header_label = ui.child(header, "LABEL");
            ui.label(header_label).get_text = Some(Rc::new(move || group_title.clone()));
            ui.add_child(self.list_panel, header);
            for item in group {
                let label = ui.clone_widget(self.label_list_template);
                let item = item.clone();
                ui.label(label).get_text = Some(Rc::new(move || item.clone()));
                ui.add_child(self.list_panel, label);
            }
        }
        self.set_mode(ui, Mode::List, self.list_container);
        let me = self.clone();
        self.set_buttons(ui, "button-retry", Rc::new(move |ui| me.detect(ui)), "button-back");
    }

    fn show_failure(self: &Rc<Self>, ui: &mut Ui) {
        *self.title.borrow_mut() = message(ui, "label-installation-failed");
        *self.message_text.borrow_mut() = message(ui, "label-check-install-log");
        self.set_mode(ui, Mode::Message, self.message_container);
        let me = self.clone();
        let retry = self.retry_install.borrow().clone();
        self.set_buttons(
            ui,
            "button-retry",
            Rc::new(move |ui| {
                if let Some((path, source)) = retry.clone() {
                    me.install(ui, path, source);
                }
            }),
            "button-back",
        );
    }
}

struct InstallLogic {
    state: Rc<InstallState>,
}

impl ChromeLogic for InstallLogic {
    fn tick(&mut self, ui: &mut Ui) {
        let state = &self.state;
        let events: Vec<InstallEvent> = state.events.borrow().as_ref().map_or_else(Vec::new, |r| r.try_iter().collect());
        for event in events {
            match event {
                InstallEvent::Status(status) => {
                    let text = status_text(ui, &status);
                    let font = ui.label(state.progress_label).font.clone();
                    let width = ui.node(state.progress_label).bounds.w;
                    *state.progress_text.borrow_mut() = ui.truncate_text(&font, &text, width);
                }
                InstallEvent::Found { path, source, packages } => state.show_packages(ui, path, source, &packages),
                InstallEvent::NotFound {
                    game_sources,
                    digital_installs,
                } => state.show_not_found(ui, &game_sources, &digital_installs),
                InstallEvent::Installed => {
                    ui.run_after_tick(|ui| ui.close_window());
                    return;
                }
                InstallEvent::Failed => state.show_failure(ui),
            }
        }
    }
}

pub fn create_install_from_source(ui: &mut Ui, widget: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let ctx = context(args);
    let mode = Rc::new(Cell::new(Mode::Progress));
    let title = Rc::new(RefCell::new(String::new()));
    let progress_text = Rc::new(RefCell::new(String::new()));
    let message_text = Rc::new(RefCell::new(String::new()));
    let list_text = Rc::new(RefCell::new(String::new()));

    let title_label = ui.child(widget, "TITLE");
    ui.label(title_label).get_text = Some(text_getter(&title));

    let in_mode = |ui: &mut Ui, id: WidgetId, wanted: Mode| {
        let mode = mode.clone();
        ui.set_visible_fn(id, move || mode.get() == wanted);
    };
    let progress_container = ui.child(widget, "PROGRESS");
    in_mode(ui, progress_container, Mode::Progress);
    let bar = ui.child(widget, "PROGRESS_BAR");
    ui.get::<ProgressBarWidget>(bar).is_indeterminate = Some(Rc::new(|| true));
    let progress_label = ui.child(widget, "PROGRESS_MESSAGE");
    ui.label(progress_label).get_text = Some(text_getter(&progress_text));

    let message_container = ui.child(widget, "MESSAGE");
    in_mode(ui, message_container, Mode::Message);
    let message_label = ui.child(message_container, "MESSAGE_MESSAGE");
    ui.label(message_label).get_text = Some(text_getter(&message_text));

    let list_container = ui.child(widget, "LIST");
    in_mode(ui, list_container, Mode::List);
    let list_panel = ui.child(list_container, "LIST_PANEL");
    let list_header_template = ui.child(list_panel, "LIST_HEADER_TEMPLATE");
    let label_list_template = ui.child(list_panel, "LABEL_LIST_TEMPLATE");
    let checkbox_list_template = ui.child(list_panel, "CHECKBOX_LIST_TEMPLATE");
    for template in [list_header_template, label_list_template, checkbox_list_template] {
        ui.hide_child(list_panel, template);
    }
    let list_label = ui.child(list_container, "LIST_MESSAGE");
    ui.label(list_label).get_text = Some(text_getter(&list_text));

    let primary = ui.child(widget, "PRIMARY_BUTTON");
    let primary_visible = Rc::new(Cell::new(true));
    let visible = primary_visible.clone();
    ui.set_visible_fn(primary, move || visible.get());

    let state = Rc::new(InstallState {
        ctx,
        panel: widget,
        primary,
        primary_visible,
        secondary: ui.child(widget, "SECONDARY_BUTTON"),
        progress_container,
        progress_label,
        message_container,
        list_container,
        list_panel,
        list_header_template,
        label_list_template,
        checkbox_list_template,
        mode,
        title,
        progress_text,
        message_text,
        list_text,
        events: RefCell::new(None),
        selected: Rc::default(),
        retry_install: RefCell::new(None),
    });
    state.detect(ui);
    cell(InstallLogic { state })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_of_urls() {
        assert_eq!(host_of("https://a.example.net:8080/x/y.zip?z=1"), "a.example.net");
        assert_eq!(host_of("http://user@b.example.net/"), "b.example.net");
        assert_eq!(host_of("c.example.net/file"), "c.example.net");
    }

    #[test]
    fn download_sizes_use_the_magnitude_of_the_total() {
        let status = download_status("h", 1024 * 1024, Some(10 * 1024 * 1024));
        assert_eq!(status.key, "label-downloading-from-progress");
        assert!(status.args.contains(&("received", StatusArg::Text("1.00".to_string()))));
        assert!(status.args.contains(&("total", StatusArg::Text("10.00".to_string()))));
        assert!(status.args.contains(&("suffix", StatusArg::Text("MB".to_string()))));
        assert!(status.args.contains(&("progress", StatusArg::Number(10))));

        let status = download_status("h", 2048, None);
        assert_eq!(status.key, "label-downloading-from");
        assert!(status.args.contains(&("received", StatusArg::Text("2.00".to_string()))));
        assert!(status.args.contains(&("suffix", StatusArg::Text("KB".to_string()))));

        let status = download_status("h", 0, None);
        assert!(status.args.contains(&("suffix", StatusArg::Text("bytes".to_string()))));
    }
}
