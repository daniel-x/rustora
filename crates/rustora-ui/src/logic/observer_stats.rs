// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `ObserverStatsLogic.cs`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use rustora_core::fieldloader::Color;
use rustora_core::miniyaml::MiniYamlNode;
use rustora_engine::player::PlayerRef;
use rustora_engine::world::World;
use rustora_mods_common::base::{BaseBuilding, PowerManager, PowerState};
use rustora_mods_common::harvester::Harvester;
use rustora_mods_common::player::PlayerResources;
use rustora_mods_common::player::statistics::{PlayerStatistics, UpdatesDerrickCountInfo};

use super::game_info::player_name_and_status;
use super::ingame::Ingame;
use super::sidebar::resolved_player_name;
use super::{NoLogic, message, message_with, services};
use crate::fluent::MessageArg;
use crate::input::{KeyEvent, KeyInput};
use crate::loader::cell;
use crate::widget::{ChromeLogic, Getter, Ui, WidgetId};
use crate::widgets::observer::{
    GradientColorBlockWidget, LineGraphSeries, LineGraphWidget, ObserverArmyIconsWidget, ObserverProductionIconsWidget,
    ObserverSupportPowerIconsWidget,
};
use crate::widgets::production::LogicKeyListenerWidget;
use crate::widgets::{ColorBlockWidget, DropDownButtonWidget, ImageWidget, ScrollBar, ScrollItemWidget, ScrollPanelWidget};

/// `ObserverStatsPanel`, in the order of the drop down options and the hotkey names.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Panel {
    None,
    Basic,
    Economy,
    Production,
    SupportPowers,
    Combat,
    Army,
    Graph,
    ArmyGraph,
}

const PANELS: [(Panel, &str, &str); 9] = [
    (Panel::None, "None", "options-observer-stats.none"),
    (Panel::Basic, "Basic", "options-observer-stats.basic"),
    (Panel::Economy, "Economy", "options-observer-stats.economy"),
    (Panel::Production, "Production", "options-observer-stats.production"),
    (Panel::SupportPowers, "SupportPowers", "options-observer-stats.support-powers"),
    (Panel::Combat, "Combat", "options-observer-stats.combat"),
    (Panel::Army, "Army", "options-observer-stats.army"),
    (Panel::Graph, "Graph", "options-observer-stats.earnings-graph"),
    (Panel::ArmyGraph, "ArmyGraph", "options-observer-stats.army-graph"),
];

impl Panel {
    fn template(self) -> Option<&'static str> {
        Some(match self {
            Panel::Basic => "BASIC_PLAYER_TEMPLATE",
            Panel::Economy => "ECONOMY_PLAYER_TEMPLATE",
            Panel::Production => "PRODUCTION_PLAYER_TEMPLATE",
            Panel::SupportPowers => "SUPPORT_POWERS_PLAYER_TEMPLATE",
            Panel::Combat => "COMBAT_PLAYER_TEMPLATE",
            Panel::Army => "ARMY_PLAYER_TEMPLATE",
            _ => return None,
        })
    }

    fn headers(self) -> Option<&'static str> {
        Some(match self {
            Panel::Basic => "BASIC_STATS_HEADERS",
            Panel::Economy => "ECONOMY_STATS_HEADERS",
            Panel::Production => "PRODUCTION_STATS_HEADERS",
            Panel::SupportPowers => "SUPPORT_POWERS_HEADERS",
            Panel::Combat => "COMBAT_STATS_HEADERS",
            Panel::Army => "ARMY_HEADERS",
            _ => return None,
        })
    }
}

struct Stats {
    ingame: Rc<Ingame>,
    world: Rc<World>,
    players: Vec<PlayerRef>,
    teams: Vec<(i32, Vec<PlayerRef>)>,
    has_teams: bool,
    panel: WidgetId,
    templates: Vec<(Panel, WidgetId)>,
    team_template: WidgetId,
    dropdown: WidgetId,
    income_graph: WidgetId,
    army_graph: WidgetId,
    active: Rc<Cell<Panel>>,
    title: Rc<RefCell<String>>,
    titles: Vec<String>,
    hotkeys: Vec<String>,
    click_sound: String,
    /// `Ui.MouseOverWidget`, refreshed each tick for the color getters.
    mouse_over: Rc<Cell<Option<WidgetId>>>,
    projected_cells: i32,
}

fn text(f: impl Fn() -> String + 'static) -> Option<Getter<String>> {
    Some(Rc::new(f))
}

fn dollars(i: i32) -> String {
    format!("${i}")
}

/// `GetPowerColor`.
fn power_color(state: PowerState) -> Color {
    if state == PowerState::Critical {
        Color::from_argb(255, 255, 0, 0)
    } else if state == PowerState::Low {
        Color::from_argb(255, 255, 165, 0)
    } else {
        Color::from_argb(255, 50, 205, 50)
    }
}

impl Stats {
    fn template(&self, panel: Panel) -> Option<WidgetId> {
        self.templates.iter().find(|(p, _)| *p == panel).map(|(_, t)| *t)
    }

    fn scrollbar_offset(&self, ui: &mut Ui) -> i32 {
        let panel = ui.get::<ScrollPanelWidget>(self.panel);
        if panel.scrollbar == ScrollBar::Hidden {
            0
        } else {
            panel.scrollbar_width
        }
    }

    /// The `OnClick` of drop down option `index`.
    fn select(self: &Rc<Self>, ui: &mut Ui, index: usize) {
        let (panel, _, _) = PANELS[index];
        ui.remove_children(self.panel);
        ui.get::<LineGraphWidget>(self.income_graph).get_series = None;
        ui.get::<LineGraphWidget>(self.army_graph).get_series = None;
        *self.title.borrow_mut() = self.titles[index].clone();
        self.active.set(panel);
        if let Some(template) = self.template(panel) {
            let width = ui.node(template).bounds.w + self.scrollbar_offset(ui);
            ui.node_mut(self.panel).bounds.w = width;
            self.display_stats(ui, panel, template);
        }
        match panel {
            Panel::Graph => self.graph(ui, self.income_graph, |s| s.income_samples.borrow().clone()),
            Panel::ArmyGraph => self.graph(ui, self.army_graph, |s| s.army_samples.borrow().clone()),
            _ => {}
        }
        ui.reset_tooltips();
    }

    /// `IncomeGraph` and `ArmyValueGraph`.
    fn graph(&self, ui: &mut Ui, graph: WidgetId, samples: fn(&PlayerStatistics) -> Vec<i32>) {
        let players: Vec<(String, Color, Option<Rc<PlayerStatistics>>)> = self
            .players
            .iter()
            .map(|p| {
                let name = resolved_player_name(ui, &self.world, p);
                (name, p.color, p.player_actor().trait_or_default::<PlayerStatistics>())
            })
            .collect();
        ui.get::<LineGraphWidget>(graph).get_series = Some(Rc::new(move || {
            players
                .iter()
                .map(|(name, color, stats)| LineGraphSeries {
                    key: name.clone(),
                    color: *color,
                    points: stats
                        .as_ref()
                        .map_or_else(Vec::new, |s| samples(s).into_iter().map(|v| v as f32).collect()),
                })
                .collect()
        }));
    }

    /// `DisplayStats`.
    fn display_stats(self: &Rc<Self>, ui: &mut Ui, panel: Panel, template: WidgetId) {
        for (team, players) in &self.teams {
            if self.has_teams {
                self.add_team_row(ui, *team);
            }
            for player in players {
                let item = self.player_row(ui, panel, template, player);
                ui.add_child(self.panel, item);
            }
        }
    }

    fn add_team_row(&self, ui: &mut Ui, team: i32) {
        let tt = ScrollItemWidget::setup(ui, self.team_template, Rc::new(|| false), Rc::new(|_| {}));
        ui.node_mut(tt).ignore_mouse_over = true;
        let label = ui.child(tt, "TEAM");
        let team_text = if team > 0 {
            message_with(ui, "label-team-name", &[("team", MessageArg::Number(team as i64))])
        } else {
            message(ui, "label-no-team")
        };
        let font = ui.get::<ScrollItemWidget>(tt).button.font.clone();
        let width = ui.measure(&font, &team_text).0;
        ui.node_mut(tt).bounds.w = width;
        ui.node_mut(label).bounds.w = width;
        ui.label(label).get_text = text(move || team_text.clone());
        let bounds_width = ui.node(self.panel).bounds.w - self.scrollbar_offset(ui);
        let color = ui.child(tt, "TEAM_COLOR");
        ui.node_mut(color).bounds.w = bounds_width - 200;
        let gradient = ui.child(tt, "TEAM_GRADIENT");
        ui.node_mut(gradient).bounds.x = bounds_width - 200;
        ui.add_child(self.panel, tt);
    }

    /// `SetupPlayerScrollItemWidget`, `AddPlayerFlagAndName`, `SetupPlayerColor` and the panel specific rows.
    fn player_row(self: &Rc<Self>, ui: &mut Ui, panel: Panel, template: WidgetId, player: &PlayerRef) -> WidgetId {
        let (world, ingame, p) = (self.world.clone(), self.ingame.clone(), player.clone());
        let item = ScrollItemWidget::setup(
            ui,
            template,
            Rc::new(|| false),
            Rc::new(move |_| {
                let base = world
                    .actors_having_trait::<BaseBuilding>()
                    .into_iter()
                    .find(|a| !a.is_dead() && a.is_owned_by(&p));
                if let Some(base) = base {
                    let c = base.center_position();
                    ingame.viewport.borrow_mut().center_on((c.x * 24 / 1024, (c.y - c.z) * 24 / 1024));
                }
            }),
        );

        let flag = ui.child(item, "FLAG");
        {
            let faction = player.faction.clone();
            let mut f = ui.get::<ImageWidget>(flag);
            f.get_image_collection = Some(Rc::new(|| "flags".to_string()));
            f.get_image_name = Some(Rc::new(move || faction.clone()));
        }
        let name_label = ui.child(item, "PLAYER");
        let name = resolved_player_name(ui, &self.world, player);
        let net = services(ui).net.clone();
        let name_text = player_name_and_status(ui, name_label, &self.world, player, &name, &net);
        {
            let mut l = ui.label(name_label);
            l.get_text = Some(name_text);
            l.get_color = Some(Rc::new(|| Color::from_argb(255, 255, 255, 255)));
        }
        self.setup_player_color(ui, item, player.color);

        let get_player: Getter<Option<PlayerRef>> = {
            let p = player.clone();
            Rc::new(move || Some(p.clone()))
        };
        match panel {
            Panel::Basic => self.basic_stats(ui, item, player),
            Panel::Economy => self.economy_stats(ui, item, player),
            Panel::Combat => self.combat_stats(ui, item, player),
            Panel::Production => {
                let icons = ui.child(item, "PRODUCTION_ICONS");
                ui.get::<ObserverProductionIconsWidget>(icons).get_player = Some(get_player);
                ui.node_mut(item).ignore_child_mouse_over = false;
            }
            Panel::SupportPowers => {
                let icons = ui.child(item, "SUPPORT_POWER_ICONS");
                ui.get::<ObserverSupportPowerIconsWidget>(icons).get_player = Some(get_player);
                ui.node_mut(item).ignore_child_mouse_over = false;
            }
            Panel::Army => {
                let icons = ui.child(item, "ARMY_ICONS");
                ui.get::<ObserverArmyIconsWidget>(icons).get_player = Some(get_player);
                ui.node_mut(item).ignore_child_mouse_over = false;
            }
            _ => {}
        }
        item
    }

    /// `SetupPlayerColor`.
    fn setup_player_color(&self, ui: &mut Ui, item: WidgetId, player_color: Color) {
        let color = Color::from_argb(128, player_color.r(), player_color.g(), player_color.b());
        let hover_color = Color::from_argb(192, player_color.r(), player_color.g(), player_color.b());
        let children = ui.node(item).children.clone();
        let mouse_over = self.mouse_over.clone();
        let is_over = Rc::new(move || mouse_over.get().is_some_and(|w| w == item || children.contains(&w)));
        let pick = |base: Color| -> Getter<Color> {
            let is_over = is_over.clone();
            Rc::new(move || if is_over() { hover_color } else { base })
        };
        let block = ui.child(item, "PLAYER_COLOR");
        ui.get::<ColorBlockWidget>(block).get_color = Some(pick(color));
        let gradient = ui.child(item, "PLAYER_GRADIENT");
        let transparent = Color::from_argb(0, 0, 0, 0);
        ui.get::<GradientColorBlockWidget>(gradient)
            .set_colors(pick(color), pick(transparent), pick(transparent), pick(color));
    }

    fn set_text(ui: &mut Ui, item: WidgetId, name: &str, f: impl Fn() -> String + 'static) {
        if let Some(label) = ui.find(item, name) {
            ui.label(label).get_text = text(f);
        }
    }

    /// `BasicStats`.
    fn basic_stats(&self, ui: &mut Ui, item: WidgetId, player: &PlayerRef) {
        let pa = player.player_actor();
        if let Some(res) = pa.trait_or_default::<PlayerResources>() {
            Self::set_text(ui, item, "CASH", move || dollars(res.get_cash_and_resources()));
        }
        if let Some(power) = pa.trait_or_default::<PowerManager>() {
            let label = ui.child(item, "POWER");
            let (p, p2) = (power.clone(), power);
            let mut l = ui.label(label);
            l.get_text = text(move || format!("{}/{}", p.power_drained(), p.power_provided()));
            l.get_color = Some(Rc::new(move || power_color(p2.power_state())));
        }
        let Some(stats) = pa.trait_or_default::<PlayerStatistics>() else {
            return;
        };
        let s = stats.clone();
        Self::set_text(ui, item, "KILLS", move || {
            (s.units_killed.get() + s.buildings_killed.get()).to_string()
        });
        let s = stats.clone();
        Self::set_text(ui, item, "DEATHS", move || {
            (s.units_dead.get() + s.buildings_dead.get()).to_string()
        });
        let s = stats.clone();
        Self::set_text(ui, item, "ASSETS_DESTROYED", move || dollars(s.kills_cost.get()));
        let s = stats.clone();
        Self::set_text(ui, item, "ASSETS_LOST", move || dollars(s.deaths_cost.get()));
        let s = stats.clone();
        Self::set_text(ui, item, "EXPERIENCE", move || s.experience().to_string());
        let world = self.world.clone();
        Self::set_text(ui, item, "ACTIONS_MIN", move || {
            let tick = world.world_tick();
            let orders = stats.order_count.get() as f64;
            let average = if tick == 0 { 0.0 } else { orders / (tick as f64 / 1500.0) };
            format!("{average:.1}")
        });
    }

    /// `EconomyStats`.
    fn economy_stats(&self, ui: &mut Ui, item: WidgetId, player: &PlayerRef) {
        let pa = player.player_actor();
        let (Some(stats), Some(res)) = (pa.trait_or_default::<PlayerStatistics>(), pa.trait_or_default::<PlayerResources>()) else {
            return;
        };
        let r = res.clone();
        Self::set_text(ui, item, "CASH", move || dollars(r.get_cash_and_resources()));
        let s = stats.clone();
        Self::set_text(ui, item, "INCOME", move || dollars(s.display_income.get()));
        let r = res.clone();
        Self::set_text(ui, item, "EARNED", move || dollars(r.earned.get()));
        Self::set_text(ui, item, "SPENT", move || dollars(res.spent.get()));
        Self::set_text(ui, item, "ASSETS", move || dollars(stats.assets_value.get()));
        let (world, p) = (self.world.clone(), player.clone());
        Self::set_text(ui, item, "DERRICKS", move || {
            world
                .actors()
                .iter()
                .filter(|a| a.is_owned_by(&p) && !a.is_dead() && a.info.has_trait_info::<UpdatesDerrickCountInfo>())
                .count()
                .to_string()
        });
        let (world, p) = (self.world.clone(), player.clone());
        Self::set_text(ui, item, "HARVESTERS", move || {
            world
                .actors_with_trait::<Harvester>()
                .iter()
                .filter(|(a, h)| a.is_owned_by(&p) && !a.is_dead() && !h.is_trait_disabled())
                .count()
                .to_string()
        });
    }

    /// `CombatStats`.
    fn combat_stats(&self, ui: &mut Ui, item: WidgetId, player: &PlayerRef) {
        let Some(stats) = player.player_actor().trait_or_default::<PlayerStatistics>() else {
            return;
        };
        let fields: [(&str, fn(&PlayerStatistics) -> String); 7] = [
            ("ASSETS_DESTROYED", |s| dollars(s.kills_cost.get())),
            ("ASSETS_LOST", |s| dollars(s.deaths_cost.get())),
            ("UNITS_KILLED", |s| s.units_killed.get().to_string()),
            ("UNITS_DEAD", |s| s.units_dead.get().to_string()),
            ("BUILDINGS_KILLED", |s| s.buildings_killed.get().to_string()),
            ("BUILDINGS_DEAD", |s| s.buildings_dead.get().to_string()),
            ("ARMY_VALUE", |s| dollars(s.army_value.get())),
        ];
        for (name, f) in fields {
            let s = stats.clone();
            Self::set_text(ui, item, name, move || f(&s));
        }
        let (p, total) = (player.clone(), self.projected_cells.max(1));
        Self::set_text(ui, item, "VISION", move || {
            let shroud = p.shroud();
            if shroud.disabled() {
                return "100%".into();
            }
            let percent = (shroud.revealed_cells.get() as f64 * 100.0 / total as f64).ceil();
            format!("{percent}%")
        });
    }

    /// The `STATS_DROPDOWN_KEYHANDLER` handler.
    fn handle_key(self: &Rc<Self>, ui: &mut Ui, e: &KeyInput) -> bool {
        if e.event != KeyEvent::Down || e.repeat {
            return false;
        }
        let index = {
            let assets = ui.assets.borrow();
            self.hotkeys.iter().position(|h| !h.is_empty() && assets.hotkeys.is_activated(h, e))
        };
        let Some(index) = index else { return false };
        rustora_engine::sound::play_notification(&self.world, None, "Sounds", &self.click_sound, None);
        self.select(ui, index);
        true
    }
}

impl ChromeLogic for Rc<Stats> {
    fn tick(&mut self, ui: &mut Ui) {
        self.mouse_over.set(ui.mouse_over);
    }
}

/// `AdjustHeader`.
fn adjust_header(ui: &mut Ui, headers: WidgetId, offset: i32) {
    let color = ui.child(headers, "HEADER_COLOR");
    ui.node_mut(color).bounds.w += offset;
    let gradient = ui.child(headers, "HEADER_GRADIENT");
    ui.node_mut(gradient).bounds.x += offset;
    for child in ui.node(headers).children.clone() {
        if ui.kind(child).borrow_mut().as_any().is::<crate::widgets::LabelWidget>() {
            ui.node_mut(child).bounds.x += offset;
        }
    }
}

/// `ObserverStatsLogic`.
pub fn create(ui: &mut Ui, widget: WidgetId, logic_args: &[MiniYamlNode]) -> Rc<RefCell<dyn ChromeLogic>> {
    let Some(ingame) = services(ui).ingame.borrow().clone() else {
        return cell(NoLogic);
    };
    let world = ingame.world.clone();
    let arg = |key: &str| logic_args.iter().find(|n| n.key == key).and_then(|n| n.value.value.clone());
    let hotkeys = PANELS
        .iter()
        .map(|(_, name, _)| arg(&format!("Statistics{name}Key")).unwrap_or_default())
        .collect();

    let players: Vec<PlayerRef> = world.players().into_iter().filter(|p| !p.non_combatant && p.playable).collect();
    let mut teams: Vec<(i32, Vec<PlayerRef>)> = Vec::new();
    for p in players.iter().cloned() {
        let team = world.lobby_info.client_with_index(p.client_index).map_or(0, |c| c.team);
        match teams.iter_mut().find(|(t, _)| *t == team) {
            Some((_, list)) => list.push(p),
            None => teams.push((team, vec![p])),
        }
    }
    teams.sort_by_key(|(t, _)| *t);
    let has_teams = !(teams.len() == 1 && teams[0].0 == 0);
    let player_count = players.len();

    let panel = ui.child(widget, "PLAYER_STATS_PANEL");
    ui.set_grid_layout(panel);
    ui.node_mut(panel).ignore_mouse_over = true;
    let active = Rc::new(Cell::new(Panel::None));
    if player_count + if has_teams { teams.len() } else { 0 } > 10 {
        let width = {
            let mut p = ui.get::<ScrollPanelWidget>(panel);
            p.scrollbar = ScrollBar::Left;
            p.scrollbar_width
        };
        for (p, ..) in PANELS {
            if let Some(headers) = p.headers() {
                let headers = ui.child(widget, headers);
                adjust_header(ui, headers, width);
            }
        }
    }
    for (p, ..) in PANELS {
        if let Some(headers) = p.headers() {
            let headers = ui.child(widget, headers);
            let a = active.clone();
            ui.set_visible_fn(headers, move || a.get() == p);
        }
    }
    let a = active.clone();
    ui.set_visible_fn(panel, move || a.get().template().is_some());

    let mut templates = Vec::new();
    for (p, ..) in PANELS {
        if let Some(name) = p.template() {
            let t = ui.child(panel, name);
            ui.hide_child(panel, t);
            templates.push((p, t));
        }
    }
    let team_template = ui.child(panel, "TEAM_TEMPLATE");
    ui.hide_child(panel, team_template);

    let graph = |ui: &mut Ui, container: &str, graph: &str, p: Panel| {
        let container = ui.child(widget, container);
        let a = active.clone();
        ui.set_visible_fn(container, move || a.get() == p);
        ui.child(container, graph)
    };
    let income_graph = graph(ui, "INCOME_GRAPH_CONTAINER", "INCOME_GRAPH", Panel::Graph);
    let army_graph = graph(ui, "ARMY_VALUE_GRAPH_CONTAINER", "ARMY_VALUE_GRAPH", Panel::ArmyGraph);

    let dropdown = ui.child(widget, "STATS_DROPDOWN");
    let titles: Vec<String> = PANELS.iter().map(|(_, _, key)| message(ui, key)).collect();
    let projected_cells = world.map().projected_cells().count() as i32;
    let click_sound = arg("ClickSound").unwrap_or_else(|| ui.assets.borrow().metrics.string("ClickSound"));
    let stats = Rc::new(Stats {
        ingame: ingame.clone(),
        world: world.clone(),
        players,
        teams,
        has_teams,
        panel,
        templates,
        team_template,
        dropdown,
        income_graph,
        army_graph,
        active: active.clone(),
        title: Rc::new(RefCell::new(String::new())),
        titles,
        hotkeys,
        click_sound,
        mouse_over: Rc::default(),
        projected_cells,
    });

    let title = stats.title.clone();
    ui.button(dropdown).get_text = text(move || title.borrow().clone());
    let template = arg("StatsDropDownPanelTemplate").unwrap_or_else(|| "LABEL_DROPDOWN_TEMPLATE".into());
    {
        let s = stats.clone();
        ui.button(dropdown).on_mouse_down = Some(Rc::new(move |ui: &mut Ui, _| {
            let s2 = s.clone();
            let setup = move |ui: &mut Ui, index: usize, item_template: WidgetId| {
                let (s3, s4) = (s2.clone(), s2.clone());
                let item = ScrollItemWidget::setup(
                    ui,
                    item_template,
                    Rc::new(move || s3.active.get() == PANELS[index].0),
                    Rc::new(move |ui: &mut Ui| s4.select(ui, index)),
                );
                let label = ui.child(item, "LABEL");
                let title = s2.titles[index].clone();
                ui.label(label).get_text = text(move || title.clone());
                item
            };
            DropDownButtonWidget::show_dropdown(ui, s.dropdown, &template, 230, 0..PANELS.len(), setup);
        }));
    }
    stats.select(ui, 0);

    let key_handler = ui.child(dropdown, "STATS_DROPDOWN_KEYHANDLER");
    let s = stats.clone();
    ui.get::<LogicKeyListenerWidget>(key_handler)
        .handlers
        .push(Rc::new(move |ui: &mut Ui, e: &KeyInput| s.handle_key(ui, e)));
    cell(stats)
}
