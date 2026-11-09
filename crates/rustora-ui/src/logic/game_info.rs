// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `GameInfoLogic.cs`, `GameInfoBriefingLogic.cs`, `GameInfoObjectivesLogic.cs`, `GameInfoStatsLogic.cs`
//! and `DebugMenuLogic.cs`. The script error panel comes with Lua in P9. The debug visualizations and the terrain
//! overlays are not ported, so their checkboxes are disabled.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use rustora_core::fieldloader::Color;
use rustora_engine::order::Order;
use rustora_engine::player::{PlayerRef, PlayerRelationship, WinState};
use rustora_engine::target::Target;
use rustora_engine::world::World;
use rustora_mods_common::base::DeveloperMode;
use rustora_mods_common::player::objectives::{MissionDataInfo, MissionObjectives, ObjectiveState, ObjectivesPanelInfo};
use rustora_mods_common::player::statistics::PlayerStatistics;
use rustora_mods_common::world::FactionInfo;
use rustora_net::session::{Client, ClientState};

use super::ingame::Ingame;
use super::lobby_options::GetRules;
use super::sidebar::resolved_player_name;
use super::{ChromeLogic, NoLogic, button_prompt_full, message, message_with, services};
use crate::fluent::MessageArg;
use crate::loader::cell;
use crate::net::Net;
use crate::widget::{Action, Getter, Ui, WidgetArgs, WidgetId};
use crate::widgets::{CheckboxWidget, ImageWidget, LabelWithTooltipWidget, MapPreviewWidget, ScrollItemWidget};

/// `IngameInfoPanel`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IngameInfoPanel {
    AutoSelect,
    Map,
    Objectives,
    Debug,
    Chat,
    LobbyOptions,
}

/// The `hideMenu` argument of the game info panels.
pub type HideMenu = Rc<dyn Fn(bool)>;

/// `DeveloperModeInfo.CheckboxEnabled` overridden by the `cheats` lobby option.
pub fn cheats_enabled(world: &World) -> bool {
    let default = world
        .rules()
        .actor("player")
        .and_then(|a| a.trait_info::<rustora_mods_common::base::DeveloperModeInfo>())
        .is_some_and(|i| i.checkbox_enabled);
    world.lobby_info.global_settings.option_bool("cheats", default)
}

fn ingame(ui: &Ui) -> Option<Rc<Ingame>> {
    services(ui).ingame.borrow().clone()
}

/// `GameInfoLogic`.
pub fn game_info(ui: &mut Ui, widget: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let Some(ingame) = ingame(ui) else {
        return cell(NoLogic);
    };
    let world = ingame.world.clone();
    let hide_menu = args.value::<HideMenu>("hideMenu").unwrap_or_else(|| Rc::new(|_| {}));
    let close_menu = args.action("closeMenu");
    let active = Rc::new(Cell::new(
        args.value::<IngameInfoPanel>("initialPanel").unwrap_or(IngameInfoPanel::AutoSelect),
    ));

    let world_info = &world.world_actor().info;
    let objectives_panel = world_info.trait_info::<ObjectivesPanelInfo>().and_then(|i| i.panel_name.clone());
    let briefing = world_info
        .trait_info::<MissionDataInfo>()
        .and_then(|i| i.briefing.clone())
        .filter(|b| !b.is_empty());

    let mut visible = Vec::new();
    if objectives_panel.is_some() {
        visible.push(IngameInfoPanel::Objectives);
    }
    if briefing.is_some() {
        visible.push(IngameInfoPanel::Map);
    }
    visible.push(IngameInfoPanel::LobbyOptions);
    if ingame.local_player.is_some() && cheats_enabled(&world) {
        visible.push(IngameInfoPanel::Debug);
    }
    if world.lobby_info.clients.iter().filter(|c| !c.is_bot()).count() > 1 {
        visible.push(IngameInfoPanel::Chat);
    }

    let tab_container = ui.find(widget, &format!("TAB_CONTAINER_{}", visible.len()));
    if let Some(t) = tab_container {
        ui.set_visible_fn(t, || true);
    }
    let chat_panel = ui.child(widget, "CHAT_PANEL");
    for (i, &panel) in visible.iter().enumerate() {
        let (container_name, label) = match panel {
            IngameInfoPanel::Objectives => ("OBJECTIVES_PANEL", "menu-game-info.objectives"),
            IngameInfoPanel::Map => ("MAP_PANEL", "menu-game-info.briefing"),
            IngameInfoPanel::LobbyOptions => ("LOBBY_OPTIONS_PANEL", "menu-game-info.options"),
            IngameInfoPanel::Debug => ("DEBUG_PANEL", "menu-game-info.debug"),
            IngameInfoPanel::Chat | IngameInfoPanel::AutoSelect => ("CHAT_PANEL", "menu-game-info.chat"),
        };
        let tab = tab_container.and_then(|t| ui.find(t, &format!("BUTTON{}", i + 1)));
        if let Some(tab) = tab {
            let text = message(ui, label);
            let (a, a2) = (active.clone(), active.clone());
            let mut b = ui.button(tab);
            b.get_text = Some(Rc::new(move || text.clone()));
            b.on_click = Some(Rc::new(move |ui: &mut Ui| {
                if a.get() == IngameInfoPanel::Chat {
                    leave_chat_panel(ui, chat_panel);
                }
                a.set(panel);
            }));
            b.is_highlighted = Some(Rc::new(move || a2.get() == panel));
        }
        let container = ui.child(widget, container_name);
        let a = active.clone();
        ui.set_visible_fn(container, move || a.get() == panel);
        match panel {
            IngameInfoPanel::Objectives => {
                let name = objectives_panel.clone().unwrap_or_default();
                let args = WidgetArgs::default()
                    .with_value("hideMenu", hide_menu.clone())
                    .with_action("closeMenu", close_menu.clone());
                ui.load_widget(&name, Some(container), &args);
            }
            IngameInfoPanel::Map => {
                ui.load_widget("MAP_PANEL", Some(container), &WidgetArgs::default());
            }
            IngameInfoPanel::LobbyOptions => {
                let uid = world.map().uid.clone();
                let rules = services(ui).lobby_rules.borrow().clone().and_then(|f| f(&uid));
                let get_rules: GetRules = Rc::new(move || rules.clone().map(|r| (uid.clone(), r)));
                let disabled: Getter<bool> = Rc::new(|| true);
                let args = WidgetArgs::default()
                    .with_value("getRules", get_rules)
                    .with_value("configurationDisabled", disabled);
                ui.load_widget("LOBBY_OPTIONS_PANEL", Some(container), &args);
            }
            IngameInfoPanel::Debug => {
                if let Some(tab) = tab {
                    let w = world.clone();
                    ui.button(tab).is_disabled = Some(Rc::new(move || w.is_game_over()));
                }
                ui.load_widget("DEBUG_PANEL", Some(container), &WidgetArgs::default());
                if active.get() == IngameInfoPanel::AutoSelect {
                    active.set(IngameInfoPanel::Debug);
                }
            }
            IngameInfoPanel::Chat | IngameInfoPanel::AutoSelect => {
                if let Some(tab) = tab {
                    let last = ui.button(tab).on_click.clone();
                    ui.button(tab).on_click = Some(Rc::new(move |ui: &mut Ui| {
                        if let Some(f) = &last {
                            f(ui);
                        }
                        if let Some(field) = ui.find(chat_panel, "CHAT_TEXTFIELD") {
                            ui.take_keyboard_focus(field);
                        }
                    }));
                }
                let args = WidgetArgs::default().with_value("isMenuChat", true);
                ui.load_widget("CHAT_CONTAINER", Some(container), &args);
            }
        }
        if active.get() == IngameInfoPanel::AutoSelect {
            active.set(panel);
        }
    }

    let title = ui.child(widget, "TITLE");
    let map = services(ui).maps.borrow().get(&world.map().uid);
    let map_title = match map.as_ref().and_then(|m| m.categories.first()) {
        Some(category) => format!("{category}: {}", world.map().title),
        None => world.map().title.clone(),
    };
    ui.label(title).get_text = Some(Rc::new(move || map_title.clone()));
    cell(NoLogic)
}

fn leave_chat_panel(ui: &mut Ui, chat_panel: WidgetId) {
    if let Some(field) = ui.find(chat_panel, "CHAT_TEXTFIELD") {
        ui.yield_keyboard_focus(field);
    }
}

/// `GameInfoBriefingLogic`.
pub fn briefing(ui: &mut Ui, widget: WidgetId) -> Rc<RefCell<dyn ChromeLogic>> {
    let Some(ingame) = ingame(ui) else {
        return cell(NoLogic);
    };
    let world = ingame.world.clone();
    let preview = services(ui).maps.borrow().get(&world.map().uid);
    let preview_widget = ui.child(widget, "MAP_PREVIEW");
    ui.get::<MapPreviewWidget>(preview_widget).preview = Some(Rc::new(move || preview.clone()));
    let panel = ui.child(widget, "MAP_DESCRIPTION_PANEL");
    let description = ui.child(widget, "MAP_DESCRIPTION");
    let briefing = world
        .world_actor()
        .info
        .trait_info::<MissionDataInfo>()
        .map(|i| i.briefing.clone().unwrap_or_default());
    if let Some(briefing) = briefing {
        let font = ui.label(description).font.clone();
        let width = ui.node(description).bounds.w;
        let text = ui.assets.borrow_mut().wrap_text(&font, &briefing.replace("\\n", "\n"), width);
        ui.node_mut(description).bounds.h = ui.measure(&font, &text).1;
        ui.label(description).get_text = Some(Rc::new(move || text.clone()));
        ui.scroll_to_top(panel, false);
        ui.adjust_children(panel);
    }
    cell(NoLogic)
}

fn mission_status_getters(ui: &Ui, player: &PlayerRef) -> (Getter<String>, Getter<Color>) {
    let (in_progress, accomplished, failed) = (
        message(ui, "label-mission-in-progress"),
        message(ui, "label-mission-accomplished"),
        message(ui, "label-mission-failed"),
    );
    let p = player.clone();
    let text: Getter<String> = Rc::new(move || match p.win_state.get() {
        WinState::Won => accomplished.clone(),
        WinState::Lost => failed.clone(),
        WinState::Undefined => in_progress.clone(),
    });
    let p = player.clone();
    let color: Getter<Color> = Rc::new(move || match p.win_state.get() {
        WinState::Won => Color::from_argb(255, 50, 205, 50),
        WinState::Lost => Color::from_argb(255, 255, 0, 0),
        WinState::Undefined => Color::from_argb(255, 255, 255, 255),
    });
    (text, color)
}

/// `GameInfoObjectivesLogic`.
struct ObjectivesLogic {
    objectives: Rc<MissionObjectives>,
    panel: WidgetId,
    template: WidgetId,
    count: usize,
}

impl ObjectivesLogic {
    /// `PopulateObjectivesList`.
    fn populate(&mut self, ui: &mut Ui) {
        ui.remove_children(self.panel);
        let mut objectives = self.objectives.objectives();
        objectives.sort_by(|a, b| a.ty.cmp(&b.ty));
        for (index, objective) in objectives.into_iter().enumerate() {
            let item = ui.clone_widget(self.template);
            let label = ui.child(item, "OBJECTIVE_TYPE");
            let ty = objective.ty.clone();
            ui.label(label).get_text = Some(Rc::new(move || ty.clone()));
            let checkbox = ui.child(item, "OBJECTIVE_STATUS");
            let mo = self.objectives.clone();
            let state = move || {
                let mut o = mo.objectives();
                o.sort_by(|a, b| a.ty.cmp(&b.ty));
                o.get(index).map_or(ObjectiveState::Incomplete, |o| o.state)
            };
            let state = Rc::new(state);
            let mut c = ui.get::<CheckboxWidget>(checkbox);
            let s = state.clone();
            c.is_checked = Some(Rc::new(move || s() != ObjectiveState::Incomplete));
            let s = state.clone();
            c.get_checkmark = Some(Rc::new(move || {
                if s() == ObjectiveState::Completed { "tick" } else { "cross" }.to_string()
            }));
            let description = objective.description.clone();
            c.button.get_text = Some(Rc::new(move || description.clone()));
            drop(c);
            ui.add_child(self.panel, item);
        }
        self.count = self.objectives.objective_count();
    }
}

impl ChromeLogic for ObjectivesLogic {
    fn tick(&mut self, ui: &mut Ui) {
        if self.objectives.objective_count() != self.count {
            self.populate(ui);
        }
    }
}

/// `GameInfoObjectivesLogic`.
pub fn objectives(ui: &mut Ui, widget: WidgetId) -> Rc<RefCell<dyn ChromeLogic>> {
    let Some(ingame) = ingame(ui) else {
        return cell(NoLogic);
    };
    let panel = ui.child(widget, "OBJECTIVES_PANEL");
    let template = ui.child(panel, "OBJECTIVE_TEMPLATE");
    let player = ingame.render_player().clone().or_else(|| ingame.local_player.clone());
    let Some((player, mo)) = player.and_then(|p| {
        let mo = p.player_actor().trait_or_default::<MissionObjectives>()?;
        Some((p, mo))
    }) else {
        ui.remove_children(panel);
        return cell(NoLogic);
    };
    let status = ui.child(widget, "MISSION_STATUS");
    let (text, color) = mission_status_getters(ui, &player);
    ui.label(status).get_text = Some(text);
    ui.label(status).get_color = Some(color);
    ui.hide_child(panel, template);
    let mut logic = ObjectivesLogic {
        objectives: mo,
        panel,
        template,
        count: 0,
    };
    logic.populate(ui);
    cell(logic)
}

fn client_state(net: &Net, index: i32) -> Option<ClientState> {
    net.borrow().session.client_with_index(index).map(|c| c.state)
}

/// `UnitOrders.KickVoteTarget` and the client queries of `GameInfoStatsLogic`.
#[derive(Clone)]
struct KickContext {
    net: Net,
    world: Rc<World>,
    hide_menu: HideMenu,
    close_menu: Action,
}

impl KickContext {
    /// `LocalPlayerCanKick`.
    fn local_player_can_kick(&self) -> bool {
        let n = self.net.borrow();
        let Some(local) = n.local_client() else {
            return false;
        };
        if local.is_admin {
            return true;
        }
        let local_player = self.world.players().into_iter().find(|p| p.client_index == local.index);
        !local.is_observer() && local_player.is_some_and(|p| p.win_state.get() == WinState::Undefined)
    }

    /// `CanClientBeKicked`.
    fn can_client_be_kicked(&self, client_index: i32, vote_kick: bool) -> bool {
        let n = self.net.borrow();
        let Some(client) = n.session.client_with_index(client_index) else {
            return false;
        };
        let local_index = n.local_client().map_or(-1, |c| c.index);
        client.index != local_index
            && client.state != ClientState::Disconnected
            && (!client.is_admin || n.session.global_settings.independent_server)
            && (!vote_kick || n.kick_vote_target.is_none_or(|t| t == client.index))
    }

    fn is_host(&self) -> bool {
        self.net.borrow().is_host()
    }

    /// `KickAction`.
    fn kick(&self, ui: &mut Ui, client: &Client, vote_kick: bool) {
        (self.hide_menu)(true);
        let name = client.name.clone().unwrap_or_default();
        let index = client.index;
        let show: Action = {
            let h = self.hide_menu.clone();
            Rc::new(move |_| h(false))
        };
        if !vote_kick {
            let title = message_with(ui, "dialog-kick.title", &[("player", MessageArg::Text(&name))]);
            let text = message(ui, "dialog-kick.prompt");
            let (net, h) = (self.net.clone(), self.hide_menu.clone());
            let on_confirm: Action = Rc::new(move |_| {
                net.borrow_mut().command(&format!("kick {index} False"));
                h(false);
            });
            button_prompt_full(ui, title, &text, on_confirm, "dialog-kick.confirm", Some(show), None, None);
            return;
        }
        let bots = if client.is_admin {
            self.world
                .players()
                .iter()
                .filter(|p| p.is_bot && p.win_state.get() == WinState::Undefined)
                .count()
        } else {
            0
        };
        let title = message_with(ui, "dialog-vote-kick.title", &[("player", MessageArg::Text(&name))]);
        let text = if bots > 0 {
            message_with(
                ui,
                "dialog-vote-kick.prompt-break-bots",
                &[("bots", MessageArg::Number(bots as i64))],
            )
        } else {
            message(ui, "dialog-vote-kick.prompt")
        };
        let vote = |ctx: &KickContext, value: bool| -> Action {
            let (net, h, close) = (ctx.net.clone(), ctx.hide_menu.clone(), ctx.close_menu.clone());
            let flag = if value { "True" } else { "False" };
            Rc::new(move |ui: &mut Ui| {
                net.borrow_mut().command(&format!("vote_kick {index} {flag}"));
                h(false);
                close(ui);
            })
        };
        if self.net.borrow().kick_vote_target.is_none() {
            button_prompt_full(
                ui,
                title,
                &text,
                vote(self, true),
                "dialog-vote-kick.vote-start",
                Some(show),
                None,
                None,
            );
            return;
        }
        let against = vote(self, false);
        let on_other: Action = Rc::new(move |ui: &mut Ui| {
            ui.close_window();
            against(ui);
        });
        button_prompt_full(
            ui,
            title,
            &text,
            vote(self, true),
            "dialog-vote-kick.vote-for",
            Some(show),
            Some("dialog-vote-kick.vote-cancel"),
            Some((on_other, "dialog-vote-kick.vote-against")),
        );
    }
}

/// Binds the `MUTE` checkbox and the `KICK` button of a player or spectator row.
fn bind_row_actions(ui: &mut Ui, item: WidgetId, ctx: &KickContext, client: &Client, is_bot: bool, host_vote_kick: Getter<bool>) {
    let index = client.index;
    let (unmute, mute) = (message(ui, "label-unmute-player"), message(ui, "label-mute-player"));
    let mute_checkbox = ui.child(item, "MUTE");
    {
        let net = ctx.net.clone();
        let mut c = ui.get::<CheckboxWidget>(mute_checkbox);
        let n = net.clone();
        c.is_checked = Some(Rc::new(move || n.borrow().muted_players.contains(&index)));
        let n = net.clone();
        c.button.on_click = Some(Rc::new(move |_| {
            let muted = &mut n.borrow_mut().muted_players;
            if !muted.remove(&index) {
                muted.insert(index);
            }
        }));
        let n = net.clone();
        c.button.get_tooltip_text = Some(Rc::new(move || {
            if n.borrow().muted_players.contains(&index) {
                unmute.clone()
            } else {
                mute.clone()
            }
        }));
    }
    let net = ctx.net.clone();
    ui.set_visible_fn(mute_checkbox, move || {
        let n = net.borrow();
        !is_bot
            && n.session
                .client_with_index(index)
                .is_some_and(|c| c.state != ClientState::Disconnected)
            && n.local_client().is_none_or(|l| l.index != index)
    });

    let kick = ui.child(item, "KICK");
    let (vote_tooltip, kick_tooltip) = (message(ui, "button-vote-kick-player"), message(ui, "button-kick-player"));
    {
        let v = host_vote_kick.clone();
        let mut b = ui.button(kick);
        b.get_tooltip_text = Some(Rc::new(move || if v() { vote_tooltip.clone() } else { kick_tooltip.clone() }));
        let (ctx, client, v) = (ctx.clone(), client.clone(), host_vote_kick.clone());
        b.on_click = Some(Rc::new(move |ui: &mut Ui| ctx.kick(ui, &client, v())));
    }
    let (ctx, v) = (ctx.clone(), host_vote_kick);
    ui.set_visible_fn(kick, move || {
        !is_bot && ctx.local_player_can_kick() && ctx.can_client_be_kicked(index, v())
    });
}

/// The text getter of `WidgetUtils.BindPlayerNameAndStatus`, where `name` is `ResolvedPlayerName`.
pub(crate) fn player_name_and_status(ui: &Ui, label: WidgetId, world: &World, player: &PlayerRef, name: &str, net: &Net) -> Getter<String> {
    let variants = player_name_variants(ui, label, name);
    let (p, n, index) = (player.clone(), net.clone(), player.client_index);
    let has_client = world.lobby_info.client_with_index(index).is_some();
    Rc::new(move || {
        if has_client && client_state(&n, index) == Some(ClientState::Disconnected) {
            return variants[3].clone();
        }
        match p.win_state.get() {
            WinState::Won => variants[1].clone(),
            WinState::Lost => variants[2].clone(),
            WinState::Undefined => variants[0].clone(),
        }
    })
}

/// The text of `WidgetUtils.BindPlayerNameAndStatus` for each status, truncated to the label.
fn player_name_variants(ui: &Ui, label: WidgetId, name: &str) -> [String; 4] {
    let font = ui.label(label).font.clone();
    let width = ui.node(label).bounds.w;
    let base = ui.measure(&font, name).0;
    let variant = |suffix: &str| {
        let text = format!("{name}{suffix}");
        let size = ui.measure(&font, &text).0 - base;
        ui.truncate_text(&font, &text, width - size)
    };
    [
        variant(""),
        variant(&format!(" ({})", message(ui, "label-win-state-won"))),
        variant(&format!(" ({})", message(ui, "label-win-state-lost"))),
        variant(&format!(" ({})", message(ui, "label-client-state-disconnected"))),
    ]
}

fn faction_name(factions: &[Rc<FactionInfo>], internal: &str) -> String {
    factions
        .iter()
        .find(|f| f.internal_name.as_deref() == Some(internal))
        .and_then(|f| f.name.clone())
        .unwrap_or_else(|| internal.to_string())
}

/// `GameInfoStatsLogic`.
pub fn stats(ui: &mut Ui, widget: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let Some(ingame) = ingame(ui) else {
        return cell(NoLogic);
    };
    let world = ingame.world.clone();
    let net = services(ui).net.clone();
    let player = ingame.local_player.clone();
    let player_panel = ui.child(widget, "PLAYER_LIST");
    let stats_header = ui.child(widget, "STATS_HEADERS");
    match player.as_ref().filter(|p| !p.non_combatant) {
        Some(player) => {
            let checkbox = ui.child(widget, "STATS_CHECKBOX");
            let status = ui.child(widget, "STATS_STATUS");
            {
                let mut c = ui.get::<CheckboxWidget>(checkbox);
                let p = player.clone();
                c.is_checked = Some(Rc::new(move || p.win_state.get() != WinState::Undefined));
                let p = player.clone();
                c.get_checkmark = Some(Rc::new(move || {
                    if p.win_state.get() == WinState::Won { "tick" } else { "cross" }.to_string()
                }));
                if player.has_objectives.get()
                    && let Some(mo) = player.player_actor().trait_or_default::<MissionObjectives>()
                {
                    c.button.get_text = Some(Rc::new(move || {
                        mo.objectives().first().map(|o| o.description.clone()).unwrap_or_default()
                    }));
                }
            }
            let (text, color) = mission_status_getters(ui, player);
            ui.label(status).get_text = Some(text);
            ui.label(status).get_color = Some(color);
        }
        None => {
            let objective = ui.child(widget, "OBJECTIVE");
            ui.node_mut(objective).visible = false;
            let h = ui.node(objective).bounds.h;
            ui.node_mut(stats_header).bounds.y -= h;
            let b = &mut ui.node_mut(player_panel).bounds;
            b.y -= h;
            b.h += h;
        }
    }

    let others_connected = {
        let n = net.borrow();
        let local = n.local_client().map(|c| c.index);
        n.session
            .clients
            .iter()
            .any(|c| !c.is_bot() && Some(c.index) != local && c.state != ClientState::Disconnected)
    };
    if !others_connected {
        let actions = ui.child(stats_header, "ACTIONS");
        ui.node_mut(actions).visible = false;
    }

    let team_template = ui.child(player_panel, "TEAM_TEMPLATE");
    let player_template = ui.child(player_panel, "PLAYER_TEMPLATE");
    let spectator_template = ui.child(player_panel, "SPECTATOR_TEMPLATE");
    for t in [team_template, player_template, spectator_template] {
        ui.hide_child(player_panel, t);
    }

    let ctx = KickContext {
        net: net.clone(),
        world: world.clone(),
        hide_menu: args.value::<HideMenu>("hideMenu").unwrap_or_else(|| Rc::new(|_| {})),
        close_menu: args.action("closeMenu"),
    };
    let factions = world.world_actor().info.trait_infos::<FactionInfo>();
    let stats_of = |p: &PlayerRef| p.player_actor().trait_or_default::<PlayerStatistics>();
    let experience = |s: &Option<Rc<PlayerStatistics>>| s.as_ref().map_or(0, |s| s.experience());

    let mut members: Vec<(PlayerRef, Option<Rc<PlayerStatistics>>)> = world
        .players()
        .into_iter()
        .filter(|p| !p.non_combatant && p.playable)
        .map(|p| {
            let s = stats_of(&p);
            (p, s)
        })
        .collect();
    members.sort_by_key(|(_, s)| std::cmp::Reverse(experience(s)));
    let mut teams: Vec<(i32, Vec<(PlayerRef, Option<Rc<PlayerStatistics>>)>)> = Vec::new();
    for m in members {
        let team = world.lobby_info.client_with_index(m.0.client_index).map_or(0, |c| c.team);
        match teams.iter_mut().find(|(t, _)| *t == team) {
            Some((_, list)) => list.push(m),
            None => teams.push((team, vec![m])),
        }
    }
    teams.sort_by_key(|(_, list)| std::cmp::Reverse(list.iter().map(|(_, s)| experience(s)).sum::<i32>()));

    let team_count = teams.len();
    for (team, list) in teams {
        if team_count > 1 {
            let header = ScrollItemWidget::setup(ui, team_template, Rc::new(|| false), Rc::new(|_| {}));
            let name = if team > 0 {
                message_with(ui, "label-team-name", &[("team", MessageArg::Number(team as i64))])
            } else {
                message(ui, "label-no-team")
            };
            let label = ui.child(header, "TEAM");
            ui.label(label).get_text = Some(Rc::new(move || name.clone()));
            let score = ui.child(header, "TEAM_SCORE");
            let stats: Vec<Rc<PlayerStatistics>> = list.iter().filter_map(|(_, s)| s.clone()).collect();
            ui.label(score).get_text = Some(Rc::new(move || stats.iter().map(|s| s.experience()).sum::<i32>().to_string()));
            ui.add_child(player_panel, header);
        }
        for (pp, statistics) in list {
            let client = world.lobby_info.client_with_index(pp.client_index).cloned().unwrap_or_default();
            let item = ui.clone_widget(player_template);
            super::lobby_utils::setup_profile_widget(ui, item, &client);

            let name_label = ui.child(item, "NAME");
            let name = resolved_player_name(ui, &world, &pp);
            let text = player_name_and_status(ui, name_label, &world, &pp, &name, &net);
            {
                let mut l = ui.label(name_label);
                l.get_text = Some(text);
                let color = pp.color;
                l.get_color = Some(Rc::new(move || color));
            }

            let flag = ui.child(item, "FACTIONFLAG");
            let display_name = message(ui, &faction_name(&factions, &pp.display_faction));
            let reveal = player
                .as_ref()
                .is_none_or(|l| l.relationship_with(Some(&pp)) == PlayerRelationship::ALLY || l.win_state.get() != WinState::Undefined);
            let (flag_name, faction_text) = if reveal {
                let real = message(ui, &faction_name(&factions, &pp.faction));
                let text = if pp.faction != pp.display_faction {
                    format!("{display_name} ({real})")
                } else {
                    real
                };
                (pp.faction.clone(), text)
            } else {
                (pp.display_faction.clone(), display_name)
            };
            {
                let mut f = ui.get::<ImageWidget>(flag);
                f.get_image_collection = Some(Rc::new(|| "flags".to_string()));
                f.get_image_name = Some(Rc::new(move || flag_name.clone()));
            }
            let faction_label = ui.child(item, "FACTION");
            LabelWithTooltipWidget::truncate_to_tooltip(ui, faction_label, &faction_text);

            let score = ui.child(item, "SCORE");
            ui.label(score).get_text = Some(Rc::new(move || statistics.as_ref().map_or(0, |s| s.experience()).to_string()));

            let (p, c) = (pp.clone(), ctx.clone());
            let vote_kick: Getter<bool> = Rc::new(move || !c.is_host() || p.win_state.get() == WinState::Undefined);
            bind_row_actions(ui, item, &ctx, &client, pp.is_bot, vote_kick);
            ui.add_child(player_panel, item);
        }
    }

    let spectators: Vec<Client> = net.borrow().session.clients.iter().filter(|c| c.is_observer()).cloned().collect();
    if !spectators.is_empty() {
        let header = ScrollItemWidget::setup(ui, team_template, Rc::new(|| false), Rc::new(|_| {}));
        let text = message(ui, "label-spectators");
        let label = ui.child(header, "TEAM");
        ui.label(label).get_text = Some(Rc::new(move || text.clone()));
        ui.add_child(player_panel, header);
        for client in spectators {
            let item = ui.clone_widget(spectator_template);
            super::lobby_utils::setup_profile_widget(ui, item, &client);
            let name_label = ui.child(item, "NAME");
            let font = ui.label(name_label).font.clone();
            let width = ui.node(name_label).bounds.w;
            let name = client.name.clone().unwrap_or_default();
            let gone = format!(" ({})", message(ui, "label-client-state-disconnected"));
            let connected_text = ui.truncate_text(&font, &name, width);
            let gone_width = ui.measure(&font, &gone).0;
            let gone_text = ui.truncate_text(&font, &name, width - gone_width) + &gone;
            let (n, index) = (net.clone(), client.index);
            ui.label(name_label).get_text = Some(Rc::new(move || {
                if client_state(&n, index) == Some(ClientState::Disconnected) {
                    gone_text.clone()
                } else {
                    connected_text.clone()
                }
            }));
            let c = ctx.clone();
            let vote_kick: Getter<bool> = Rc::new(move || !c.is_host());
            bind_row_actions(ui, item, &ctx, &client, client.is_bot(), vote_kick);
            ui.add_child(player_panel, item);
        }
    }
    cell(NoLogic)
}

/// `PredictedCachedTransform<bool, bool>` with the identity transform.
#[derive(Default)]
struct PredictedBool {
    value: Cell<bool>,
    last_input: Cell<Option<bool>>,
    predicted: Cell<bool>,
}

impl PredictedBool {
    fn update(&self, input: bool) -> bool {
        if (self.predicted.get() || self.last_input.get().is_some()) && self.last_input.get() == Some(input) {
            return self.value.get();
        }
        self.value.set(input);
        self.last_input.set(Some(input));
        self.predicted.set(false);
        input
    }

    fn predict(&self, value: bool) {
        self.predicted.set(true);
        self.value.set(value);
    }
}

/// `DebugMenuLogic.IssueOrder`.
fn issue_dev_order(world: &World, player: &PlayerRef, order: &str) {
    world.issue_order(Order::new(order, Some(player.player_actor().clone()), Target::Invalid, false));
}

/// `DebugMenuLogic`.
pub fn debug_menu(ui: &mut Ui, widget: WidgetId) -> Rc<RefCell<dyn ChromeLogic>> {
    let Some(ingame) = ingame(ui) else {
        return cell(NoLogic);
    };
    let Some(player) = ingame.local_player.clone() else {
        return cell(NoLogic);
    };
    let world = ingame.world.clone();
    let Some(dev) = player.player_actor().trait_or_default::<DeveloperMode>() else {
        return cell(NoLogic);
    };
    type DevGetter = fn(&DeveloperMode) -> bool;
    let checkboxes: [(&str, &str, DevGetter); 7] = [
        ("DISABLE_VISIBILITY_CHECKS", "DevVisibility", DeveloperMode::disable_shroud),
        ("SHOW_UNIT_PATHS", "DevPathDebug", DeveloperMode::path_debug),
        ("INSTANT_BUILD", "DevFastBuild", DeveloperMode::fast_build),
        ("INSTANT_CHARGE", "DevFastCharge", DeveloperMode::fast_charge),
        ("ENABLE_TECH", "DevEnableTech", DeveloperMode::all_tech),
        ("UNLIMITED_POWER", "DevUnlimitedPower", DeveloperMode::unlimited_power),
        ("BUILD_ANYWHERE", "DevBuildAnywhere", DeveloperMode::build_anywhere),
    ];
    for (id, order, get) in checkboxes {
        let Some(checkbox) = ui.find(widget, id) else { continue };
        let predicted = Rc::new(PredictedBool::default());
        let mut c = ui.get::<CheckboxWidget>(checkbox);
        let (p, d) = (predicted.clone(), dev.clone());
        c.is_checked = Some(Rc::new(move || p.update(get(&d))));
        let (p, d, w, pl) = (predicted, dev.clone(), world.clone(), player.clone());
        c.button.on_click = Some(Rc::new(move |_| {
            p.predict(!get(&d));
            issue_dev_order(&w, &pl, order);
        }));
    }
    let buttons = [
        ("GIVE_CASH", "DevGiveCash"),
        ("GROW_RESOURCES", "DevGrowResources"),
        ("GIVE_EXPLORATION", "DevGiveExploration"),
        ("RESET_EXPLORATION", "DevResetExploration"),
    ];
    for (id, order) in buttons {
        let Some(button) = ui.find(widget, id) else { continue };
        let (w, pl) = (world.clone(), player.clone());
        ui.button(button).on_click = Some(Rc::new(move |_| issue_dev_order(&w, &pl, order)));
    }
    for id in [
        "SHOW_COMBATOVERLAY",
        "SHOW_GEOMETRY",
        "SHOW_SCREENMAP",
        "SHOW_DEPTH_PREVIEW",
        "SHOW_ACTOR_TAGS",
        "SHOW_CUSTOMTERRAIN_OVERLAY",
        "SHOW_TERRAIN_OVERLAY",
    ] {
        if let Some(checkbox) = ui.find(widget, id) {
            ui.get::<CheckboxWidget>(checkbox).button.disabled = true;
        }
    }
    cell(NoLogic)
}
