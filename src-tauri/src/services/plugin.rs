//! MCP ve (ileride) Lounge `plugins/` dizini — marketplace değil, katalog.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::models::{now_rfc3339, DiscoveredTool, ServiceHealth, ServiceId};

use super::autodiscover::{
    merge_plugins_by_name, scan_antigravity, scan_claude_desktop, scan_cursor,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginCatalog {
    pub scanned_at: String,
    pub plugins: Vec<DiscoveredTool>,
    pub lounge_dir: Option<String>,
}

impl PluginCatalog {
    pub fn host_count(&self) -> usize {
        let mut hosts = HashSet::new();
        for plugin in &self.plugins {
            for host in plugin.hosts() {
                hosts.insert(host);
            }
        }
        hosts.len()
    }
}

pub fn scan_plugin_catalog(workspace: &Path) -> PluginCatalog {
    let (_, claude) = scan_claude_desktop();
    let (_, cursor) = scan_cursor(workspace);
    let (_, antigravity) = scan_antigravity(workspace);
    let mut plugins = claude;
    plugins.extend(cursor);
    plugins.extend(antigravity);
    plugins.retain(|tool| tool.kind == "plugin" || tool.kind == "mcp");
    let plugins = merge_plugins_by_name(plugins);
    let lounge_dir = lounge_plugin_dir(workspace);
    PluginCatalog {
        scanned_at: now_rfc3339(),
        plugins,
        lounge_dir: lounge_dir
            .exists()
            .then(|| lounge_dir.display().to_string()),
    }
}

pub fn catalog_from_plugins(plugins: Vec<DiscoveredTool>) -> PluginCatalog {
    PluginCatalog {
        scanned_at: now_rfc3339(),
        plugins,
        lounge_dir: None,
    }
}

pub fn plugin_health(catalog: &PluginCatalog) -> ServiceHealth {
    let count = catalog.plugins.len();
    let hosts = catalog.host_count();
    ServiceHealth {
        id: ServiceId::Plugin,
        name: "Plugin catalog".into(),
        running: true,
        started_by_us: false,
        endpoint: catalog.lounge_dir.clone().unwrap_or_else(|| "mcp".into()),
        detail: Some(format!("{count} plugin · {hosts} host")),
        error: None,
    }
}

pub fn lounge_workspace() -> PathBuf {
    std::env::var_os("LOUNGE_WORKSPACE")
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
        .unwrap_or_else(super::probe::data_root)
}

fn lounge_plugin_dir(workspace: &Path) -> PathBuf {
    workspace.join("plugins")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_catalog_still_reports_health() {
        let catalog = catalog_from_plugins(Vec::new());
        let health = plugin_health(&catalog);
        assert!(health.running);
        assert_eq!(health.detail.as_deref(), Some("0 plugin · 0 host"));
    }

    #[test]
    fn host_count_follows_host_id() {
        let catalog = catalog_from_plugins(vec![
            DiscoveredTool::new("cursor", "notion", "plugin"),
            DiscoveredTool::new("cursor", "browser", "plugin"),
            DiscoveredTool::new("claude_desktop", "github", "plugin"),
        ]);
        assert_eq!(catalog.host_count(), 2);
        let health = plugin_health(&catalog);
        assert_eq!(health.detail.as_deref(), Some("3 plugin · 2 host"));
    }

    #[test]
    fn merged_plugin_counts_each_host_once() {
        let notion = {
            let mut tool = DiscoveredTool::new("cursor", "notion", "plugin");
            tool.host_ids = vec!["cursor".into(), "antigravity".into()];
            tool
        };
        let catalog = catalog_from_plugins(vec![notion]);
        assert_eq!(catalog.host_count(), 2);
    }
}
