//! The MCP store: the 300 most viewed servers dotbot can run from ModelScope's MCP 广场
//! (modelscope.cn/mcp), a domestic marketplace with Chinese names and descriptions. Each entry
//! carries its install command or streamable-HTTP address and the settings it needs, so adding
//! one needs no README. The snapshot is `mcp_store.json`; refresh it with dev/mcp-store-index.py.
use serde_json::Value;

pub fn index() -> &'static Value {
    static INDEX: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    INDEX.get_or_init(|| serde_json::from_str(include_str!("mcp_store.json")).expect("valid MCP store index"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_snapshot_holds_runnable_chinese_servers_in_known_categories() {
        let index = index();
        assert!(index["source"].as_str().unwrap().contains("ModelScope"));
        let categories: Vec<&str> = index["categories"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap()).collect();
        let servers = index["servers"].as_array().unwrap();
        assert!((250..=300).contains(&servers.len()), "{} servers", servers.len());
        let chinese = |text: &str| text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c));
        let mut ids = std::collections::HashSet::new();
        for server in servers {
            assert!(ids.insert(server["id"].as_str().unwrap()), "duplicate id");
            assert!(categories.contains(&server["cat"].as_str().unwrap()), "unknown category");
            assert!(chinese(server["desc"].as_str().unwrap()), "{} has no Chinese description", server["id"]);
            assert!(!server["name"].as_str().unwrap().is_empty());
            match server["kind"].as_str().unwrap() {
                "stdio" => {
                    assert!(["npx", "uvx"].contains(&server["command"].as_str().unwrap()));
                    assert!(!server["args"].as_array().unwrap().is_empty());
                }
                "http" => assert!(server["url"].as_str().unwrap().starts_with("https://")),
                other => panic!("unknown kind {other}"),
            }
            for env in server["env"].as_array().unwrap() { assert!(!env["name"].as_str().unwrap().is_empty()); }
        }
        let views: Vec<u64> = servers.iter().map(|s| s["views"].as_u64().unwrap()).collect();
        assert!(views.windows(2).all(|w| w[0] >= w[1]), "most viewed first");
    }
}
