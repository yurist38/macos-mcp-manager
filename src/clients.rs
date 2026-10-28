use crate::storage::{Parked, ParkedServer};
use anyhow::{Context, Result, bail};
use jsonc_parser::{
    ParseOptions,
    cst::{CstInputValue, CstNode, CstObject, CstRootNode},
};
use serde::Serialize;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};
use toml_edit::{DocumentMut, Item};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum ClientKind {
    Codex,
    ClaudeDesktop,
    ClaudeCode,
    VsCode,
    Cursor,
}

impl ClientKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::ClaudeDesktop => "Claude Desktop",
            Self::ClaudeCode => "Claude Code",
            Self::VsCode => "VS Code / Copilot",
            Self::Cursor => "Cursor",
        }
    }

    pub fn guidance(self) -> &'static str {
        match self {
            Self::Codex => {
                "User configuration · shared by Codex clients; restart MCP or start a new session"
            }
            Self::ClaudeDesktop => {
                "User configuration · disabled definitions kept privately; fully restart Claude"
            }
            Self::ClaudeCode => {
                "User definitions only · disabled definitions kept privately; start a new session"
            }
            Self::VsCode => {
                "User definitions only · checkmark means present, not VS Code's separate enablement; reload window"
            }
            Self::Cursor => {
                "User definitions only · disabled definitions kept privately; reload MCP or restart Cursor"
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Source {
    pub kind: ClientKind,
    pub path: PathBuf,
}

impl Source {
    pub fn json_key(&self) -> &'static str {
        if self.kind == ClientKind::VsCode {
            "servers"
        } else {
            "mcpServers"
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum Target {
    Toml(Vec<String>),
    Json(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Server {
    pub name: String,
    pub enabled: bool,
    pub can_toggle: bool,
    pub target: Target,
}

pub fn sources(home: &Path, codex_home: Option<PathBuf>) -> Vec<Source> {
    vec![
        Source {
            kind: ClientKind::Codex,
            path: codex_home
                .unwrap_or_else(|| home.join(".codex"))
                .join("config.toml"),
        },
        Source {
            kind: ClientKind::ClaudeDesktop,
            path: home.join("Library/Application Support/Claude/claude_desktop_config.json"),
        },
        Source {
            kind: ClientKind::ClaudeCode,
            path: home.join(".claude.json"),
        },
        Source {
            kind: ClientKind::VsCode,
            path: home.join("Library/Application Support/Code/User/mcp.json"),
        },
        Source {
            kind: ClientKind::Cursor,
            path: home.join(".cursor/mcp.json"),
        },
    ]
}

pub fn parked_key(path: &Path, name: &str) -> String {
    serde_json::to_string(&(path, name)).expect("paths and names serialize")
}

pub fn inventory(source: &Source, contents: &str, parked: &Parked) -> Result<Vec<Server>> {
    let mut servers = if source.kind == ClientKind::Codex {
        toml_inventory(contents)?
    } else {
        let root = parse_json(contents)?;
        let object = root
            .object_value()
            .context("Configuration must be an object")?;
        let mut rows = Vec::new();
        if let Some(property) = object.get(source.json_key()) {
            let definitions = property
                .value()
                .and_then(|v| v.as_object())
                .context("Server definitions must be an object")?;
            for prop in definitions.properties() {
                let name = prop.decoded_name().context("Invalid server name")?;
                if prop.value().and_then(|v| v.as_object()).is_none() {
                    bail!("A server definition is not an object");
                }
                if parked.contains_key(&parked_key(&source.path, &name)) {
                    bail!(
                        "A disabled server was also added externally. Restore or rename the conflicting definition before toggling."
                    );
                }
                rows.push(Server {
                    target: Target::Json(name.clone()),
                    name,
                    enabled: true,
                    can_toggle: true,
                });
            }
        }
        rows
    };
    for value in parked.values().filter(|v| v.source == source.path) {
        servers.push(Server {
            name: value.name.clone(),
            enabled: false,
            can_toggle: true,
            target: Target::Json(value.name.clone()),
        });
    }
    servers.sort_by_key(|server| server.name.to_lowercase());
    Ok(servers)
}

fn parse_toml(contents: &str) -> Result<DocumentMut> {
    contents.parse().map_err(|_| {
        anyhow::anyhow!("Invalid TOML configuration. Fix it in the client before making changes.")
    })
}

fn toml_inventory(contents: &str) -> Result<Vec<Server>> {
    let document = parse_toml(contents)?;
    let mut rows = Vec::new();
    if let Some(item) = document.get("mcp_servers") {
        collect_toml_servers(item, &["mcp_servers".into()], None, true, &mut rows)?;
    }
    // Existing plugin-scoped policy is safe to edit. We never infer installed plugins from a cache.
    if let Some(plugins) = document.get("plugins") {
        for (name, plugin) in plugins
            .as_table_like()
            .context("plugins must be a table")?
            .iter()
        {
            let plugin = plugin
                .as_table_like()
                .context("Plugin configuration must be a table")?;
            let enabled = bool_setting(plugin.get("enabled"))?;
            rows.push(Server {
                name: format!("Plugin: {name} (whole plugin)"),
                enabled,
                can_toggle: true,
                target: Target::Toml(vec!["plugins".into(), name.into()]),
            });
            if let Some(servers) = plugin.get("mcp_servers") {
                collect_toml_servers(
                    servers,
                    &["plugins".into(), name.into(), "mcp_servers".into()],
                    Some(name),
                    enabled,
                    &mut rows,
                )?;
            }
        }
    }
    Ok(rows)
}

fn collect_toml_servers(
    item: &Item,
    prefix: &[String],
    plugin: Option<&str>,
    parent_enabled: bool,
    rows: &mut Vec<Server>,
) -> Result<()> {
    let table = item
        .as_table_like()
        .context("MCP servers must be a table")?;
    for (name, entry) in table.iter() {
        let server = entry
            .as_table_like()
            .context("MCP server must be a table")?;
        let mut path = prefix.to_vec();
        path.push(name.into());
        let title = match plugin {
            Some(plugin) if !parent_enabled => format!("{name} ({plugin}; plugin disabled)"),
            Some(plugin) => format!("{name} ({plugin})"),
            None => name.into(),
        };
        rows.push(Server {
            name: title,
            enabled: bool_setting(server.get("enabled"))? && parent_enabled,
            can_toggle: parent_enabled,
            target: Target::Toml(path),
        });
    }
    Ok(())
}

fn bool_setting(item: Option<&Item>) -> Result<bool> {
    item.map(|v| v.as_bool().context("enabled must be true or false"))
        .unwrap_or(Ok(true))
}

pub fn edit(
    source: &Source,
    contents: &str,
    server: &Server,
    parked: &mut Parked,
) -> Result<String> {
    if !server.can_toggle {
        bail!("Enable the parent plugin in Codex first");
    }
    match &server.target {
        Target::Toml(path) => {
            let mut document = parse_toml(contents)?;
            let mut item = document.as_item_mut();
            for segment in path {
                item = item
                    .as_table_like_mut()
                    .and_then(|t| t.get_mut(segment))
                    .context("Server changed; wait for the menu to update before toggling")?;
            }
            let table = item.as_table_like_mut().context("Invalid server table")?;
            let mut replacement = toml_edit::value(!server.enabled);
            if let Some(existing) = table.get("enabled").and_then(Item::as_value) {
                *replacement
                    .as_value_mut()
                    .expect("boolean value")
                    .decor_mut() = existing.decor().clone();
            }
            table.insert("enabled", replacement);
            Ok(document.to_string())
        }
        Target::Json(name) => {
            let root = parse_json(contents)?;
            let object = root
                .object_value()
                .context("Configuration must be an object")?;
            let definitions = match object.get(source.json_key()) {
                Some(prop) => prop
                    .value()
                    .and_then(|v| v.as_object())
                    .context("Server definitions must be an object")?,
                None if !server.enabled => object
                    .append(source.json_key(), CstInputValue::Object(Vec::new()))
                    .value()
                    .and_then(|v| v.as_object())
                    .context("Cannot create server definitions")?,
                None => bail!("Server disappeared; wait for the menu to update before toggling"),
            };
            let key = parked_key(&source.path, name);
            if server.enabled {
                let property = definitions
                    .get(name)
                    .context("Server disappeared; wait for the menu to update before toggling")?;
                let definition = property
                    .value()
                    .context("Server has no definition")?
                    .to_string();
                if parked.contains_key(&key) {
                    bail!("A disabled copy already exists; refusing to overwrite it");
                }
                property.remove();
                parked.insert(
                    key,
                    ParkedServer {
                        source: source.path.clone(),
                        name: name.clone(),
                        definition,
                    },
                );
                let result = root.to_string();
                parse_json(&result)?;
                Ok(result)
            } else {
                if definitions.get(name).is_some() {
                    bail!("Server was added externally; refusing to overwrite it");
                }
                let saved = parked.get(&key).context("Disabled definition is missing")?;
                let saved_root = parse_json(&saved.definition)?;
                if saved_root.object_value().is_none() {
                    bail!("Disabled definition is not an object");
                }
                // Splice the raw value into a unique placeholder, preserving comments and all credentials.
                let marker = (0_u64..)
                    .map(|n| format!("__mcp_manager_restore_{n}__"))
                    .find(|marker| !contents.contains(marker) && !saved.definition.contains(marker))
                    .expect("unique marker");
                definitions.append(name, CstInputValue::String(marker.clone()));
                let result = root.to_string().replacen(
                    &serde_json::to_string(&marker)?,
                    &saved.definition,
                    1,
                );
                parse_json(&result)?;
                parked.remove(&key);
                Ok(result)
            }
        }
    }
}

pub fn parse_json(contents: &str) -> Result<CstRootNode> {
    // Accept JSONC, not the parser's more permissive JSON5 defaults.
    let options = ParseOptions {
        allow_comments: true,
        allow_trailing_commas: true,
        allow_loose_object_property_names: false,
        allow_missing_commas: false,
        allow_single_quoted_strings: false,
        allow_hexadecimal_numbers: false,
        allow_unary_plus_numbers: false,
        allow_bare_decimal_point_numbers: false,
        allow_non_finite_numbers: false,
        allow_extended_string_escapes: false,
    };
    let root = CstRootNode::parse(contents, &options).map_err(|_| {
        anyhow::anyhow!(
            "Invalid JSON/JSONC configuration. Fix it in the client before making changes."
        )
    })?;
    if let Some(value) = root.value() {
        reject_duplicate_keys(value)?;
    }
    Ok(root)
}

fn reject_duplicate_keys(node: CstNode) -> Result<()> {
    if let Some(object) = node.as_object() {
        check_object(object)?;
    }
    if let Some(array) = node.as_array() {
        for value in array.elements() {
            reject_duplicate_keys(value)?;
        }
    }
    Ok(())
}

fn check_object(object: CstObject) -> Result<()> {
    let mut seen = BTreeSet::new();
    for prop in object.properties() {
        let name = prop.decoded_name().context("Invalid JSON property name")?;
        if !seen.insert(name) {
            bail!("Duplicate JSON keys make this configuration ambiguous; fix it before toggling");
        }
        if let Some(value) = prop.value() {
            reject_duplicate_keys(value)?;
        }
    }
    Ok(())
}
