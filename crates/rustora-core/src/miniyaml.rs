// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! MiniYaml: the indentation based configuration format of OpenRA.
//!
//! This is a port of `OpenRA.Game/MiniYaml.cs`. Parsing, merging, inheritance resolution and
//! serialization follow the C# implementation so that rule trees come out identical.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

const SPACES_PER_LEVEL: usize = 4;

/// Source file name and 1-based line number of a node.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceLocation {
    pub name: Arc<str>,
    pub line: u32,
}

impl fmt::Display for SourceLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.name, self.line)
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct YamlError(pub String);

/// A key with a [`MiniYaml`] value. An empty key stands for the C# `null` key.
#[derive(Clone, Debug, PartialEq)]
pub struct MiniYamlNode {
    pub key: String,
    pub value: MiniYaml,
    pub comment: Option<String>,
    pub location: SourceLocation,
}

/// A value string plus child nodes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MiniYaml {
    pub value: Option<String>,
    pub nodes: Vec<MiniYamlNode>,
}

impl MiniYamlNode {
    pub fn new(key: impl Into<String>, value: MiniYaml) -> Self {
        MiniYamlNode {
            key: key.into(),
            value,
            comment: None,
            location: SourceLocation::default(),
        }
    }

    pub fn with_value_str(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self::new(key, MiniYaml::from_value(value))
    }

    fn with_value(&self, value: MiniYaml) -> Self {
        MiniYamlNode {
            key: self.key.clone(),
            value,
            comment: self.comment.clone(),
            location: self.location.clone(),
        }
    }
}

impl MiniYaml {
    pub fn new(value: Option<String>, nodes: Vec<MiniYamlNode>) -> Self {
        MiniYaml { value, nodes }
    }

    pub fn from_value(value: impl Into<String>) -> Self {
        MiniYaml {
            value: Some(value.into()),
            nodes: Vec::new(),
        }
    }

    /// The value string, with the C# null mapped to "".
    pub fn value_str(&self) -> &str {
        self.value.as_deref().unwrap_or("")
    }

    /// Returns the single node with `key`, or `None`. Errors on duplicate keys.
    pub fn node_with_key_or_default(&self, key: &str) -> Result<Option<&MiniYamlNode>, YamlError> {
        let mut result = None;
        for node in &self.nodes {
            if node.key != key {
                continue;
            }
            if result.is_some() {
                return Err(YamlError(format!("Duplicate key '{}' in {}", node.key, node.location)));
            }
            result = Some(node);
        }
        Ok(result)
    }

    pub fn node_with_key(&self, key: &str) -> Result<&MiniYamlNode, YamlError> {
        self.node_with_key_or_default(key)?
            .ok_or_else(|| YamlError(format!("No node with key '{key}'")))
    }

    /// Child nodes keyed by name, in insertion order. Errors on duplicate keys.
    pub fn to_map(&self) -> Result<indexmap::IndexMap<&str, &MiniYaml>, YamlError> {
        let mut ret = indexmap::IndexMap::with_capacity(self.nodes.len());
        for n in &self.nodes {
            if ret.insert(n.key.as_str(), &n.value).is_some() {
                return Err(YamlError(format!("Duplicate key '{}' in {}", n.key, n.location)));
            }
        }
        Ok(ret)
    }

    /// Parses text. Lines are split at "\n" with an optional preceding "\r".
    pub fn from_str(text: &str, name: &str, discard_comments_and_whitespace: bool) -> Result<Vec<MiniYamlNode>, YamlError> {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let lines = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l));
        from_lines(lines, name, discard_comments_and_whitespace)
    }

    /// Parses UTF-8 bytes, as read from a file.
    pub fn from_bytes(data: &[u8], name: &str, discard_comments_and_whitespace: bool) -> Result<Vec<MiniYamlNode>, YamlError> {
        let text = String::from_utf8_lossy(data);
        let text = text.strip_suffix('\n').unwrap_or(&text);
        Self::from_str(text, name, discard_comments_and_whitespace)
    }

    /// Serializes `key: value #comment` and the children, indented by tabs.
    pub fn to_lines(&self, key: &str, comment: Option<&str>, out: &mut Vec<String>) {
        let has_key = !key.is_empty();
        let value = self.value.as_deref().unwrap_or("");
        let has_value = !value.is_empty();
        let mut line = String::new();
        if has_key {
            line.push_str(key);
            line.push(':');
        }
        if has_value {
            line.push(' ');
            line.push_str(&value.replace('#', "\\#"));
        }
        if let Some(c) = comment {
            if has_key || has_value {
                line.push(' ');
            }
            line.push('#');
            line.push_str(c);
        }
        out.push(line);
        let mut child = Vec::new();
        nodes_to_lines(&self.nodes, &mut child);
        out.extend(child.into_iter().map(|l| format!("\t{l}")));
    }

    /// Merges several node lists (later ones override earlier ones) and resolves
    /// `Inherits`, `Inherits@*` and `-Removal` nodes.
    pub fn merge(sources: Vec<Vec<MiniYamlNode>>) -> Result<Vec<MiniYamlNode>, YamlError> {
        if sources.is_empty() {
            return Ok(Vec::new());
        }

        let mut iter = sources.into_iter();
        let mut acc = merge_self_partial(iter.next().unwrap())?;
        for s in iter {
            acc = merge_partial_nodes(acc, merge_self_partial(s)?)?;
        }

        let mut tree: indexmap::IndexMap<String, MiniYaml> = indexmap::IndexMap::new();
        for n in acc.into_iter().filter(|n| !n.key.is_empty()) {
            if tree.contains_key(&n.key) {
                return Err(YamlError(format!(
                    "An item with the same key has already been added. Key: {}",
                    n.key
                )));
            }
            tree.insert(n.key, n.value);
        }

        let mut resolved_nodes = Vec::with_capacity(tree.len());
        for (k, v) in &tree {
            let mut inherited = Inherited::default();
            inherited.add(k.clone(), SourceLocation::default());
            let children = resolve_inherits(v, &tree, &inherited)?;
            resolved_nodes.push(MiniYamlNode::new(k.clone(), MiniYaml::new(v.value.clone(), children)));
        }

        let nodes = MiniYaml::new(Some(String::new()), resolved_nodes);
        resolve_inherits(&nodes, &tree, &Inherited::default())
    }
}

/// Serializes nodes to lines.
pub fn nodes_to_lines(nodes: &[MiniYamlNode], out: &mut Vec<String>) {
    for n in nodes {
        n.value.to_lines(&n.key, n.comment.as_deref(), out);
    }
}

/// Serializes nodes like `WriteToString` in C#: lines joined by "\n", one final newline.
pub fn nodes_to_string(nodes: &[MiniYamlNode]) -> String {
    let mut lines = Vec::new();
    nodes_to_lines(nodes, &mut lines);
    let joined = lines.join("\n");
    format!("{}\n", joined.trim_end_matches('\n'))
}

struct ParsedLine {
    level: usize,
    key: String,
    value: Option<String>,
    comment: Option<String>,
    location: SourceLocation,
}

fn trim_chars(s: &[char]) -> &[char] {
    let start = s.iter().position(|c| !c.is_whitespace()).unwrap_or(s.len());
    let end = s.iter().rposition(|c| !c.is_whitespace()).map_or(start, |e| e + 1);
    &s[start..end.max(start)]
}

fn from_lines<'a>(lines: impl Iterator<Item = &'a str>, name: &str, discard: bool) -> Result<Vec<MiniYamlNode>, YamlError> {
    let name: Arc<str> = Arc::from(name);
    let mut result: Vec<Vec<MiniYamlNode>> = vec![Vec::new()];
    let mut parsed: Vec<ParsedLine> = Vec::new();

    for (idx, line_str) in lines.enumerate() {
        let line: Vec<char> = line_str.chars().collect();
        let location = SourceLocation {
            name: name.clone(),
            line: (idx + 1) as u32,
        };
        let mut level = 0usize;
        let mut key: &[char] = &[];
        let mut value: Option<Vec<char>> = None;
        let mut comment: Option<&[char]> = None;

        if !line.is_empty() {
            let mut key_start = 0usize;
            let mut spaces = 0usize;
            let mut text_start = false;
            let mut curr = line[0];
            while !(curr == '\n' || curr == '\r') && key_start < line.len() && !text_start {
                curr = line[key_start];
                match curr {
                    ' ' => {
                        spaces += 1;
                        if spaces >= SPACES_PER_LEVEL {
                            spaces = 0;
                            level += 1;
                        }
                        key_start += 1;
                    }
                    '\t' => {
                        level += 1;
                        key_start += 1;
                    }
                    _ => text_start = true,
                }
            }

            let mut key_length = line.len() as isize - key_start as isize;
            let mut value_start: isize = -1;
            let mut value_length: isize = 0;
            let mut comment_start: isize = -1;
            for i in 0..line.len() {
                if value_start < 0 && line[i] == ':' {
                    value_start = i as isize + 1;
                    key_length = i as isize - key_start as isize;
                    value_length = line.len() as isize - i as isize - 1;
                }
                if comment_start < 0 && line[i] == '#' && (i == 0 || line[i - 1] != '\\') {
                    comment_start = i as isize + 1;
                    if i as isize <= key_start as isize + key_length {
                        key_length = i as isize - key_start as isize;
                    } else {
                        value_length = i as isize - value_start;
                    }
                    break;
                }
            }

            if key_length > 0 {
                key = trim_chars(&line[key_start..key_start + key_length as usize]);
            }
            if value_start >= 0 {
                let vs = value_start as usize;
                let trimmed = trim_chars(&line[vs..vs + value_length as usize]);
                if !trimmed.is_empty() {
                    value = Some(trimmed.to_vec());
                }
            }
            if comment_start >= 0 && !discard {
                comment = Some(&line[comment_start as usize..]);
            }

            if let Some(v) = value.as_mut().filter(|v| v.len() > 1) {
                let n = v.len();
                let lead = usize::from(v[0] == '\\' && (v[1] == ' ' || v[1] == '\t'));
                let trail = usize::from(v[n - 1] == '\\' && (v[n - 2] == ' ' || v[n - 2] == '\t'));
                if lead + trail > 0 {
                    *v = v[lead..n - trail].to_vec();
                }
                let s: String = v.iter().collect();
                if s.contains("\\#") {
                    *v = s.replace("\\#", "#").chars().collect();
                }
            }
        }

        if !key.is_empty() || !discard {
            if let Some(last) = parsed.last()
                && (last.level as isize) < level as isize - 1
            {
                return Err(YamlError(format!("Bad indent in miniyaml at {location}")));
            }
            while parsed.last().is_some_and(|l| l.level > level) {
                build_completed_sub_node(level, &mut parsed, &mut result);
            }
            parsed.push(ParsedLine {
                level,
                key: key.iter().collect(),
                value: value.map(|v| v.into_iter().collect()).filter(|v: &String| !v.is_empty()),
                comment: comment.map(|c| c.iter().collect()),
                location,
            });
        }
    }

    if !parsed.is_empty() {
        build_completed_sub_node(0, &mut parsed, &mut result);
    }

    Ok(result.swap_remove(0))
}

fn build_completed_sub_node(level: usize, parsed: &mut Vec<ParsedLine>, result: &mut Vec<Vec<MiniYamlNode>>) {
    let last_level = parsed.last().unwrap().level;
    while last_level >= result.len() {
        result.push(Vec::new());
    }

    while parsed.last().is_some_and(|l| l.level >= level) {
        let parent_level = parsed.last().unwrap().level;
        let mut start = parsed.len() - 1;
        while start > 0 && parsed[start - 1].level == parent_level {
            start -= 1;
        }

        let range: Vec<ParsedLine> = parsed.drain(start..).collect();
        let (siblings, parent) = range.split_at(range.len() - 1);
        for s in siblings {
            result[parent_level].push(MiniYamlNode {
                key: s.key.clone(),
                value: MiniYaml::new(s.value.clone(), Vec::new()),
                comment: s.comment.clone(),
                location: s.location.clone(),
            });
        }

        let parent = &parent[0];
        let children = if parent_level + 1 < result.len() {
            std::mem::take(&mut result[parent_level + 1])
        } else {
            Vec::new()
        };
        result[parent_level].push(MiniYamlNode {
            key: parent.key.clone(),
            value: MiniYaml::new(parent.value.clone(), children),
            comment: parent.comment.clone(),
            location: parent.location.clone(),
        });
    }
}

/// Parent types inherited along the current resolution path (an immutable dictionary in C#).
#[derive(Clone, Default)]
struct Inherited(HashMap<String, SourceLocation>);

impl Inherited {
    fn add(&mut self, key: String, loc: SourceLocation) -> Option<SourceLocation> {
        if let Some(existing) = self.0.get(&key) {
            return Some(existing.clone());
        }
        self.0.insert(key, loc);
        None
    }
}

fn index_of_key(nodes: &[MiniYamlNode], key: &str) -> Option<usize> {
    nodes.iter().position(|n| n.key == key)
}

fn last_index_of_key(nodes: &[MiniYamlNode], key: &str) -> Option<usize> {
    nodes.iter().rposition(|n| n.key == key)
}

fn check_conflicts(nodes: &[MiniYamlNode]) -> Result<(), YamlError> {
    let mut seen: HashMap<&str, &MiniYamlNode> = HashMap::with_capacity(nodes.len());
    let mut dups: Vec<String> = Vec::new();
    for n in nodes.iter().filter(|n| !n.key.is_empty()) {
        if let Some(first) = seen.get(n.key.as_str()) {
            dups.push(format!(
                "{}: [{} (at {}), {} (at {})]",
                n.key, first.key, first.location, n.key, n.location
            ));
        } else {
            seen.insert(&n.key, n);
        }
    }
    if dups.is_empty() {
        Ok(())
    } else {
        Err(YamlError(format!(
            "MiniYaml.Merge, duplicate values found for the following keys: {}",
            dups.join(", ")
        )))
    }
}

fn merge_self_partial(nodes: Vec<MiniYamlNode>) -> Result<Vec<MiniYamlNode>, YamlError> {
    let mut keys = HashSet::with_capacity(nodes.len());
    let mut ret: Vec<MiniYamlNode> = Vec::with_capacity(nodes.len());
    for n in nodes {
        if keys.insert(n.key.clone()) {
            ret.push(n);
        } else {
            let idx = index_of_key(&ret, &n.key).unwrap();
            let merged = merge_partial_yaml(Some(&ret[idx].value), Some(&n.value))?;
            ret[idx] = ret[idx].with_value(merged);
        }
    }
    Ok(ret)
}

fn merge_partial_yaml(existing: Option<&MiniYaml>, overrides: Option<&MiniYaml>) -> Result<MiniYaml, YamlError> {
    if let Some(e) = existing {
        check_conflicts(&e.nodes)?;
    }
    if let Some(o) = overrides {
        check_conflicts(&o.nodes)?;
    }
    match (existing, overrides) {
        (None, Some(o)) => Ok(o.clone()),
        (Some(e), None) => Ok(e.clone()),
        (None, None) => Ok(MiniYaml::default()),
        (Some(e), Some(o)) => Ok(MiniYaml::new(
            o.value.clone().or_else(|| e.value.clone()),
            merge_partial_nodes(e.nodes.clone(), o.nodes.clone())?,
        )),
    }
}

fn merge_partial_nodes(existing: Vec<MiniYamlNode>, overrides: Vec<MiniYamlNode>) -> Result<Vec<MiniYamlNode>, YamlError> {
    if existing.is_empty() {
        return Ok(overrides);
    }
    if overrides.is_empty() {
        return Ok(existing);
    }

    let mut ret: Vec<MiniYamlNode> = Vec::with_capacity(existing.len() + overrides.len());
    let mut plain_keys: HashSet<String> = HashSet::new();
    for node in existing.into_iter().chain(overrides) {
        if node.key.starts_with('-') {
            ret.push(node);
            continue;
        }
        if plain_keys.insert(node.key.clone()) {
            ret.push(node);
            continue;
        }
        let prev = last_index_of_key(&ret, &node.key).unwrap();
        let removal = last_index_of_key(&ret, &format!("-{}", node.key));
        if removal.is_some_and(|r| r > prev) {
            ret.push(node);
            continue;
        }
        let merged = merge_partial_yaml(Some(&ret[prev].value), Some(&node.value))?;
        ret[prev] = node.with_value(merged);
    }
    Ok(ret)
}

fn merge_into_resolved(
    override_node: &MiniYamlNode,
    existing: &mut Vec<MiniYamlNode>,
    existing_keys: &mut HashSet<String>,
    tree: &indexmap::IndexMap<String, MiniYaml>,
    inherited: &Inherited,
) -> Result<(), YamlError> {
    let existing_index = if existing_keys.insert(override_node.key.clone()) {
        None
    } else {
        index_of_key(existing, &override_node.key)
    };
    let existing_value = existing_index.map(|i| &existing[i].value);
    let mut value = merge_partial_yaml(existing_value, Some(&override_node.value))?;
    let nodes = resolve_inherits(&value, tree, inherited)?;
    if value.nodes != nodes {
        value.nodes = nodes;
    }
    match existing_index {
        Some(i) => existing[i] = existing[i].with_value(value),
        None => existing.push(override_node.with_value(value)),
    }
    Ok(())
}

fn resolve_inherits(
    node: &MiniYaml,
    tree: &indexmap::IndexMap<String, MiniYaml>,
    inherited: &Inherited,
) -> Result<Vec<MiniYamlNode>, YamlError> {
    if node.nodes.is_empty() {
        return Ok(Vec::new());
    }

    let mut inherited = inherited.clone();
    let mut resolved: Vec<MiniYamlNode> = Vec::with_capacity(node.nodes.len());
    let mut resolved_keys: HashSet<String> = HashSet::with_capacity(node.nodes.len());

    for n in &node.nodes {
        if n.key == "Inherits" || n.key.starts_with("Inherits@") {
            let parent_name = n.value.value_str();
            let parent = tree
                .get(parent_name)
                .ok_or_else(|| YamlError(format!("{}: Parent type `{}` not found", n.location, parent_name)))?;
            if let Some(prev) = inherited.add(parent_name.to_string(), n.location.clone()) {
                return Err(YamlError(format!(
                    "{}: Parent type `{}` was already inherited by this yaml tree at {} (note: may be from a derived tree)",
                    n.location, parent_name, prev
                )));
            }
            for r in resolve_inherits(parent, tree, &inherited)? {
                merge_into_resolved(&r, &mut resolved, &mut resolved_keys, tree, &inherited)?;
            }
        } else if let Some(removed) = n.key.strip_prefix('-') {
            let before = resolved.len();
            resolved.retain(|r| r.key != removed);
            if resolved.len() == before {
                return Err(YamlError(format!(
                    "{}: There are no elements with key `{}` to remove",
                    n.location, removed
                )));
            }
            resolved_keys.remove(removed);
        } else {
            merge_into_resolved(n, &mut resolved, &mut resolved_keys, tree, &inherited)?;
        }
    }

    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Vec<MiniYamlNode> {
        MiniYaml::from_str(s, "test", true).unwrap()
    }

    #[test]
    fn parses_nested_nodes_and_values() {
        let n = parse("A: 1\n\tB: 2 # comment\n\t\tC:\nD: \\ x \\\nE: a\\#b\n");
        assert_eq!(n.len(), 3);
        assert_eq!(n[0].key, "A");
        assert_eq!(n[0].value.value.as_deref(), Some("1"));
        assert_eq!(n[0].value.nodes[0].value.value.as_deref(), Some("2"));
        assert_eq!(n[0].value.nodes[0].value.nodes[0].key, "C");
        assert_eq!(n[0].value.nodes[0].value.nodes[0].value.value, None);
        assert_eq!(n[1].value.value.as_deref(), Some(" x "));
        assert_eq!(n[2].value.value.as_deref(), Some("a#b"));
    }

    #[test]
    fn spaces_count_as_levels() {
        let n = parse("A:\n    B: 1\n        C: 2\n");
        assert_eq!(n[0].value.nodes[0].value.nodes[0].key, "C");
    }

    #[test]
    fn bad_indent_is_an_error() {
        assert!(MiniYaml::from_str("A:\n\t\tB:\n", "t", true).is_err());
    }

    #[test]
    fn merge_resolves_inheritance_and_removal() {
        let base = parse(
            "^Base:\n\tHealth:\n\t\tHP: 100\n\tArmor:\n\t\tType: Light\nUnit:\n\tInherits: ^Base\n\tHealth:\n\t\tHP: 200\n\t-Armor:\n",
        );
        let merged = MiniYaml::merge(vec![base]).unwrap();
        let unit = merged.iter().find(|n| n.key == "Unit").unwrap();
        assert_eq!(unit.value.nodes.len(), 1);
        assert_eq!(unit.value.nodes[0].key, "Health");
        assert_eq!(unit.value.nodes[0].value.nodes[0].value.value.as_deref(), Some("200"));
    }

    #[test]
    fn later_sources_override_and_removal_then_readd() {
        let a = parse("Unit:\n\tFoo:\n\t\tX: 1\n");
        let b = parse("Unit:\n\t-Foo:\n\tFoo:\n\t\tY: 2\n");
        let merged = MiniYaml::merge(vec![a, b]).unwrap();
        let unit = &merged[0];
        assert_eq!(unit.value.nodes.len(), 1);
        assert_eq!(unit.value.nodes[0].value.nodes.len(), 1);
        assert_eq!(unit.value.nodes[0].value.nodes[0].key, "Y");
    }

    #[test]
    fn round_trip_serialization() {
        let src = "A: 1\n\tB: x\\#y\nC:\n";
        assert_eq!(nodes_to_string(&parse(src)), src);
    }
}
