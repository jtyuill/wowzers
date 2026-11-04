//! `.toc` files: the metadata every addon folder carries.

use std::{collections::HashMap, path::Path};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Toc {
    /// Display title with WoW colour/texture escapes removed.
    pub title: Option<String>,
    pub notes: Option<String>,
    pub author: Option<String>,
    pub version: Option<String>,
    pub interface: Vec<u32>,
    pub dependencies: Vec<String>,
    /// `X-Curse-Project-ID`, written by CurseForge's packager.
    pub curse_project_id: Option<u32>,
}

impl Toc {
    pub fn parse(text: &str) -> Toc {
        let mut fields: HashMap<String, String> = HashMap::new();
        for line in text.trim_start_matches('\u{feff}').lines() {
            let Some(rest) = line.trim().strip_prefix("##") else {
                continue;
            };
            let Some((key, value)) = rest.split_once(':') else {
                continue;
            };
            // First occurrence wins, like the game client.
            fields
                .entry(key.trim().to_ascii_lowercase())
                .or_insert_with(|| value.trim().to_string());
        }
        let get = |key: &str| fields.get(key).filter(|v| !v.is_empty()).cloned();
        let list = |keys: &[&str]| -> Vec<String> {
            keys.iter()
                .filter_map(|k| get(k))
                .flat_map(|v| {
                    v.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                })
                .collect()
        };
        Toc {
            title: get("title")
                .map(|t| strip_escapes(&t))
                .filter(|t| !t.is_empty()),
            notes: get("notes").map(|t| strip_escapes(&t)),
            author: get("author").map(|t| strip_escapes(&t)),
            version: get("version"),
            interface: list(&["interface"])
                .iter()
                .filter_map(|v| v.parse().ok())
                .collect(),
            dependencies: list(&["dependencies", "requireddeps"]),
            curse_project_id: get("x-curse-project-id").and_then(|v| v.parse().ok()),
        }
    }

    pub fn read(path: &Path) -> std::io::Result<Toc> {
        // TOCs are UTF-8 in practice, but old ones are Latin-1.
        Ok(Toc::parse(&String::from_utf8_lossy(&std::fs::read(path)?)))
    }
}

/// Removes `|cAARRGGBB`, `|r`, `|Tpath:..|t`, `|Aatlas:..|a` and turns `|n`
/// into a space.
pub fn strip_escapes(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '|' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        match chars.get(i + 1).copied() {
            Some('c' | 'C')
                if chars.len() >= i + 10
                    && chars[i + 2..i + 10].iter().all(char::is_ascii_hexdigit) =>
            {
                i += 10
            }
            Some('r' | 'R') => i += 2,
            Some('n' | 'N') => {
                out.push(' ');
                i += 2
            }
            Some(open @ ('T' | 't' | 'A' | 'a')) => {
                let close = if open.is_ascii_uppercase() {
                    open.to_ascii_lowercase()
                } else {
                    open
                };
                // Skip to the matching closer, or past the escape if there is none.
                match chars[i + 2..].iter().position(|&c| c == '|').filter(|&p| {
                    chars
                        .get(i + 2 + p + 1)
                        .is_some_and(|c| c.eq_ignore_ascii_case(&close))
                }) {
                    Some(p) => i += 2 + p + 2,
                    None => i += 2,
                }
            }
            Some('|') => {
                out.push('|');
                i += 2
            }
            _ => {
                out.push('|');
                i += 1
            }
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fields_and_ignores_comments() {
        let toc = Toc::parse(
            "\u{feff}## Interface: 110002, 110005\r\n## Title: |cff00ff00Deadly|r Boss |TInterface\\Icons\\x:16|t Mods\r\n\
             ## Notes: Warns you.\n## Version: 1.2.3\n## X-Curse-Project-ID: 3358\n## Dependencies: Ace3, LibStub\n\
             ## Title: ignored duplicate\n# plain comment\nCore.lua\n",
        );
        assert_eq!(toc.title.as_deref(), Some("Deadly Boss  Mods"));
        assert_eq!(toc.version.as_deref(), Some("1.2.3"));
        assert_eq!(toc.interface, vec![110002, 110005]);
        assert_eq!(toc.curse_project_id, Some(3358));
        assert_eq!(toc.dependencies, vec!["Ace3", "LibStub"]);
    }

    #[test]
    fn missing_or_garbage_project_id_is_none() {
        assert_eq!(
            Toc::parse("## X-Curse-Project-ID: nope").curse_project_id,
            None
        );
        assert_eq!(Toc::parse("## Title: x").curse_project_id, None);
    }

    #[test]
    fn strip_escapes_handles_stray_pipes() {
        assert_eq!(strip_escapes("a||b"), "a|b");
        assert_eq!(strip_escapes("|cffff0000red|r|nnext"), "red next");
        assert_eq!(strip_escapes("trailing |"), "trailing |");
    }
}
