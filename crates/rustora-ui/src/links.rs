// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! Links in chat lines: finding them and opening them in the browser.

use rustora_core::fieldloader::Color;

/// The color of links in chat lines.
pub const LINK_COLOR: Color = Color::from_argb(255, 110, 170, 255);

const PREFIXES: [&str; 3] = ["https://", "http://", "www."];
const LEADING: &[char] = &['(', '<', '[', '"', '\''];
const TRAILING: &[char] = &['.', ',', ';', ':', '!', '?', ')', ']', '}', '>', '"', '\''];

/// The parts of one line of text, each with the address that it links to, if it is a link. Links are words that
/// start with `http://`, `https://` or `www.`, without enclosing brackets and quotes and without punctuation at the
/// end.
pub fn link_components(line: &str) -> Vec<(&str, Option<String>)> {
    let mut parts = Vec::new();
    let mut plain_start = 0;
    let mut word_start = None;
    for (i, c) in line.char_indices().chain(std::iter::once((line.len(), ' '))) {
        match (c.is_whitespace(), word_start) {
            (false, None) => word_start = Some(i),
            (true, Some(start)) => {
                word_start = None;
                let word = &line[start..i];
                let trimmed = word.trim_start_matches(LEADING);
                let link_start = start + word.len() - trimmed.len();
                let link = trimmed.trim_end_matches(TRAILING);
                let Some(prefix) = PREFIXES.iter().find(|p| link.starts_with(**p) && link.len() > p.len()) else {
                    continue;
                };
                if link_start > plain_start {
                    parts.push((&line[plain_start..link_start], None));
                }
                let address = if *prefix == "www." {
                    format!("https://{link}")
                } else {
                    link.to_string()
                };
                parts.push((link, Some(address)));
                plain_start = link_start + link.len();
            }
            _ => {}
        }
    }
    if plain_start < line.len() || parts.is_empty() {
        parts.push((&line[plain_start..], None));
    }
    parts
}

/// Whether the text has a link.
pub fn has_link(text: &str) -> bool {
    text.lines()
        .any(|line| link_components(line).iter().any(|(_, link)| link.is_some()))
}

/// Opens the address in the browser of the system.
pub fn open(address: &str) {
    if let Err(e) = open::that_detached(address) {
        log::warn!("Cannot open {address}: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn links(line: &str) -> Vec<String> {
        link_components(line).into_iter().filter_map(|(_, l)| l).collect()
    }

    #[test]
    fn finds_links() {
        assert_eq!(links("see https://www.openra.net."), ["https://www.openra.net"]);
        assert_eq!(
            links("(www.openra.net/x?a=1), http://a.b"),
            ["https://www.openra.net/x?a=1", "http://a.b"]
        );
        assert!(links("no link: https:// or www. here").is_empty());
        let parts = link_components("a https://x.y b");
        assert_eq!(parts.iter().map(|p| p.0).collect::<String>(), "a https://x.y b");
        assert_eq!(parts.len(), 3);
    }

    #[test]
    fn lines_without_links_stay_whole() {
        assert_eq!(link_components("plain text"), vec![("plain text", None)]);
        assert_eq!(link_components(""), vec![("", None)]);
    }
}
