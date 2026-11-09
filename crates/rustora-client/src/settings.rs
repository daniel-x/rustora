// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Settings.cs`: the user settings in `settings.yaml` of the support dir.
//!
//! Hotkeys stay as written in the file until the hotkey logic is ported.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use rustora_core::fieldloader::Color;
use rustora_core::miniyaml::{MiniYaml, MiniYamlNode, nodes_to_lines};

/// A settings field type: `FieldLoader.GetValue` and `FieldSaver.FormatValue`.
pub trait SettingValue: Sized {
    fn parse_setting(value: &str) -> Option<Self>;
    fn format_setting(&self) -> String;
}

impl SettingValue for bool {
    fn parse_setting(value: &str) -> Option<bool> {
        match value.trim().to_ascii_lowercase().as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        }
    }

    fn format_setting(&self) -> String {
        if *self { "True".into() } else { "False".into() }
    }
}

impl SettingValue for i32 {
    fn parse_setting(value: &str) -> Option<i32> {
        value.trim().parse().ok()
    }

    fn format_setting(&self) -> String {
        self.to_string()
    }
}

impl SettingValue for f32 {
    fn parse_setting(value: &str) -> Option<f32> {
        value.trim().parse().ok()
    }

    fn format_setting(&self) -> String {
        self.to_string()
    }
}

impl SettingValue for String {
    fn parse_setting(value: &str) -> Option<String> {
        Some(value.to_string())
    }

    fn format_setting(&self) -> String {
        self.clone()
    }
}

impl SettingValue for Option<String> {
    fn parse_setting(value: &str) -> Option<Option<String>> {
        Some(if value.is_empty() { None } else { Some(value.to_string()) })
    }

    fn format_setting(&self) -> String {
        self.clone().unwrap_or_default()
    }
}

impl SettingValue for Color {
    fn parse_setting(value: &str) -> Option<Color> {
        Color::parse(value.trim())
    }

    fn format_setting(&self) -> String {
        if self.a() == 255 {
            format!("{:02X}{:02X}{:02X}", self.r(), self.g(), self.b())
        } else {
            format!("{:02X}{:02X}{:02X}{:02X}", self.r(), self.g(), self.b(), self.a())
        }
    }
}

impl SettingValue for (i32, i32) {
    fn parse_setting(value: &str) -> Option<(i32, i32)> {
        let (x, y) = value.split_once(',')?;
        Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
    }

    fn format_setting(&self) -> String {
        format!("{},{}", self.0, self.1)
    }
}

impl<T: SettingValue> SettingValue for Vec<T> {
    fn parse_setting(value: &str) -> Option<Vec<T>> {
        value
            .split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(T::parse_setting)
            .collect()
    }

    fn format_setting(&self) -> String {
        self.iter().map(T::format_setting).collect::<Vec<_>>().join(", ")
    }
}

macro_rules! setting_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum $name { $($variant),+ }

        impl SettingValue for $name {
            fn parse_setting(value: &str) -> Option<$name> {
                let value = value.trim();
                $(if value.eq_ignore_ascii_case(stringify!($variant)) {
                    return Some($name::$variant);
                })+
                None
            }

            fn format_setting(&self) -> String {
                match self { $($name::$variant => stringify!($variant).to_string()),+ }
            }
        }
    };
}

setting_enum!(MouseScrollType {
    Disabled,
    Standard,
    Inverted,
    Joystick
});
setting_enum!(StatusBarsType {
    Standard,
    DamageShow,
    AlwaysShow
});
setting_enum!(TargetLinesType {
    Disabled,
    Manual,
    Automatic
});
setting_enum!(WorldViewport {
    Native,
    Close,
    Medium,
    Far
});
setting_enum!(WindowMode {
    Windowed,
    Fullscreen,
    PseudoFullscreen
});
setting_enum!(GlProfile {
    Automatic,
    ANGLE,
    Modern,
    Embedded
});

macro_rules! setting_flags {
    ($(#[$meta:meta])* $name:ident { $($flag:ident = $bit:expr),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct $name(pub u32);

        #[allow(non_upper_case_globals)]
        impl $name {
            pub const None: $name = $name(0);
            $(pub const $flag: $name = $name($bit);)+

            pub fn has(self, flag: $name) -> bool {
                self.0 & flag.0 == flag.0
            }
        }

        impl SettingValue for $name {
            fn parse_setting(value: &str) -> Option<$name> {
                let mut bits = 0;
                for part in value.split(',').map(str::trim) {
                    if part.eq_ignore_ascii_case("None") {
                        continue;
                    }
                    bits |= [$((stringify!($flag), $bit)),+]
                        .iter()
                        .find(|(name, _)| part.eq_ignore_ascii_case(name))?
                        .1;
                }
                Some($name(bits))
            }

            fn format_setting(&self) -> String {
                let names: Vec<&str> = [$((stringify!($flag), $bit)),+]
                    .iter()
                    .filter(|(_, bit)| self.0 & bit != 0)
                    .map(|(name, _)| *name)
                    .collect();
                if names.is_empty() { "None".into() } else { names.join(", ") }
            }
        }
    };
}

setting_flags!(
    /// `MPGameFilters`.
    MpGameFilters { Waiting = 1, Empty = 2, Protected = 4, Started = 8, Incompatible = 16 }
);
setting_flags!(
    /// `TextNotificationPoolFilters`.
    TextNotificationPoolFilters { Feedback = 1, Transients = 2 }
);
setting_flags!(
    /// `Modifiers` of the input handler.
    ModifierFlags { Shift = 1, Alt = 2, Ctrl = 4, Meta = 8 }
);

/// A field value that its type cannot parse.
#[derive(Debug)]
pub struct InvalidValue;

/// One section of the settings file.
pub trait SettingsSection {
    /// The yaml name and the formatted value of each field, in declaration order.
    fn fields(&self) -> Vec<(&'static str, String)>;

    /// Sets a field. `Ok(false)` means the section has no such field.
    fn load_field(&mut self, key: &str, value: &str) -> Result<bool, InvalidValue>;
}

macro_rules! section {
    ($(#[$meta:meta])* $name:ident { $($yaml:literal $field:ident : $ty:ty = $default:expr),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Debug)]
        pub struct $name { $(pub $field: $ty),+ }

        impl Default for $name {
            fn default() -> $name {
                $name { $($field: $default),+ }
            }
        }

        impl SettingsSection for $name {
            fn fields(&self) -> Vec<(&'static str, String)> {
                vec![$(($yaml, self.$field.format_setting())),+]
            }

            fn load_field(&mut self, key: &str, value: &str) -> Result<bool, InvalidValue> {
                match key {
                    $($yaml => {
                        self.$field = <$ty>::parse_setting(value).ok_or(InvalidValue)?;
                        Ok(true)
                    })+
                    _ => Ok(false),
                }
            }
        }
    };
}

section!(
    /// `ServerSettings`.
    ServerSection {
        "Name" name: String = String::new(),
        "ListenPort" listen_port: i32 = 1234,
        "AdvertiseOnline" advertise_online: bool = true,
        "Password" password: String = String::new(),
        "DiscoverNatDevices" discover_nat_devices: bool = false,
        "NatPortMappingLifetime" nat_port_mapping_lifetime: i32 = 36000,
        "Map" map: Option<String> = None,
        "Ban" ban: Vec<String> = Vec::new(),
        "RequireAuthentication" require_authentication: bool = false,
        "ProfileIDWhitelist" profile_id_whitelist: Vec<i32> = Vec::new(),
        "ProfileIDBlacklist" profile_id_blacklist: Vec<i32> = Vec::new(),
        "EnableSingleplayer" enable_singleplayer: bool = false,
        "QueryMapRepository" query_map_repository: bool = true,
        "EnableSyncReports" enable_sync_reports: bool = false,
        "TimestampFormat" timestamp_format: String = "yyyy-MM-ddTHH:mm:ss".into(),
        "ShareAnonymizedIPs" share_anonymized_ips: bool = true,
        "EnableGeoIP" enable_geo_ip: bool = true,
        "RecordReplays" record_replays: bool = false,
        "EnableLintChecks" enable_lint_checks: bool = true,
        "MapPool" map_pool: Vec<String> = Vec::new(),
        "FloodLimitJoinCooldown" flood_limit_join_cooldown: i32 = 5000,
        "FloodLimitInterval" flood_limit_interval: i32 = 5000,
        "FloodLimitMessageCount" flood_limit_message_count: i32 = 5,
        "FloodLimitCooldown" flood_limit_cooldown: i32 = 15000,
        "EnableVoteKick" enable_vote_kick: bool = true,
        "VoteKickTimer" vote_kick_timer: i32 = 30000,
        "VoteKickerCooldown" vote_kicker_cooldown: i32 = 120000,
    }
);

section!(
    /// `DebugSettings`.
    DebugSection {
        "PerfText" perf_text: bool = false,
        "PerfGraph" perf_graph: bool = false,
        "Samples" samples: i32 = 25,
        "CheckVersion" check_version: bool = true,
        "SendSystemInformation" send_system_information: bool = true,
        "SystemInformationVersionPrompt" system_information_version_prompt: i32 = 0,
        "UUID" uuid: String = new_uuid(),
        "DisplayDeveloperSettings" display_developer_settings: bool = false,
        "BotDebug" bot_debug: bool = false,
        "LuaDebug" lua_debug: bool = false,
        "EnableDebugCommandsInReplays" enable_debug_commands_in_replays: bool = false,
        "EnableSimulationPerfLogging" enable_simulation_perf_logging: bool = false,
        "LongTickThresholdMs" long_tick_threshold_ms: f32 = 1.0,
        "SyncCheckUnsyncedCode" sync_check_unsynced_code: bool = false,
        "SyncCheckBotModuleCode" sync_check_bot_module_code: bool = false,
    }
);

section!(
    /// `GraphicSettings`.
    GraphicsSection {
        "Mode" mode: WindowMode = WindowMode::PseudoFullscreen,
        "VSync" vsync: bool = true,
        "FullscreenSize" fullscreen_size: (i32, i32) = (0, 0),
        "WindowedSize" windowed_size: (i32, i32) = (1024, 768),
        "CursorDouble" cursor_double: bool = false,
        "ViewportDistance" viewport_distance: WorldViewport = WorldViewport::Medium,
        "UIScale" ui_scale: f32 = 1.0,
        "CapFramerate" cap_framerate: bool = false,
        "MaxFramerate" max_framerate: i32 = 60,
        "CapFramerateToGameFps" cap_framerate_to_game_fps: bool = false,
        "DisableGLDebugMessageCallback" disable_gl_debug_message_callback: bool = false,
        "DisableHardwareCursors" disable_hardware_cursors: bool = false,
        "VideoDisplay" video_display: i32 = 0,
        "GLProfile" gl_profile: GlProfile = GlProfile::Automatic,
        "BatchSize" batch_size: i32 = 8192,
        "SheetSize" sheet_size: i32 = 2048,
    }
);

section!(
    /// `SoundSettings`.
    SoundSection {
        "SoundVolume" sound_volume: f32 = 0.5,
        "MusicVolume" music_volume: f32 = 0.5,
        "VideoVolume" video_volume: f32 = 0.5,
        "Shuffle" shuffle: bool = false,
        "Repeat" repeat: bool = false,
        "Device" device: Option<String> = None,
        "CashTicks" cash_ticks: bool = true,
        "Mute" mute: bool = false,
        "MuteBackgroundMusic" mute_background_music: bool = false,
    }
);

section!(
    /// `PlayerSettings`.
    PlayerSection {
        "Name" name: String = DEFAULT_PLAYER_NAME.into(),
        "Color" color: Color = Color::parse("C82020").expect("valid color"),
        "LastServer" last_server: String = "localhost:1234".into(),
        "CustomColors" custom_colors: Vec<Color> = Vec::new(),
    }
);

section!(
    /// `GameSettings`.
    GameSection {
        "Platform" platform: String = "Default".into(),
        "ViewportEdgeScroll" viewport_edge_scroll: bool = true,
        "ViewportEdgeScrollMargin" viewport_edge_scroll_margin: i32 = 5,
        "LockMouseWindow" lock_mouse_window: bool = false,
        "MouseScroll" mouse_scroll: MouseScrollType = MouseScrollType::Joystick,
        "ViewportEdgeScrollStep" viewport_edge_scroll_step: f32 = 30.0,
        "UIScrollSpeed" ui_scroll_speed: f32 = 50.0,
        "ZoomSpeed" zoom_speed: f32 = 0.04,
        "SelectionDeadzone" selection_deadzone: i32 = 24,
        "MouseScrollDeadzone" mouse_scroll_deadzone: i32 = 8,
        "UseClassicMouseStyle" use_classic_mouse_style: bool = false,
        "UseAlternateScrollButton" use_alternate_scroll_button: bool = false,
        "HideReplayChat" hide_replay_chat: bool = false,
        "StatusBars" status_bars: StatusBarsType = StatusBarsType::Standard,
        "TargetLines" target_lines: TargetLinesType = TargetLinesType::Manual,
        "UsePlayerStanceColors" use_player_stance_colors: bool = false,
        "AllowDownloading" allow_downloading: bool = true,
        "AuthProfile" auth_profile: String = "player.oraid".into(),
        "ZoomModifier" zoom_modifier: ModifierFlags = ModifierFlags::None,
        "FetchNews" fetch_news: bool = true,
        "IntroductionPromptVersion" introduction_prompt_version: i32 = 0,
        "MPGameFilters" mp_game_filters: MpGameFilters = MpGameFilters(1 | 2 | 4 | 8),
        "PauseShellmap" pause_shellmap: bool = false,
        "EnableDiscordService" enable_discord_service: bool = true,
        "TextNotificationPoolFilters" text_notification_pool_filters: TextNotificationPoolFilters =
            TextNotificationPoolFilters(1 | 2),
    }
);

const DEFAULT_PLAYER_NAME: &str = "Commander";

/// A random version 4 UUID, as `Guid.NewGuid().ToString()` writes it.
fn new_uuid() -> String {
    let word = || RandomState::new().build_hasher().finish();
    let (a, b) = (word(), word());
    let hex = format!("{a:016x}{b:016x}");
    let variant = ['8', '9', 'a', 'b'][(b >> 60) as usize & 3];
    format!(
        "{}-{}-4{}-{variant}{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[13..16],
        &hex[17..20],
        &hex[20..32]
    )
}

/// The settings of the client. Unknown keys of the file survive a save.
#[derive(Clone, Debug)]
pub struct Settings {
    file: PathBuf,
    pub player: PlayerSection,
    pub game: GameSection,
    pub sound: SoundSection,
    pub graphics: GraphicsSection,
    pub server: ServerSection,
    pub debug: DebugSection,
    pub keys: IndexMap<String, String>,
    yaml_cache: Vec<MiniYamlNode>,
}

impl Settings {
    /// Loads `file` if it exists and applies the command line arguments of the form `Section.Field=value`.
    /// Invalid values keep their default and unknown fields are ignored, each with a log line.
    pub fn load(file: impl Into<PathBuf>, args: &[String]) -> Settings {
        let file = file.into();
        let mut settings = Settings {
            player: Default::default(),
            game: Default::default(),
            sound: Default::default(),
            graphics: Default::default(),
            server: Default::default(),
            debug: Default::default(),
            keys: IndexMap::new(),
            yaml_cache: read_yaml(&file),
            file,
        };
        let cache = std::mem::take(&mut settings.yaml_cache);
        for node in &cache {
            if node.key == "Keys" {
                continue;
            }
            if let Some(section) = settings.section_mut(&node.key) {
                for field in &node.value.nodes {
                    load_field(section, &node.key, &field.key, field.value.value_str());
                }
            }
        }
        if let Some(keys) = cache.iter().find(|n| n.key == "Keys") {
            for node in keys.value.nodes.iter().filter(|n| !n.key.is_empty()) {
                settings.keys.insert(node.key.clone(), node.value.value_str().to_string());
            }
        }
        settings.yaml_cache = cache;
        for arg in args {
            let Some((name, value)) = arg.split_once('=') else {
                continue;
            };
            let Some((section_name, field)) = name.split_once('.') else {
                continue;
            };
            if let Some(section) = settings.section_mut(section_name) {
                load_field(section, section_name, field, value);
            }
        }
        settings
    }

    fn section_mut(&mut self, name: &str) -> Option<&mut dyn SettingsSection> {
        Some(match name {
            "Player" => &mut self.player,
            "Game" => &mut self.game,
            "Sound" => &mut self.sound,
            "Graphics" => &mut self.graphics,
            "Server" => &mut self.server,
            "Debug" => &mut self.debug,
            _ => return None,
        })
    }

    /// Merges the current values over the file as it was loaded and writes it. Values equal to their default are
    /// not written.
    pub fn save(&mut self) -> std::io::Result<()> {
        let sections: [(&str, Vec<(&str, String)>, Vec<(&str, String)>); 6] = [
            ("Player", self.player.fields(), PlayerSection::default().fields()),
            ("Game", self.game.fields(), GameSection::default().fields()),
            ("Sound", self.sound.fields(), SoundSection::default().fields()),
            ("Graphics", self.graphics.fields(), GraphicsSection::default().fields()),
            ("Server", self.server.fields(), ServerSection::default().fields()),
            ("Debug", self.debug.fields(), DebugSection::default().fields()),
        ];
        for (name, fields, defaults) in &sections {
            let section = node_or_add(&mut self.yaml_cache, name);
            for ((field, value), (_, default)) in fields.iter().zip(defaults) {
                if value == default {
                    section.value.nodes.retain(|n| n.key != *field);
                } else if let Some(node) = section.value.nodes.iter_mut().find(|n| n.key == *field) {
                    node.value.value = Some(value.clone());
                } else {
                    section.value.nodes.push(MiniYamlNode::with_value_str(*field, value.clone()));
                }
            }
        }
        let keys = node_or_add(&mut self.yaml_cache, "Keys");
        keys.value.nodes = self
            .keys
            .iter()
            .map(|(k, v)| MiniYamlNode::with_value_str(k.clone(), v.clone()))
            .collect();

        let mut lines = Vec::new();
        nodes_to_lines(&self.yaml_cache, &mut lines);
        let mut text = String::new();
        for line in &lines {
            text.push_str(line.trim_end());
            text.push('\n');
        }
        if let Some(dir) = self.file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&self.file, text)
    }

    /// `SanitizedServerName`.
    pub fn sanitized_server_name(&self, dirty: &str) -> String {
        let clean = sanitized_name(dirty);
        if clean.trim().is_empty() {
            format!("{}'s Game", sanitized_player_name(&self.player.name))
        } else {
            clean
        }
    }
}

fn read_yaml(file: &Path) -> Vec<MiniYamlNode> {
    let Ok(data) = std::fs::read(file) else {
        return Vec::new();
    };
    MiniYaml::from_bytes(&data, &file.to_string_lossy(), false).unwrap_or_else(|e| {
        log::warn!("Ignoring invalid settings file: {e}");
        Vec::new()
    })
}

fn load_field(section: &mut dyn SettingsSection, section_name: &str, key: &str, value: &str) {
    match section.load_field(key, value) {
        Ok(true) => {}
        Ok(false) => log::warn!("Ignoring unknown field `{key}` on `{section_name}`"),
        Err(InvalidValue) => log::warn!("Cannot parse `{value}` into `{section_name}.{key}`; keeping the current value"),
    }
}

fn node_or_add<'a>(nodes: &'a mut Vec<MiniYamlNode>, key: &str) -> &'a mut MiniYamlNode {
    let index = nodes.iter().position(|n| n.key == key).unwrap_or_else(|| {
        nodes.push(MiniYamlNode::with_value_str(key, ""));
        nodes.len() - 1
    });
    &mut nodes[index]
}

fn sanitized_name(dirty: &str) -> String {
    const DISALLOWED: &[char] = &['#', '@', ':', '\n', '\t', '[', ']', '{', '}', '<', '>', '"', '`'];
    dirty.chars().filter(|c| !DISALLOWED.contains(c)).collect()
}

/// `SanitizedPlayerName`.
pub fn sanitized_player_name(dirty: &str) -> String {
    let clean = sanitized_name(dirty);
    let clean = if clean.trim().is_empty() || clean == "Open" || clean == "Closed" {
        DEFAULT_PLAYER_NAME.to_string()
    } else {
        clean
    };
    clean
        .encode_utf16()
        .take(16)
        .map(|u| char::from_u32(u as u32).unwrap_or('?'))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rustora-settings-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("settings.yaml")
    }

    #[test]
    fn saves_only_changes_and_keeps_unknown_yaml() {
        let file = temp_file("merge");
        std::fs::write(
            &file,
            "Game:\n\tUIScrollSpeed: 70\n\tFutureField: 3\nFuture:\n\tKey: value\nPlayer:\n\tName: Tanya\n",
        )
        .unwrap();
        let mut settings = Settings::load(&file, &[]);
        assert_eq!(settings.game.ui_scroll_speed, 70.0);
        assert_eq!(settings.player.name, "Tanya");
        settings.game.ui_scroll_speed = 50.0;
        settings.sound.mute = true;
        settings.keys.insert("MuteAudioKey".into(), "M Ctrl".into());
        settings.save().unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(!text.contains("UIScrollSpeed"), "{text}");
        assert!(text.contains("\tFutureField: 3\n"), "{text}");
        assert!(text.contains("Future:\n\tKey: value\n"), "{text}");
        assert!(text.contains("Sound:\n\tMute: True\n"), "{text}");
        assert!(text.contains("Keys:\n\tMuteAudioKey: M Ctrl\n"), "{text}");
        let again = Settings::load(&file, &[]);
        assert!(again.sound.mute);
        assert_eq!(again.keys["MuteAudioKey"], "M Ctrl");
    }

    #[test]
    fn uuid_stays_stable_across_saves() {
        let file = temp_file("uuid");
        let mut settings = Settings::load(&file, &[]);
        settings.save().unwrap();
        let uuid = settings.debug.uuid.clone();
        assert_eq!(uuid.len(), 36);
        assert_eq!(Settings::load(&file, &[]).debug.uuid, uuid);
    }

    #[test]
    fn invalid_values_keep_the_default_and_args_override() {
        let file = temp_file("invalid");
        std::fs::write(&file, "Game:\n\tMouseScroll: Sideways\n\tFetchNews: maybe\n").unwrap();
        let args = [
            "Game.FetchNews=False".to_string(),
            "Debug.Samples=3".to_string(),
            "other".to_string(),
        ];
        let settings = Settings::load(&file, &args);
        assert_eq!(settings.game.mouse_scroll, MouseScrollType::Joystick);
        assert!(!settings.game.fetch_news);
        assert_eq!(settings.debug.samples, 3);
    }

    #[test]
    fn formats_match_the_field_saver() {
        assert_eq!(MpGameFilters(1 | 8).format_setting(), "Waiting, Started");
        assert_eq!(MpGameFilters::parse_setting("Waiting, Started"), Some(MpGameFilters(9)));
        assert_eq!(ModifierFlags::None.format_setting(), "None");
        assert_eq!(0.04f32.format_setting(), "0.04");
        assert_eq!(true.format_setting(), "True");
        assert_eq!(PlayerSection::default().color.format_setting(), "C82020");
    }

    #[test]
    fn sanitizes_names() {
        assert_eq!(sanitized_player_name("Open"), "Commander");
        assert_eq!(sanitized_player_name("a:b#c"), "abc");
        assert_eq!(sanitized_player_name("12345678901234567890"), "1234567890123456");
    }
}
