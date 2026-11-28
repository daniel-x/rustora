// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `OpenRA.Support.Log`: named log channels, each written to its own file in `<support dir>/rustora/Logs`.
//!
//! The channels are a `log` logger. A record goes to the channel named by its target, like
//! `log::info!(target: "lua", ...)`. Records with a module path as target go to the channel of their crate:
//! `server` for the server, `sound` for audio, `graphics` for rendering and `debug` for the rest. A channel that the
//! process does not have falls back to `debug`, or to the first channel of a process without `debug`. Other crates
//! log only warnings and errors.
//!
//! Like in C#, files are buffered and a thread flushes them every 5 seconds; warnings and errors flush at once. A channel whose
//! file cannot be created tries `<file>.1` up to `<file>.127`.
//!
//! A rustora addition for processes that run for years: a log file that reaches `ROTATE_BYTES` becomes `<file>.old`,
//! replacing the one before, so a channel takes at most `2 * ROTATE_BYTES` = 20 MB. Of the `exception-*.log` crash
//! reports the newest `MAX_EXCEPTION_LOGS` stay.

use crate::hash::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

const FLUSH_INTERVAL: Duration = Duration::from_secs(5);
const CREATE_LOG_FILE_MAX_RETRY_COUNT: usize = 128;
const ROTATE_BYTES: u64 = 10 * 1024 * 1024;
const MAX_EXCEPTION_LOGS: usize = 20;
/// `ServerSettings.TimestampFormat`.
const TIMESTAMP_FORMAT: &str = "%Y-%m-%dT%H:%M:%S";

/// A channel and its file, the arguments of `Log.AddChannel`.
#[derive(Clone, Copy, Debug)]
pub struct ChannelSpec {
    pub name: &'static str,
    pub file: &'static str,
    pub timestamped: bool,
    /// Created when logging starts, like the channels of `Game.Initialize`. Other channels are created on their
    /// first line, like `lua` and `install` in C#.
    pub eager: bool,
    /// Appends to an existing file instead of replacing it.
    pub append: bool,
}

impl ChannelSpec {
    pub const fn new(name: &'static str, file: &'static str, timestamped: bool, eager: bool) -> ChannelSpec {
        ChannelSpec {
            name,
            file,
            timestamped,
            eager,
            append: false,
        }
    }

    pub const fn appending(self) -> ChannelSpec {
        ChannelSpec { append: true, ..self }
    }
}

/// The channels of the game client, from `Game.Initialize`, `ScriptContext`, `SyncReport`, the installation logic
/// and `TraitDictionary`. The single player server runs inside the client and writes `server.log`; a multiplayer
/// server that the client starts runs as its own process and appends to the same file.
pub const CLIENT_CHANNELS: &[ChannelSpec] = &[
    ChannelSpec::new("server", "server.log", true, true),
    ChannelSpec::new("perf", "perf.log", false, true),
    ChannelSpec::new("debug", "debug.log", false, true),
    ChannelSpec::new("sound", "sound.log", false, true),
    ChannelSpec::new("graphics", "graphics.log", false, true),
    ChannelSpec::new("geoip", "geoip.log", false, true),
    ChannelSpec::new("nat", "nat.log", false, true),
    ChannelSpec::new("client", "client.log", true, true),
    ChannelSpec::new("lua", "lua.log", false, false),
    ChannelSpec::new("install", "install.log", false, false),
    ChannelSpec::new("sync", "syncreport.log", false, false),
    ChannelSpec::new("traitreport", "traitreport.log", false, false),
    ChannelSpec::new("assetbrowser", "assetbrowser.log", false, false),
    MALFORMED_PACKETS,
];

/// Packets that do not follow the protocol, with a hex dump, and signs of an attack like several clients that drop at
/// once or a flood of connections. A rustora addition. It appends, so a restart keeps the lines of earlier runs.
const MALFORMED_PACKETS: ChannelSpec = ChannelSpec::new("malformed_packets", "malformed_packets.log", true, false).appending();

/// The channels of an independent server, from `OpenRA.Server.Program` (`dedicated-*.log` in C#). Unlike in C#, they
/// append, so that a restart, for example by systemd, keeps the lines of the runs before.
pub const INDEPENDENT_SERVER_CHANNELS: &[ChannelSpec] = &[
    ChannelSpec::new("debug", "independent-debug.log", true, true).appending(),
    ChannelSpec::new("perf", "independent-perf.log", true, true).appending(),
    ChannelSpec::new("server", "independent-server.log", true, true).appending(),
    ChannelSpec::new("nat", "independent-nat.log", true, true).appending(),
    ChannelSpec::new("geoip", "independent-geoip.log", true, true).appending(),
    MALFORMED_PACKETS,
];

/// The channels of a dependent server, the multiplayer server process that a client started for its game. In C# it
/// runs inside the client and writes the `server.log` of the client for the whole session, so each server appends
/// to it. Its other lines go there too.
pub const DEPENDENT_SERVER_CHANNELS: &[ChannelSpec] =
    &[ChannelSpec::new("server", "server.log", true, true).appending(), MALFORMED_PACKETS];

struct Channel {
    writer: Option<BufWriter<File>>,
    timestamped: bool,
    path: Option<PathBuf>,
    /// The size of the file, counted from its size at opening.
    size: u64,
}

impl Channel {
    fn open(dir: &Path, spec: &ChannelSpec) -> Channel {
        let opened = create_log_file(dir, spec.file, spec.append);
        let size = opened.as_ref().and_then(|(f, _)| f.metadata().ok()).map_or(0, |m| m.len());
        let (writer, path) = opened.map_or((None, None), |(f, p)| (Some(BufWriter::new(f)), Some(p)));
        Channel {
            writer,
            timestamped: spec.timestamped,
            path,
            size,
        }
    }

    /// Writes `line`, first moving a full file to `<file>.old`.
    fn write_line(&mut self, line: &str) {
        if self.size + line.len() as u64 > ROTATE_BYTES {
            self.rotate();
        }
        if let Some(w) = &mut self.writer
            && w.write_all(line.as_bytes()).is_ok()
        {
            self.size += line.len() as u64;
        }
    }

    fn rotate(&mut self) {
        let Some(path) = self.path.clone() else {
            return;
        };
        if let Some(mut w) = self.writer.take() {
            let _ = w.flush();
        }
        let mut old = path.clone().into_os_string();
        old.push(".old");
        let _ = std::fs::rename(&path, &old);
        self.writer = OpenOptions::new().append(true).create(true).open(&path).ok().map(BufWriter::new);
        self.size = 0;
    }
}

struct State {
    open: HashMap<&'static str, Channel>,
}

struct ChannelLogger {
    dir: PathBuf,
    specs: &'static [ChannelSpec],
    state: Mutex<State>,
}

static LOGGER: OnceLock<ChannelLogger> = OnceLock::new();

/// `Log.AddChannel` for `specs` with the files in `<support_dir>/rustora/Logs`, installed as the `log` logger. Only the
/// first call of a process has an effect.
pub fn init(support_dir: &Path, specs: &'static [ChannelSpec]) {
    assert!(!specs.is_empty(), "a process logs to at least one channel");
    let logger = LOGGER.get_or_init(|| ChannelLogger {
        dir: crate::own_files::logs_dir(support_dir),
        specs,
        state: Mutex::new(State { open: HashMap::default() }),
    });
    if log::set_logger(logger).is_err() {
        return;
    }
    log::set_max_level(log::LevelFilter::Debug);
    let mut state = logger.lock();
    for spec in specs.iter().filter(|s| s.eager) {
        logger.channel(&mut state, spec.name);
    }
    drop(state);
    std::thread::Builder::new()
        .name("rustora log flush".to_string())
        .spawn(|| {
            loop {
                std::thread::sleep(FLUSH_INTERVAL);
                log::logger().flush();
            }
        })
        .ok();
}

static PANIC_CONTEXT: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Sets the lines about the running game that a panic report starts with, like the mod version and the map.
pub fn set_panic_context(lines: Vec<String>) {
    *PANIC_CONTEXT.lock().unwrap_or_else(|e| e.into_inner()) = lines;
}

/// Writes the report of a panic to `exception-<time>.log`, like `ExceptionHandler.HandleFatalError`, and then runs
/// the previous panic hook.
pub fn install_panic_hook(support_dir: &Path) {
    let dir = crate::own_files::logs_dir(support_dir);
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::logger().flush();
        let now = chrono::Utc::now();
        let mut lines = vec![format!("rustora engine version {}", env!("CARGO_PKG_VERSION"))];
        lines.extend(PANIC_CONTEXT.lock().unwrap_or_else(|e| e.into_inner()).iter().cloned());
        lines.push(format!("Date: {}", now.format("%Y-%m-%d %H:%M:%SZ")));
        lines.push(format!("Operating System: {} ({})", std::env::consts::OS, std::env::consts::ARCH));
        lines.push(format!("Exception: {info}"));
        lines.push(std::backtrace::Backtrace::force_capture().to_string());
        let name = format!("exception-{}.log", now.format("%Y-%m-%dT%H%M%SZ"));
        remove_old_exception_logs(&dir);
        if let Some((mut file, _)) = create_log_file(&dir, &name, false) {
            let _ = file.write_all((lines.join("\n") + "\n").as_bytes());
        }
        previous(info);
    }));
}

/// Deletes the oldest crash reports so that a new one makes `MAX_EXCEPTION_LOGS`. The names sort by time.
fn remove_old_exception_logs(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut reports: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("exception-")))
        .collect();
    reports.sort();
    let excess = (reports.len() + 1).saturating_sub(MAX_EXCEPTION_LOGS);
    for path in reports.iter().take(excess) {
        let _ = std::fs::remove_file(path);
    }
}

/// Opens the log file in append mode, so lines that another process appends to the same file stay intact. Without
/// `append` the file is emptied first, by an open of its own: on Windows, a file opened for appending cannot be
/// truncated.
fn create_log_file(dir: &Path, base: &str, append: bool) -> Option<(File, PathBuf)> {
    std::fs::create_dir_all(dir).ok()?;
    (0..CREATE_LOG_FILE_MAX_RETRY_COUNT).find_map(|i| {
        let name = if i > 0 { format!("{base}.{i}") } else { base.to_string() };
        let path = dir.join(name);
        if !append {
            OpenOptions::new().write(true).create(true).truncate(true).open(&path).ok()?;
        }
        let file = OpenOptions::new().append(true).create(true).open(&path).ok()?;
        Some((file, path))
    })
}

/// The channel of a record target that names no channel.
fn crate_channel(target: &str) -> &'static str {
    let krate = target.split("::").next().unwrap_or(target);
    match krate {
        "rustora_server" => "server",
        "rustora_audio" => "sound",
        "rustora_render" | "wgpu" | "wgpu_core" | "wgpu_hal" | "naga" => "graphics",
        "rustora_launch" if target.contains("net_client") => "client",
        _ => "debug",
    }
}

impl ChannelLogger {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn spec(&self, name: &str) -> Option<&'static ChannelSpec> {
        self.specs.iter().find(|s| s.name == name)
    }

    /// The channel `name`, opened on first use, or the channel that stands in for it.
    fn channel<'a>(&self, state: &'a mut State, name: &str) -> &'a mut Channel {
        let spec = self.spec(name).or_else(|| self.spec("debug")).unwrap_or(&self.specs[0]);
        state.open.entry(spec.name).or_insert_with(|| Channel::open(&self.dir, spec))
    }

    fn flush_all(state: &mut State) {
        for channel in state.open.values_mut() {
            if let Some(w) = &mut channel.writer {
                let _ = w.flush();
            }
        }
    }
}

impl log::Log for ChannelLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        let target = metadata.target();
        target.starts_with("rustora") || self.spec(target).is_some() || metadata.level() <= log::Level::Warn
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let target = record.target();
        let name = if self.spec(target).is_some() {
            target
        } else {
            crate_channel(target)
        };
        let mut state = self.lock();
        let channel = self.channel(&mut state, name);
        let line = if channel.timestamped {
            format!("[{}] {}\n", chrono::Local::now().format(TIMESTAMP_FORMAT), record.args())
        } else {
            format!("{}\n", record.args())
        };
        channel.write_line(&line);
        if record.level() <= log::Level::Warn {
            ChannelLogger::flush_all(&mut state);
        }
    }

    fn flush(&self) {
        ChannelLogger::flush_all(&mut self.lock());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rustora-log-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_full_log_moves_to_old() {
        let dir = temp_dir("rotate");
        let spec = ChannelSpec::new("x", "x.log", false, true).appending();
        let mut channel = Channel::open(&dir, &spec);
        let line = "a".repeat(1024 * 1024) + "\n";
        for _ in 0..25 {
            channel.write_line(&line);
        }
        channel.writer.as_mut().unwrap().flush().unwrap();
        let size = |name: &str| std::fs::metadata(dir.join(name)).map_or(0, |m| m.len());
        assert!(size("x.log") <= ROTATE_BYTES && size("x.log.old") <= ROTATE_BYTES);
        assert!(size("x.log") + size("x.log.old") >= 15 * 1024 * 1024);
        let reopened = Channel::open(&dir, &spec);
        assert_eq!(reopened.size, size("x.log"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_the_newest_exception_logs_stay() {
        let dir = temp_dir("exceptions");
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..25 {
            std::fs::write(dir.join(format!("exception-2026-01-01T0000{i:02}Z.log")), "x").unwrap();
        }
        std::fs::write(dir.join("debug.log"), "x").unwrap();
        remove_old_exception_logs(&dir);
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left.len(), MAX_EXCEPTION_LOGS);
        assert_eq!(left[0], "debug.log");
        assert_eq!(left[1], "exception-2026-01-01T000006Z.log");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
