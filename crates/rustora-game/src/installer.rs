// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `ModContent.cs`, `InstallerUtils.cs`, the source resolvers and the source actions. The code here has no
//! user interface and runs on worker threads, so it reports status as `StatusMessage` values that the user interface
//! thread turns into text. `ExtractBlast` and `ExtractMscab` are not ported, because only other mods use them.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use rustora_core::miniyaml::{MiniYaml, MiniYamlNode};
use rustora_fs::mix::MixFile;
use rustora_fs::{Package, Platform};
use sha1::{Digest, Sha1};

use crate::GameError;
use crate::cab::InstallShieldCab;
use crate::manifest::split_list;
use crate::moddata::ModData;

/// `InstallFromSourceLogic.ShowPercentageThreshold`.
const SHOW_PERCENTAGE_THRESHOLD: u64 = 26_214_400;

/// `Log.Write("install", ...)`: appends a line to `install.log`, whose absence the installation does not depend on.
pub fn write_install_log(platform: &Platform, line: &str) {
    let dir = platform.support_dir.join("Logs");
    let result = fs::create_dir_all(&dir).and_then(|()| {
        let mut file = File::options().create(true).append(true).open(dir.join("install.log"))?;
        writeln!(file, "{line}")
    });
    if let Err(e) = result {
        log::warn!("cannot write install.log: {e}");
    }
}

/// An argument of a `StatusMessage`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StatusArg {
    Text(String),
    Number(i64),
}

/// A Fluent message key with its arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusMessage {
    pub key: &'static str,
    pub args: Vec<(&'static str, StatusArg)>,
}

impl StatusMessage {
    pub fn new(key: &'static str) -> StatusMessage {
        StatusMessage { key, args: Vec::new() }
    }

    pub fn text(mut self, name: &'static str, value: impl Into<String>) -> StatusMessage {
        self.args.push((name, StatusArg::Text(value.into())));
        self
    }

    pub fn number(mut self, name: &'static str, value: i64) -> StatusMessage {
        self.args.push((name, StatusArg::Number(value)));
        self
    }
}

/// `ISourceResolver.GetAvailability`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    Unavailable,
    GameSource,
    DigitalInstall,
}

/// `ModContent.ModPackage`.
#[derive(Clone, Debug, Default)]
pub struct ModPackage {
    pub title: String,
    pub identifier: String,
    pub test_files: Vec<String>,
    pub sources: Vec<String>,
    pub required: bool,
    pub download: Option<String>,
}

impl ModPackage {
    pub fn is_installed(&self, platform: &Platform) -> bool {
        self.test_files.iter().all(|f| Path::new(&platform.resolve_path(f)).is_file())
    }
}

/// `ModContent.ModSource`.
#[derive(Clone, Debug, Default)]
pub struct ModSource {
    pub key: String,
    pub title: String,
    pub source_type: String,
    pub app_id: Option<String>,
    pub registry_prefixes: Vec<String>,
    pub registry_key: Option<String>,
    pub registry_value: Option<String>,
    pub id_files: Option<Vec<MiniYamlNode>>,
    pub install: Vec<MiniYamlNode>,
}

/// `ModContent.ModDownload`.
#[derive(Clone, Debug, Default)]
pub struct ModDownload {
    pub title: String,
    pub url: Option<String>,
    pub mirror_list: Option<String>,
    pub sha1: Option<String>,
    pub extract: Vec<(String, String)>,
}

/// `ModContent` with its source and download files loaded, in file order.
#[derive(Clone, Debug, Default)]
pub struct ModContent {
    pub target_mod: String,
    pub quick_download: Option<String>,
    pub packages: Vec<ModPackage>,
    pub downloads: Vec<(String, ModDownload)>,
    pub sources: Vec<ModSource>,
}

fn node<'a>(yaml: &'a MiniYaml, key: &str) -> Option<&'a MiniYamlNode> {
    yaml.nodes.iter().find(|n| n.key == key)
}

fn value(yaml: &MiniYaml, key: &str) -> Option<String> {
    node(yaml, key).and_then(|n| n.value.value.clone())
}

fn list(yaml: &MiniYaml, key: &str) -> Vec<String> {
    split_list(node(yaml, key).and_then(|n| n.value.value.as_deref()))
}

fn keys(yaml: &MiniYaml, key: &str) -> Vec<String> {
    node(yaml, key).map_or_else(Vec::new, |n| n.value.nodes.iter().map(|c| c.key.clone()).collect())
}

fn parse_source(key: &str, yaml: &MiniYaml) -> ModSource {
    let source_type = node(yaml, "Type");
    let prefixes = list(yaml, "RegistryPrefixes");
    ModSource {
        key: key.to_string(),
        title: yaml.value.clone().unwrap_or_default(),
        source_type: source_type.and_then(|t| t.value.value.clone()).unwrap_or_default(),
        app_id: source_type.and_then(|t| value(&t.value, "AppId")),
        registry_prefixes: if prefixes.is_empty() { vec![String::new()] } else { prefixes },
        registry_key: value(yaml, "RegistryKey"),
        registry_value: value(yaml, "RegistryValue"),
        id_files: node(yaml, "IDFiles").map(|n| n.value.nodes.clone()),
        install: node(yaml, "Install").map_or_else(Vec::new, |n| n.value.nodes.clone()),
    }
}

fn parse_download(yaml: &MiniYaml) -> ModDownload {
    ModDownload {
        title: yaml.value.clone().unwrap_or_default(),
        url: value(yaml, "URL"),
        mirror_list: value(yaml, "MirrorList"),
        sha1: value(yaml, "SHA1"),
        extract: node(yaml, "Extract").map_or_else(Vec::new, |n| {
            n.value
                .nodes
                .iter()
                .map(|e| (e.key.clone(), e.value.value.clone().unwrap_or_default()))
                .collect()
        }),
    }
}

impl ModContent {
    /// Reads the `ModContent` node of the manifest of the content installer mod.
    pub fn load(mod_data: &ModData) -> Result<ModContent, GameError> {
        let manifest = mod_data
            .manifest
            .package
            .get("mod.yaml")
            .ok_or_else(|| GameError::Data("mod.yaml is missing".to_string()))?;
        let nodes = MiniYaml::from_bytes(&manifest, "mod.yaml", true)?;
        let content = &nodes
            .iter()
            .find(|n| n.key == "ModContent")
            .ok_or_else(|| GameError::Data("the mod has no ModContent node".to_string()))?
            .value;
        let packages = node(content, "Packages").map_or_else(Vec::new, |n| {
            n.value
                .nodes
                .iter()
                .map(|p| ModPackage {
                    title: value(&p.value, "Title").unwrap_or_default(),
                    identifier: value(&p.value, "Identifier").unwrap_or_default(),
                    test_files: list(&p.value, "TestFiles"),
                    sources: list(&p.value, "Sources"),
                    required: value(&p.value, "Required").is_some_and(|r| r.eq_ignore_ascii_case("true")),
                    download: value(&p.value, "Download"),
                })
                .collect()
        });
        let downloads = ModData::load_yaml(&mod_data.file_system, &keys(content, "Downloads"), None)?;
        let sources = ModData::load_yaml(&mod_data.file_system, &keys(content, "Sources"), None)?;
        Ok(ModContent {
            target_mod: value(content, "Mod").ok_or_else(|| GameError::Data("ModContent has no Mod".to_string()))?,
            quick_download: value(content, "QuickDownload"),
            packages,
            downloads: downloads.iter().map(|d| (d.key.clone(), parse_download(&d.value))).collect(),
            sources: sources.iter().map(|s| parse_source(&s.key, &s.value)).collect(),
        })
    }

    pub fn download(&self, key: &str) -> Option<&ModDownload> {
        self.downloads.iter().find(|(k, _)| k == key).map(|(_, d)| d)
    }

    pub fn source(&self, key: &str) -> Option<&ModSource> {
        self.sources.iter().find(|s| s.key == key)
    }

    /// `ModContentLogic`: whether every required package is installed.
    pub fn required_content_installed(&self, platform: &Platform) -> bool {
        self.packages.iter().filter(|p| p.required).all(|p| p.is_installed(platform))
    }
}

/// Port of `FS.ResolveCaseInsensitivePath`: replaces each component of `path` by the directory entry whose name
/// matches it ignoring case, so the result has the case on disk also on case-insensitive file systems. Returns `path`
/// unchanged from the first component that does not exist. A component that exists without a matching entry, such as
/// a Windows short name like `RUNNER~1`, is kept as given.
pub fn resolve_case_insensitive(path: &Path) -> PathBuf {
    let mut resolved = PathBuf::new();
    let components: Vec<_> = path.components().collect();
    for (i, component) in components.iter().enumerate() {
        let std::path::Component::Normal(name) = component else {
            resolved.push(component);
            continue;
        };
        let wanted = name.to_string_lossy().to_lowercase();
        let found = fs::read_dir(if resolved.as_os_str().is_empty() { Path::new(".") } else { &resolved })
            .ok()
            .and_then(|entries| {
                entries
                    .flatten()
                    .map(|e| e.file_name())
                    .find(|n| n.to_string_lossy().to_lowercase() == wanted)
            });
        match found {
            Some(n) => resolved.push(n),
            None if resolved.join(name).exists() => resolved.push(name),
            None => {
                resolved.extend(components[i..].iter());
                return resolved;
            }
        }
    }
    resolved
}

fn hash_hex(hasher: Sha1) -> String {
    hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// `CryptoUtil.SHA1Hash` of a file.
pub fn sha1_file(path: &Path) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha1::new();
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            return Ok(hash_hex(hasher));
        }
        hasher.update(&buffer[..read]);
    }
}

fn id_file_matches(path: &Path, id_file: &MiniYamlNode) -> std::io::Result<bool> {
    let file_path = resolve_case_insensitive(&path.join(&id_file.key));
    if !file_path.is_file() {
        return Ok(false);
    }
    let expected = id_file.value.value.as_deref().unwrap_or_default();
    let offset = value(&id_file.value, "Offset").and_then(|v| v.parse::<u64>().ok());
    let length = value(&id_file.value, "Length").and_then(|v| v.parse::<u64>().ok());
    if offset.is_none() && length.is_none() {
        return Ok(sha1_file(&file_path)? == expected);
    }
    let mut file = File::open(&file_path)?;
    let offset = offset.unwrap_or(0);
    let length = length.unwrap_or_else(|| file.metadata().map_or(0, |m| m.len()).saturating_sub(offset));
    file.seek(SeekFrom::Start(offset))?;
    let mut data = Vec::new();
    file.take(length).read_to_end(&mut data)?;
    let mut hasher = Sha1::new();
    hasher.update(&data);
    Ok(hash_hex(hasher) == expected)
}

/// `InstallerUtils.IsValidSourcePath`.
pub fn is_valid_source_path(path: &Path, source: &ModSource) -> bool {
    let Some(id_files) = &source.id_files else {
        return true;
    };
    id_files.iter().all(|f| id_file_matches(path, f).unwrap_or(false))
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// The value of a registry entry, read with the `reg` tool of Windows.
#[cfg(windows)]
fn registry_value(key: &str, value: &str) -> Option<String> {
    let output = std::process::Command::new("reg").args(["query", key, "/v", value]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    parse_reg_query(&String::from_utf8_lossy(&output.stdout), value)
}

#[cfg(not(windows))]
fn registry_value(_key: &str, _value: &str) -> Option<String> {
    None
}

/// Finds the data of `value` in the output of `reg query`, whose lines read `<value>    REG_SZ    <data>`.
#[cfg_attr(not(windows), allow(dead_code))]
fn parse_reg_query(output: &str, value: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let rest = line.trim_start().strip_prefix(value)?;
        let rest = rest.trim_start().strip_prefix("REG_")?;
        let (_, data) = rest.split_once(char::is_whitespace)?;
        Some(data.trim().to_string())
    })
}

/// A line `"key"  "value"` of the Valve key value format.
fn parse_key_value(line: &str) -> Option<(&str, &str)> {
    let parts: Vec<&str> = line.trim().split('"').collect();
    match parts.as_slice() {
        ["", key, separator, value, ""] if separator.trim().is_empty() && !separator.is_empty() => Some((key, value)),
        _ => None,
    }
}

fn steam_directories() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if cfg!(windows) {
        for key in [
            "HKEY_LOCAL_MACHINE\\Software\\Valve\\Steam",
            "HKEY_LOCAL_MACHINE\\SOFTWARE\\Wow6432Node\\Valve\\Steam",
        ] {
            candidates.extend(registry_value(key, "InstallPath").map(PathBuf::from));
        }
    } else if cfg!(target_os = "macos") {
        candidates.push(home_dir().join("Library/Application Support/Steam"));
    } else {
        candidates.push(home_dir().join(".steam/root"));
        candidates.push(home_dir().join(".var/app/com.valvesoftware.Steam/.steam/root"));
    }
    let mut seen = HashSet::new();
    let mut directories = Vec::new();
    for steam in candidates {
        if !steam.is_dir() {
            continue;
        }
        let libraries = fs::read_to_string(steam.join("steamapps/libraryfolders.vdf")).unwrap_or_default();
        let extra: Vec<PathBuf> = libraries
            .lines()
            .filter_map(parse_key_value)
            .filter(|(k, _)| *k == "path")
            .map(|(_, v)| PathBuf::from(v.replace("\\\\", "\\")))
            .collect();
        for directory in std::iter::once(steam).chain(extra) {
            if seen.insert(directory.clone()) {
                directories.push(directory);
            }
        }
    }
    directories
}

fn find_steam(source: &ModSource) -> Option<String> {
    let app_id = source.app_id.as_deref()?;
    for steam in steam_directories() {
        let Ok(manifest) = fs::read_to_string(steam.join(format!("steamapps/appmanifest_{app_id}.acf"))) else {
            continue;
        };
        let pairs: Vec<(&str, &str)> = manifest.lines().filter_map(parse_key_value).collect();
        let lookup = |key: &str| pairs.iter().find(|(k, _)| *k == key).map(|(_, v)| *v);
        if lookup("StateFlags") != Some("4") {
            continue;
        }
        let Some(install_dir) = lookup("installdir") else { continue };
        let path = steam.join("steamapps/common").join(install_dir);
        if is_valid_source_path(&path, source) {
            return Some(path.to_string_lossy().into_owned());
        }
    }
    None
}

/// Mounted optical discs on Linux, the `/Volumes` entries on macOS and the drive letters on Windows. Windows and macOS
/// list more than the optical drives of the C# code, which is harmless, because each path must pass the file hashes.
fn disc_volumes() -> Vec<PathBuf> {
    let mut volumes = Vec::new();
    if cfg!(windows) {
        volumes.extend(('A'..='Z').map(|c| PathBuf::from(format!("{c}:\\"))));
    } else if cfg!(target_os = "macos") {
        if let Ok(entries) = fs::read_dir("/Volumes") {
            volumes.extend(entries.flatten().map(|e| e.path()));
        }
    } else {
        let mounts = fs::read_to_string("/proc/mounts").unwrap_or_default();
        for line in mounts.lines() {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if let [_, mount, kind, ..] = fields.as_slice()
                && matches!(*kind, "iso9660" | "udf")
            {
                volumes.push(PathBuf::from(mount.replace("\\040", " ")));
            }
        }
        volumes.push(PathBuf::from("/media/openra"));
        if let Some(user) = std::env::var_os("USER") {
            volumes.push(Path::new("/media").join(user).join("openra"));
        }
        volumes.push(PathBuf::from("/mnt/openra"));
    }
    volumes
}

fn find_disc(source: &ModSource) -> Option<String> {
    disc_volumes()
        .into_iter()
        .find(|v| v.is_dir() && is_valid_source_path(v, source))
        .map(|v| v.to_string_lossy().into_owned())
}

fn find_gog(source: &ModSource) -> Option<String> {
    let app_id = source.app_id.as_deref()?;
    ["Software\\", "Software\\Wow6432Node\\"].iter().find_map(|prefix| {
        let path = registry_value(&format!("HKEY_LOCAL_MACHINE\\{prefix}GOG.com\\Games\\{app_id}"), "path")?;
        is_valid_source_path(Path::new(&path), source).then_some(path)
    })
}

fn find_registry(source: &ModSource, parent_of_value: bool) -> Option<String> {
    let key = source.registry_key.as_deref()?;
    let name = source.registry_value.as_deref()?;
    for prefix in &source.registry_prefixes {
        let Some(path) = registry_value(&format!("{prefix}{key}"), name) else {
            continue;
        };
        let path = if parent_of_value {
            Path::new(&path).parent()?.to_path_buf()
        } else {
            PathBuf::from(path)
        };
        return is_valid_source_path(&path, source).then(|| path.to_string_lossy().into_owned());
    }
    None
}

/// `ISourceResolver.FindSourcePath` of the resolver that the source names.
pub fn find_source_path(source: &ModSource) -> Option<String> {
    match source.source_type.as_str() {
        "Disc" => find_disc(source),
        "Steam" => find_steam(source),
        "Gog" => find_gog(source),
        "RegistryDirectory" => find_registry(source, false),
        "RegistryDirectoryFromFile" => find_registry(source, true),
        other => {
            log::warn!("unknown content source type {other}");
            None
        }
    }
}

/// `ISourceResolver.GetAvailability` of the resolver that the source names.
pub fn source_availability(source: &ModSource) -> Availability {
    match source.source_type.as_str() {
        "Disc" => Availability::GameSource,
        "Steam" => Availability::DigitalInstall,
        "Gog" | "RegistryDirectory" | "RegistryDirectoryFromFile" if !cfg!(windows) => Availability::DigitalInstall,
        _ => Availability::Unavailable,
    }
}

/// The part of an installation that later steps need to undo it.
pub struct InstallContext<'a> {
    pub platform: &'a Platform,
    pub path: &'a Path,
    pub extracted: &'a mut Vec<String>,
    pub on_message: &'a dyn Fn(StatusMessage),
}

impl InstallContext<'_> {
    fn source_path(&self, yaml: &MiniYaml) -> PathBuf {
        let source = yaml.value.as_deref().unwrap_or_default();
        if source.starts_with('^') {
            PathBuf::from(self.platform.resolve_path(source))
        } else {
            resolve_case_insensitive(&self.path.join(source))
        }
    }

    /// The target path of a yaml entry, or None when the file exists already.
    fn prepare_target(&mut self, key: &str, register: bool) -> Result<Option<PathBuf>, String> {
        let target = PathBuf::from(self.platform.resolve_path(key));
        if target.is_file() {
            write_install_log(self.platform, &format!("Skipping installed file {}", target.display()));
            return Ok(None);
        }
        if register {
            self.extracted.push(target.to_string_lossy().into_owned());
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        Ok(Some(target))
    }
}

fn display_name(target: &Path) -> String {
    target.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

/// `InstallerUtils.CopyStream` with the progress messages of the source actions.
fn copy_with_progress(
    input: &mut dyn Read,
    target: &Path,
    length: u64,
    plain_key: &'static str,
    progress_key: &'static str,
    on_message: &dyn Fn(StatusMessage),
) -> Result<(), String> {
    let name = display_name(target);
    if length < SHOW_PERCENTAGE_THRESHOLD {
        on_message(StatusMessage::new(plain_key).text("filename", name.clone()));
    }
    let mut output = File::create(target).map_err(|e| format!("{}: {e}", target.display()))?;
    let mut buffer = vec![0u8; 1 << 16];
    let mut copied = 0u64;
    while copied < length {
        let wanted = (length - copied).min(buffer.len() as u64) as usize;
        let read = input.read(&mut buffer[..wanted]).map_err(|e| e.to_string())?;
        if read == 0 {
            return Err(format!("{}: the source ended early", target.display()));
        }
        output.write_all(&buffer[..read]).map_err(|e| e.to_string())?;
        copied += read as u64;
        if length >= SHOW_PERCENTAGE_THRESHOLD {
            on_message(
                StatusMessage::new(progress_key)
                    .text("filename", name.clone())
                    .number("progress", (100 * copied / length) as i64),
            );
        }
    }
    Ok(())
}

const EXTRACTING: &str = "label-extracting-filename";
const EXTRACTING_PROGRESS: &str = "label-extracting-filename-progress";

fn action_copy(yaml: &MiniYaml, ctx: &mut InstallContext) -> Result<(), String> {
    let source_dir = ctx.path.join(yaml.value.as_deref().unwrap_or_default());
    for entry in &yaml.nodes {
        let source = resolve_case_insensitive(&source_dir.join(entry.value.value.as_deref().unwrap_or_default()));
        let Some(target) = ctx.prepare_target(&entry.key, false)? else {
            continue;
        };
        let mut input = File::open(&source).map_err(|e| format!("{}: {e}", source.display()))?;
        let length = input.metadata().map_err(|e| e.to_string())?.len();
        write_install_log(ctx.platform, &format!("Copying {} -> {}", source.display(), target.display()));
        ctx.extracted.push(target.to_string_lossy().into_owned());
        copy_with_progress(
            &mut input,
            &target,
            length,
            "label-copying-filename",
            "label-copying-filename-progress",
            ctx.on_message,
        )?;
    }
    Ok(())
}

fn action_delete(yaml: &MiniYaml, ctx: &mut InstallContext) -> Result<(), String> {
    let source = yaml.value.as_deref().unwrap_or_default();
    if source.starts_with('^') {
        let path = ctx.platform.resolve_path(source);
        write_install_log(ctx.platform, &format!("Deleting {path}"));
        fs::remove_file(&path).map_err(|e| format!("{path}: {e}"))?;
    } else {
        write_install_log(
            ctx.platform,
            &format!("Ignoring delete of {source}, which is outside the platform directories"),
        );
    }
    Ok(())
}

fn action_extract_raw(yaml: &MiniYaml, ctx: &mut InstallContext) -> Result<(), String> {
    let source_path = ctx.source_path(yaml);
    let mut source = File::open(&source_path).map_err(|e| format!("{}: {e}", source_path.display()))?;
    for entry in &yaml.nodes {
        let offset = value(&entry.value, "Offset").and_then(|v| v.parse::<u64>().ok());
        let length = value(&entry.value, "Length").and_then(|v| v.parse::<u64>().ok());
        let (Some(offset), Some(length)) = (offset, length) else {
            write_install_log(ctx.platform, &format!("Skipping entry without Offset or Length: {}", entry.key));
            continue;
        };
        let Some(target) = ctx.prepare_target(&entry.key, true)? else {
            continue;
        };
        source.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
        write_install_log(
            ctx.platform,
            &format!("Extracting {} -> {}", source_path.display(), target.display()),
        );
        copy_with_progress(&mut source, &target, length, EXTRACTING, EXTRACTING_PROGRESS, ctx.on_message)?;
    }
    Ok(())
}

fn action_extract_mix(yaml: &MiniYaml, ctx: &mut InstallContext) -> Result<(), String> {
    let source_path = ctx.source_path(yaml);
    let data = fs::read(&source_path).map_err(|e| format!("{}: {e}", source_path.display()))?;
    let names: Vec<String> = yaml.nodes.iter().filter_map(|n| n.value.value.clone()).collect();
    let name = display_name(&source_path);
    let mix = MixFile::parse(data.into(), &name, &names).ok_or_else(|| format!("{}: not a mix file", source_path.display()))?;
    for entry in &yaml.nodes {
        let Some(target) = ctx.prepare_target(&entry.key, true)? else {
            continue;
        };
        let file = entry.value.value.as_deref().unwrap_or_default();
        let data = mix.get(file).ok_or_else(|| format!("{}: no entry {file}", source_path.display()))?;
        write_install_log(
            ctx.platform,
            &format!("Extracting {} -> {}", source_path.display(), target.display()),
        );
        copy_with_progress(
            &mut data.as_slice(),
            &target,
            data.len() as u64,
            EXTRACTING,
            EXTRACTING_PROGRESS,
            ctx.on_message,
        )?;
    }
    Ok(())
}

fn action_extract_zip(yaml: &MiniYaml, ctx: &mut InstallContext) -> Result<(), String> {
    let source_path = ctx.source_path(yaml);
    let file = File::open(&source_path).map_err(|e| format!("{}: {e}", source_path.display()))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("{}: {e}", source_path.display()))?;
    for entry in &yaml.nodes {
        let Some(target) = ctx.prepare_target(&entry.key, false)? else {
            continue;
        };
        let file = entry.value.value.as_deref().unwrap_or_default();
        let mut input = archive.by_name(file).map_err(|e| format!("{file}: {e}"))?;
        let mut output = File::create(&target).map_err(|e| format!("{}: {e}", target.display()))?;
        write_install_log(ctx.platform, &format!("Extracting {file} -> {}", target.display()));
        std::io::copy(&mut input, &mut output).map_err(|e| format!("{file}: {e}"))?;
        (ctx.on_message)(
            StatusMessage::new(EXTRACTING_PROGRESS)
                .text("filename", display_name(&target))
                .number("progress", 100),
        );
        ctx.extracted.push(target.to_string_lossy().into_owned());
    }
    Ok(())
}

fn action_extract_iscab(yaml: &MiniYaml, ctx: &mut InstallContext) -> Result<(), String> {
    let source_path = ctx.source_path(yaml);
    let volume_node = node(yaml, "Volumes").ok_or("extract-iscab entry doesn't define a Volumes node")?;
    let extract_node = node(yaml, "Extract").ok_or("extract-iscab entry doesn't define an Extract node")?;
    let mut volumes = Vec::new();
    for entry in &volume_node.value.nodes {
        let volume: i32 = entry.key.parse().map_err(|_| format!("bad volume {}", entry.key))?;
        let path = resolve_case_insensitive(&ctx.path.join(entry.value.value.as_deref().unwrap_or_default()));
        volumes.push((volume, File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?));
    }
    let header = File::open(&source_path).map_err(|e| format!("{}: {e}", source_path.display()))?;
    let mut cab = InstallShieldCab::new(header, volumes.into_iter().collect())?;
    for entry in &extract_node.value.nodes {
        let Some(target) = ctx.prepare_target(&entry.key, true)? else {
            continue;
        };
        let name = display_name(&target);
        let mut output = File::create(&target).map_err(|e| format!("{}: {e}", target.display()))?;
        write_install_log(
            ctx.platform,
            &format!("Extracting {} -> {}", source_path.display(), target.display()),
        );
        let on_message = ctx.on_message;
        cab.extract_file(entry.value.value.as_deref().unwrap_or_default(), &mut output, &|percent| {
            on_message(
                StatusMessage::new(EXTRACTING_PROGRESS)
                    .text("filename", name.clone())
                    .number("progress", percent),
            );
        })?;
    }
    Ok(())
}

fn run_source_actions(package: &MiniYaml, ctx: &mut InstallContext) -> Result<(), String> {
    let Some(actions) = node(package, "Actions") else {
        return Ok(());
    };
    for action in &actions.value.nodes {
        let name = action.key.split('@').next().unwrap_or_default();
        match name {
            "Copy" => action_copy(&action.value, ctx)?,
            "Delete" => action_delete(&action.value, ctx)?,
            "ExtractRaw" => action_extract_raw(&action.value, ctx)?,
            "ExtractMix" => action_extract_mix(&action.value, ctx)?,
            "ExtractZip" => action_extract_zip(&action.value, ctx)?,
            "ExtractIscab" => action_extract_iscab(&action.value, ctx)?,
            other => return Err(format!("the source action {other} is not supported")),
        }
    }
    Ok(())
}

/// `InstallFromSourceLogic.InstallFromSource`, without the status changes of the user interface. Removes the files
/// that it extracted when an action fails.
pub fn install_from_source(
    source: &ModSource,
    path: &Path,
    selected: &[String],
    platform: &Platform,
    on_message: &dyn Fn(StatusMessage),
) -> Result<(), String> {
    let mut extracted = Vec::new();
    let mut ctx = InstallContext {
        platform,
        path,
        extracted: &mut extracted,
        on_message,
    };
    let result: Result<(), String> = (|| {
        if let Some(before) = source.install.iter().find(|n| n.key == "BeforeInstall") {
            run_source_actions(&before.value, &mut ctx)?;
        }
        for package in source
            .install
            .iter()
            .filter(|n| n.key == "ContentPackage" || n.key.starts_with("ContentPackage@"))
        {
            if value(&package.value, "Name").is_some_and(|name| selected.contains(&name)) {
                run_source_actions(&package.value, &mut ctx)?;
            }
        }
        if let Some(after) = source.install.iter().find(|n| n.key == "AfterInstall") {
            run_source_actions(&after.value, &mut ctx)?;
        }
        Ok(())
    })();
    if let Err(e) = &result {
        write_install_log(platform, &format!("Installation failed: {e}"));
        delete_files(&extracted, platform);
    }
    result
}

/// The `TooltipText` of the install section of a package in a source.
pub fn package_tooltip(source: &ModSource, identifier: &str) -> Option<String> {
    source
        .install
        .iter()
        .find(|n| value(&n.value, "Name").as_deref() == Some(identifier))
        .and_then(|n| value(&n.value, "TooltipText"))
}

/// Removes the files of a failed installation.
pub fn delete_files(files: &[String], platform: &Platform) {
    for file in files {
        write_install_log(platform, &format!("Deleting {file}"));
        if let Err(e) = fs::remove_file(file) {
            write_install_log(platform, &format!("Cannot delete {file}: {e}"));
        }
    }
}

/// Extracts the entries of a downloaded zip file, as `DownloadPackageLogic` does. Removes the extracted files on failure.
pub fn extract_download(download: &ModDownload, archive: &Path, platform: &Platform, on_entry: &dyn Fn(&str)) -> Result<(), String> {
    let mut extracted = Vec::new();
    let result: Result<(), String> = (|| {
        let file = File::open(archive).map_err(|e| e.to_string())?;
        let mut zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
        for (target, entry_name) in &download.extract {
            let Ok(mut entry) = zip.by_name(entry_name) else {
                continue;
            };
            if !entry.is_file() {
                continue;
            }
            on_entry(entry_name);
            let target = platform.resolve_path(target);
            if let Some(parent) = Path::new(&target).parent() {
                fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
            }
            extracted.push(target.clone());
            let mut output = File::create(&target).map_err(|e| format!("{target}: {e}"))?;
            std::io::copy(&mut entry, &mut output).map_err(|e| format!("{target}: {e}"))?;
        }
        Ok(())
    })();
    if let Err(e) = &result {
        write_install_log(platform, &format!("Archive extraction failed: {e}"));
        delete_files(&extracted, platform);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rustora-installer-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn yaml(text: &str) -> Vec<MiniYamlNode> {
        MiniYaml::from_bytes(text.as_bytes(), "test.yaml", true).unwrap()
    }

    fn sha1_hex(data: &[u8]) -> String {
        let mut hasher = Sha1::new();
        hasher.update(data);
        hash_hex(hasher)
    }

    #[test]
    fn valve_key_values() {
        assert_eq!(parse_key_value("\t\"path\"\t\t\"/a b\""), Some(("path", "/a b")));
        assert_eq!(parse_key_value("\"StateFlags\" \"4\""), Some(("StateFlags", "4")));
        assert_eq!(parse_key_value("\"AppState\""), None);
        assert_eq!(parse_key_value("{"), None);
    }

    #[test]
    fn reg_query_output() {
        let text = "\r\nHKEY_LOCAL_MACHINE\\Software\\X\r\n    Install Dir    REG_SZ    C:\\Games\\Red Alert\r\n\r\n";
        assert_eq!(parse_reg_query(text, "Install Dir").as_deref(), Some("C:\\Games\\Red Alert"));
        assert_eq!(parse_reg_query(text, "Other"), None);
    }

    #[test]
    fn case_insensitive_paths() {
        let dir = temp_dir("case");
        fs::create_dir_all(dir.join("Install")).unwrap();
        fs::write(dir.join("Install/RedAlert.MIX"), b"x").unwrap();
        assert_eq!(
            resolve_case_insensitive(&dir.join("INSTALL/redalert.mix")),
            dir.join("Install/RedAlert.MIX")
        );
        assert_eq!(resolve_case_insensitive(&dir.join("INSTALL/none")), dir.join("Install/none"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn id_files_with_and_without_a_range() {
        let dir = temp_dir("id");
        fs::write(dir.join("MAIN.MIX"), b"0123456789").unwrap();
        let whole = sha1_hex(b"0123456789");
        let part = sha1_hex(b"2345");
        let source = |id: String| ModSource {
            id_files: Some(yaml(&id)),
            ..ModSource::default()
        };
        assert!(is_valid_source_path(&dir, &source(format!("main.mix: {whole}\n"))));
        assert!(is_valid_source_path(
            &dir,
            &source(format!("main.mix: {part}\n\tOffset: 2\n\tLength: 4\n"))
        ));
        assert!(!is_valid_source_path(&dir, &source(format!("main.mix: {part}\n"))));
        assert!(!is_valid_source_path(&dir, &source(format!("none.mix: {whole}\n"))));
        assert!(is_valid_source_path(&dir, &ModSource::default()));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn actions_run_for_selected_packages_and_failures_remove_files() {
        let dir = temp_dir("actions");
        let source_dir = dir.join("source");
        fs::create_dir_all(source_dir.join("Data")).unwrap();
        fs::write(source_dir.join("Data/ONE.BIN"), b"one").unwrap();
        fs::write(source_dir.join("raw.bin"), b"0123456789").unwrap();
        let platform = Platform::new(dir.join("engine"), dir.join("support"));
        let install = |text: &str, selected: &[&str]| {
            let source = ModSource {
                install: yaml(text),
                ..ModSource::default()
            };
            let selected: Vec<String> = selected.iter().map(|s| s.to_string()).collect();
            install_from_source(&source, &source_dir, &selected, &platform, &|_| {})
        };
        let text = "ContentPackage@a:\n\tName: a\n\tActions:\n\t\tCopy: data\n\t\t\t^SupportDir|out/one.bin: one.bin\n\
                    ContentPackage@b:\n\tName: b\n\tActions:\n\t\tExtractRaw: raw.bin\n\t\t\t^SupportDir|out/raw.bin:\n\t\t\t\tOffset: 2\n\t\t\t\tLength: 3\n";
        install(text, &["a"]).unwrap();
        assert_eq!(fs::read(dir.join("support/out/one.bin")).unwrap(), b"one");
        assert!(!dir.join("support/out/raw.bin").exists());
        install(text, &["a", "b"]).unwrap();
        assert_eq!(fs::read(dir.join("support/out/raw.bin")).unwrap(), b"234");

        let failing = "BeforeInstall:\n\tActions:\n\t\tExtractRaw: raw.bin\n\t\t\t^SupportDir|out/new.bin:\n\t\t\t\tOffset: 0\n\t\t\t\tLength: 1\n\
                       AfterInstall:\n\tActions:\n\t\tCopy: data\n\t\t\t^SupportDir|out/two.bin: missing.bin\n";
        assert!(install(failing, &[]).is_err());
        assert!(!dir.join("support/out/new.bin").exists());
        assert!(dir.join("support/out/one.bin").exists());
        let delete = "AfterInstall:\n\tActions:\n\t\tDelete: ^SupportDir|out/one.bin\n";
        install(delete, &[]).unwrap();
        assert!(!dir.join("support/out/one.bin").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn extract_mix_reads_named_entries() {
        let dir = temp_dir("mix");
        let name = "file.dat";
        let hash = rustora_fs::mix::hash_filename(name, rustora_fs::mix::HashType::Classic);
        let payload = b"payload";
        let mut mix = Vec::new();
        mix.extend_from_slice(&1u16.to_le_bytes());
        mix.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        mix.extend_from_slice(&hash.to_le_bytes());
        mix.extend_from_slice(&0u32.to_le_bytes());
        mix.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        mix.extend_from_slice(payload);
        fs::write(dir.join("TEST.MIX"), &mix).unwrap();
        let platform = Platform::new(dir.join("engine"), dir.join("support"));
        let source = ModSource {
            install: yaml("AfterInstall:\n\tActions:\n\t\tExtractMix: test.mix\n\t\t\t^SupportDir|file.dat: file.dat\n"),
            ..ModSource::default()
        };
        install_from_source(&source, &dir, &[], &platform, &|_| {}).unwrap();
        assert_eq!(fs::read(dir.join("support/file.dat")).unwrap(), payload);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ra_content_manifest_loads() {
        let engine = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../openra-ref");
        if !engine.join("mods/ra-content/mod.yaml").exists() {
            return;
        }
        let support = temp_dir("manifest");
        let mod_data = ModData::load(Platform::new(engine, &support), "ra-content").unwrap();
        let content = ModContent::load(&mod_data).unwrap();
        assert_eq!(content.target_mod, "ra");
        assert_eq!(content.quick_download.as_deref(), Some("quickinstall"));
        assert!(content.packages.iter().any(|p| p.identifier == "base" && p.required));
        assert!(content.download("quickinstall").is_some_and(|d| d.mirror_list.is_some()));
        assert_eq!(content.source("ra-steam").and_then(|s| s.app_id.as_deref()), Some("2229840"));
        assert!(!content.required_content_installed(&mod_data.platform));
        fs::remove_dir_all(&support).unwrap();
    }
}
