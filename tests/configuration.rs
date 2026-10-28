use macos_mcp_manager::{
    clients::{self, ClientKind, Source},
    manager::Manager,
    storage::{self, Change, MAX_CONFIG_SIZE, Pending, State, Store},
};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::Path,
};

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn manager(root: &Path, kind: ClientKind, contents: &str) -> Manager {
    let path = root.join(if kind == ClientKind::Codex {
        "config.toml"
    } else {
        "config.json"
    });
    write(&path, contents);
    Manager::open(vec![Source { kind, path }], root.join("state")).unwrap()
}

fn toggle(manager: &Manager, name: &str) {
    let snapshot = manager.snapshot().unwrap();
    let client = &snapshot.clients[0];
    let server = client
        .servers
        .iter()
        .find(|server| server.name == name)
        .unwrap();
    manager.toggle(client, server).unwrap();
}

#[test]
fn codex_preserves_comments_credentials_and_unrelated_settings() {
    let temp = tempfile::tempdir().unwrap();
    let before = "# my configuration\nmodel = 'example'\n\n[mcp_servers.github]\ncommand = 'npx' # keep this\nargs = ['server']\n\n[mcp_servers.github.env]\nTOKEN = 'private-test-token'\n\n[mcp_servers.other]\nurl = 'https://example.test/mcp'\nenabled = false # user comment\n";
    let manager = manager(temp.path(), ClientKind::Codex, before);
    toggle(&manager, "github");
    let after = fs::read_to_string(temp.path().join("config.toml")).unwrap();
    assert!(after.contains("# my configuration"));
    assert!(after.contains("command = 'npx' # keep this"));
    assert!(after.contains("TOKEN = 'private-test-token'"));
    assert!(after.contains("enabled = false # user comment"));
    assert!(
        !manager.snapshot().unwrap().clients[0]
            .servers
            .iter()
            .find(|s| s.name == "github")
            .unwrap()
            .enabled
    );
    toggle(&manager, "github");
    assert!(
        manager.snapshot().unwrap().clients[0]
            .servers
            .iter()
            .find(|s| s.name == "github")
            .unwrap()
            .enabled
    );
}

#[test]
fn codex_updates_inline_tables_and_existing_boolean_comments() {
    let temp = tempfile::tempdir().unwrap();
    let manager = manager(
        temp.path(),
        ClientKind::Codex,
        "mcp_servers = { a = { command = 'test', enabled = false } }\n",
    );
    toggle(&manager, "a");
    assert!(manager.snapshot().unwrap().clients[0].servers[0].enabled);
    assert!(
        fs::read_to_string(temp.path().join("config.toml"))
            .unwrap()
            .contains("command = 'test'")
    );
}

#[test]
fn codex_plugin_policy_does_not_toggle_the_whole_plugin() {
    let temp = tempfile::tempdir().unwrap();
    let before = "[plugins.'sample@test']\nenabled = true\n[plugins.'sample@test'.mcp_servers.docs]\nenabled = true # preserve\n";
    let manager = manager(temp.path(), ClientKind::Codex, before);
    toggle(&manager, "docs (sample@test)");
    let text = fs::read_to_string(temp.path().join("config.toml")).unwrap();
    assert!(text.contains("enabled = true\n"));
    assert!(text.contains("enabled = false # preserve"));
}

#[test]
fn disabled_parent_plugin_cannot_be_accidentally_enabled() {
    let temp = tempfile::tempdir().unwrap();
    let manager = manager(
        temp.path(),
        ClientKind::Codex,
        "[plugins.sample]\nenabled = false\n[plugins.sample.mcp_servers.docs]\n",
    );
    let view = &manager.snapshot().unwrap().clients[0];
    assert!(!view.servers[0].can_toggle);
    assert!(manager.toggle(view, &view.servers[0]).is_err());
}

#[test]
fn jsonc_parking_restores_nested_comments_and_secret_values() {
    let temp = tempfile::tempdir().unwrap();
    let before = "{\n // unrelated comment\n \"inputs\": [],\n \"servers\": {\n   \"a\": {\n     // server comment\n     \"command\": \"test\",\n     \"env\": {\"TOKEN\": \"secret-token\"},\n   },\n   \"b\": {\"url\": \"https://example.test\"},\n },\n}\n";
    let manager = manager(temp.path(), ClientKind::VsCode, before);
    toggle(&manager, "a");
    let disabled = fs::read_to_string(temp.path().join("config.json")).unwrap();
    assert!(!disabled.contains("secret-token"));
    assert!(disabled.contains("// unrelated comment"));
    assert!(disabled.contains("\"b\""));
    assert!(
        manager.snapshot().unwrap().clients[0]
            .servers
            .iter()
            .any(|s| s.name == "a" && !s.enabled)
    );
    toggle(&manager, "a");
    let restored = fs::read_to_string(temp.path().join("config.json")).unwrap();
    assert!(restored.contains("// server comment"));
    assert!(restored.contains("secret-token"));
    assert!(manager.store.load().unwrap().parked.is_empty());
    assert!(
        manager.snapshot().unwrap().clients[0]
            .servers
            .iter()
            .all(|s| s.enabled)
    );
}

#[test]
fn server_named_like_restore_marker_can_be_disabled_and_restored() {
    let name = "__mcp_manager_restore_0__";
    for kind in [
        ClientKind::ClaudeDesktop,
        ClientKind::ClaudeCode,
        ClientKind::VsCode,
        ClientKind::Cursor,
    ] {
        let temp = tempfile::tempdir().unwrap();
        let key = if kind == ClientKind::VsCode {
            "servers"
        } else {
            "mcpServers"
        };
        let original = serde_json::json!({
            (key): { (name): { "command": "test", "env": { "TOKEN": "preserve-me" } } },
            "unrelated": true
        });
        let manager = manager(temp.path(), kind, &original.to_string());
        toggle(&manager, name);
        let disabled = fs::read_to_string(temp.path().join("config.json")).unwrap();
        assert!(!disabled.contains(name));
        let state = manager.store.load().unwrap();
        assert!(
            !state
                .parked
                .values()
                .next()
                .unwrap()
                .definition
                .contains(name)
        );

        toggle(&manager, name);
        let restored = fs::read_to_string(temp.path().join("config.json")).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&restored).unwrap(),
            original
        );
        assert!(manager.store.load().unwrap().parked.is_empty());
        assert!(manager.snapshot().unwrap().clients[0].servers[0].enabled);
    }
}

#[test]
fn last_json_server_round_trips_without_losing_other_settings() {
    for kind in [
        ClientKind::ClaudeDesktop,
        ClientKind::ClaudeCode,
        ClientKind::Cursor,
    ] {
        let temp = tempfile::tempdir().unwrap();
        let before = "{\"mcpServers\":{\"a\":{\"command\":\"test\"}},\"theme\":\"dark\"}";
        let manager = manager(temp.path(), kind, before);
        toggle(&manager, "a");
        toggle(&manager, "a");
        let restored = fs::read_to_string(temp.path().join("config.json")).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&restored).unwrap(),
            serde_json::from_str::<serde_json::Value>(before).unwrap()
        );
        assert!(manager.store.load().unwrap().parked.is_empty());
        assert!(manager.snapshot().unwrap().clients[0].servers[0].enabled);
    }
}

#[test]
fn external_edits_are_never_overwritten_by_a_stale_menu() {
    let temp = tempfile::tempdir().unwrap();
    let before = "[mcp_servers.a]\ncommand = 'test'\n";
    let manager = manager(temp.path(), ClientKind::Codex, before);
    let old = manager.snapshot().unwrap().clients.remove(0);
    write(
        &temp.path().join("config.toml"),
        "# external\n[mcp_servers.a]\ncommand = 'new'\n",
    );
    assert!(manager.toggle(&old, &old.servers[0]).is_err());
    toggle(&manager, "a");
    let stale = manager.snapshot().unwrap().clients.remove(0);
    let external = "# further external edit\n[mcp_servers.a]\ncommand = 'newer'\n";
    write(&temp.path().join("config.toml"), external);
    assert!(manager.toggle(&stale, &stale.servers[0]).is_err());
    assert_eq!(
        fs::read_to_string(temp.path().join("config.toml")).unwrap(),
        external
    );
}

#[test]
fn conflicting_external_server_is_not_overwritten_on_restore() {
    let temp = tempfile::tempdir().unwrap();
    let manager = manager(
        temp.path(),
        ClientKind::ClaudeDesktop,
        "{\"mcpServers\":{\"a\":{\"command\":\"old\"}}}",
    );
    toggle(&manager, "a");
    let external = "{\"mcpServers\":{\"a\":{\"command\":\"new\"}}}";
    write(&temp.path().join("config.json"), external);
    assert!(manager.snapshot().unwrap().clients[0].error.is_some());
    assert!(!manager.store.load().unwrap().parked.is_empty());
    assert_eq!(
        fs::read_to_string(temp.path().join("config.json")).unwrap(),
        external
    );
}

#[test]
fn malformed_duplicate_and_wrong_shape_configs_are_read_only() {
    for text in [
        "{",
        "{\"mcpServers\": null}",
        "{\"mcpServers\": {\"a\":null}}",
        "{\"mcpServers\":{},\"mcpServers\":{}}",
        "{\"mcpServers\":{\"a\":{\"env\":{\"x\":1,\"x\":2}}}}",
        "{\"mcpServers\":{} \"x\":1}",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let manager = manager(temp.path(), ClientKind::ClaudeDesktop, text);
        let view = manager.snapshot().unwrap().clients.remove(0);
        assert!(view.error.is_some(), "accepted malformed fixture");
        assert_eq!(
            fs::read_to_string(temp.path().join("config.json")).unwrap(),
            text
        );
    }
}

#[test]
fn parsing_errors_do_not_reveal_secrets() {
    let temp = tempfile::tempdir().unwrap();
    let manager = manager(
        temp.path(),
        ClientKind::Codex,
        "private_token = 'sensitive\n",
    );
    let error = manager.snapshot().unwrap().clients[0]
        .error
        .clone()
        .unwrap();
    assert!(!error.contains("sensitive"));
    assert!(!error.contains("private_token"));
}

#[test]
fn list_output_does_not_contain_configuration_or_credentials() {
    let temp = tempfile::tempdir().unwrap();
    let manager = manager(
        temp.path(),
        ClientKind::ClaudeCode,
        "{\"mcpServers\":{\"a\":{\"env\":{\"TOKEN\":\"secret-value\"}}}}",
    );
    let json = serde_json::to_string(&manager.snapshot().unwrap()).unwrap();
    assert!(!json.contains("secret-value"));
    assert!(!json.contains("TOKEN"));
    assert!(!json.contains("contents"));
}

#[test]
fn configurations_are_independent_per_client() {
    let temp = tempfile::tempdir().unwrap();
    let one = temp.path().join("one.json");
    let two = temp.path().join("two.json");
    let before = "{\"mcpServers\":{\"same\":{\"command\":\"test\"}}}";
    write(&one, before);
    write(&two, before);
    let manager = Manager::open(
        vec![
            Source {
                kind: ClientKind::ClaudeDesktop,
                path: one,
            },
            Source {
                kind: ClientKind::ClaudeCode,
                path: two.clone(),
            },
        ],
        temp.path().join("state"),
    )
    .unwrap();
    toggle(&manager, "same");
    assert_eq!(fs::read_to_string(two).unwrap(), before);
    assert!(manager.snapshot().unwrap().clients[1].servers[0].enabled);
}

#[test]
fn state_files_are_private_and_source_permissions_are_preserved() {
    let temp = tempfile::tempdir().unwrap();
    let manager = manager(
        temp.path(),
        ClientKind::Codex,
        "[mcp_servers.a]\ncommand = 'test'\n",
    );
    let config = temp.path().join("config.toml");
    fs::set_permissions(&config, fs::Permissions::from_mode(0o640)).unwrap();
    toggle(&manager, "a");
    assert_eq!(
        fs::metadata(config).unwrap().permissions().mode() & 0o777,
        0o640
    );
    for path in ["state.json", "instance.lock"] {
        assert_eq!(
            fs::metadata(temp.path().join("state").join(path))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    assert_eq!(
        fs::metadata(temp.path().join("state"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
}

#[test]
fn duplicate_instances_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let _one = Store::open(temp.path().join("state")).unwrap();
    assert!(Store::open(temp.path().join("state")).is_err());
}

#[test]
fn symlinked_configuration_is_updated_without_replacing_the_link() {
    let temp = tempfile::tempdir().unwrap();
    let actual = temp.path().join("actual.toml");
    let linked = temp.path().join("config.toml");
    write(&actual, "[mcp_servers.a]\ncommand = 'test'\n");
    symlink(&actual, &linked).unwrap();
    let manager = Manager::open(
        vec![Source {
            kind: ClientKind::Codex,
            path: linked.clone(),
        }],
        temp.path().join("state"),
    )
    .unwrap();
    toggle(&manager, "a");
    assert!(
        fs::symlink_metadata(linked)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(
        fs::read_to_string(actual)
            .unwrap()
            .contains("enabled = false")
    );
}

#[test]
fn hard_links_are_not_silently_broken() {
    let temp = tempfile::tempdir().unwrap();
    let manager = manager(
        temp.path(),
        ClientKind::Codex,
        "[mcp_servers.a]\ncommand = 'test'\n",
    );
    fs::hard_link(
        temp.path().join("config.toml"),
        temp.path().join("other.toml"),
    )
    .unwrap();
    let view = manager.snapshot().unwrap().clients.remove(0);
    assert!(manager.toggle(&view, &view.servers[0]).is_err());
    assert!(manager.store.load().unwrap().pending.is_none());
}

#[test]
fn recovery_finishes_a_committed_write_or_cancels_an_unwritten_change() {
    for committed in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        let before = "[mcp_servers.a]\ncommand = 'test'\n";
        let after = "[mcp_servers.a]\ncommand = 'test'\nenabled = false\n";
        write(&path, if committed { after } else { before });
        let state_dir = temp.path().join("state");
        let store = Store::open(state_dir.clone()).unwrap();
        let change = Change {
            path: path.clone(),
            before: before.into(),
            after: after.into(),
            parked_before: Default::default(),
            parked_after: Default::default(),
        };
        store
            .save(&State {
                version: 1,
                pending: Some(Pending { change }),
                ..State::default()
            })
            .unwrap();
        drop(store);
        let manager = Manager::open(
            vec![Source {
                kind: ClientKind::Codex,
                path,
            }],
            state_dir,
        )
        .unwrap();
        let state = manager.store.load().unwrap();
        assert!(state.pending.is_none());
        assert_eq!(
            fs::read_to_string(&manager.snapshot().unwrap().clients[0].source.path).unwrap(),
            if committed { after } else { before }
        );
    }
}

#[test]
fn recovery_preserves_conflicting_external_edits_and_journal() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.toml");
    write(&path, "# external");
    let state_dir = temp.path().join("state");
    let store = Store::open(state_dir.clone()).unwrap();
    let change = Change {
        path: path.clone(),
        before: "before".into(),
        after: "after".into(),
        parked_before: Default::default(),
        parked_after: Default::default(),
    };
    store
        .save(&State {
            version: 1,
            pending: Some(Pending { change }),
            ..State::default()
        })
        .unwrap();
    drop(store);
    assert!(Manager::open(Vec::new(), state_dir.clone()).is_err());
    assert_eq!(fs::read_to_string(path).unwrap(), "# external");
    assert!(
        Store::open(state_dir)
            .unwrap()
            .load()
            .unwrap()
            .pending
            .is_some()
    );
}

#[test]
fn disabled_definitions_survive_application_restart() {
    let temp = tempfile::tempdir().unwrap();
    let manager = manager(
        temp.path(),
        ClientKind::ClaudeDesktop,
        "{\"mcpServers\":{\"a\":{\"command\":\"test\"}}}",
    );
    toggle(&manager, "a");
    drop(manager);
    let manager = Manager::open(
        vec![Source {
            kind: ClientKind::ClaudeDesktop,
            path: temp.path().join("config.json"),
        }],
        temp.path().join("state"),
    )
    .unwrap();
    assert!(!manager.snapshot().unwrap().clients[0].servers[0].enabled);
    toggle(&manager, "a");
    assert!(manager.snapshot().unwrap().clients[0].servers[0].enabled);
}

#[test]
fn fixture_home_paths_cover_all_initial_clients() {
    let sources = clients::sources(Path::new("/fixture"), None);
    assert_eq!(sources.len(), 5);
    assert!(sources.iter().all(|s| s.path.starts_with("/fixture")));
    assert!(
        sources
            .iter()
            .any(|s| s.path.ends_with("Code/User/mcp.json"))
    );
}

#[test]
fn whole_plugin_toggle_updates_parent_and_disables_child_actions() {
    let temp = tempfile::tempdir().unwrap();
    let before = "[plugins.'example@marketplace']\nenabled = true\n[plugins.'example@marketplace'.mcp_servers.docs]\nenabled = true\n";
    let manager = manager(temp.path(), ClientKind::Codex, before);
    toggle(&manager, "Plugin: example@marketplace (whole plugin)");
    let view = manager.snapshot().unwrap().clients.remove(0);
    let child = view
        .servers
        .iter()
        .find(|server| matches!(&server.target, clients::Target::Toml(path) if path.len() == 4))
        .unwrap();
    assert!(!child.enabled);
    assert!(!child.can_toggle);
    toggle(&manager, "Plugin: example@marketplace (whole plugin)");
    assert_eq!(
        fs::read_to_string(temp.path().join("config.toml")).unwrap(),
        before
    );
}

#[test]
fn recovery_keeps_parked_definitions_in_sync_with_committed_json() {
    let temp = tempfile::tempdir().unwrap();
    let first = manager(
        temp.path(),
        ClientKind::ClaudeDesktop,
        "{\"mcpServers\":{\"a\":{\"command\":\"test\"}}}",
    );
    let original = fs::read_to_string(temp.path().join("config.json")).unwrap();
    toggle(&first, "a");
    let mut state = first.store.load().unwrap();
    let change = Change {
        path: temp.path().join("config.json"),
        before: original,
        after: fs::read_to_string(temp.path().join("config.json")).unwrap(),
        parked_before: Default::default(),
        parked_after: state.parked.clone(),
    };
    state.parked = change.parked_before.clone();
    state.pending = Some(Pending { change });
    first.store.save(&state).unwrap();
    drop(first);
    let recovered = Manager::open(
        vec![Source {
            kind: ClientKind::ClaudeDesktop,
            path: temp.path().join("config.json"),
        }],
        temp.path().join("state"),
    )
    .unwrap();
    assert!(!recovered.snapshot().unwrap().clients[0].servers[0].enabled);
    assert_eq!(recovered.store.load().unwrap().parked.len(), 1);
    toggle(&recovered, "a");
    assert!(recovered.snapshot().unwrap().clients[0].servers[0].enabled);
}

#[test]
fn stale_menu_refuses_a_retargeted_config_symlink() {
    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("first.toml");
    let second = temp.path().join("second.toml");
    let link = temp.path().join("config.toml");
    let before = "[mcp_servers.a]\ncommand = 'test'\n";
    write(&first, before);
    write(&second, before);
    symlink(&first, &link).unwrap();
    let manager = Manager::open(
        vec![Source {
            kind: ClientKind::Codex,
            path: link.clone(),
        }],
        temp.path().join("state"),
    )
    .unwrap();
    let view = manager.snapshot().unwrap().clients.remove(0);
    fs::remove_file(&link).unwrap();
    symlink(&second, &link).unwrap();
    assert!(manager.toggle(&view, &view.servers[0]).is_err());
    assert_eq!(fs::read_to_string(first).unwrap(), before);
    assert_eq!(fs::read_to_string(second).unwrap(), before);
}

#[test]
fn older_undo_state_does_not_lose_disabled_definitions() {
    let temp = tempfile::tempdir().unwrap();
    let before = "{\"mcpServers\":{\"a\":{\"command\":\"test\"}}}";
    let first = manager(temp.path(), ClientKind::ClaudeDesktop, before);
    toggle(&first, "a");
    let state = first.store.load().unwrap();
    let path = fs::canonicalize(temp.path().join("config.json")).unwrap();
    let previous_change = Change {
        path: path.clone(),
        before: before.into(),
        after: fs::read_to_string(&path).unwrap(),
        parked_before: Default::default(),
        parked_after: state.parked.clone(),
    };
    let mut legacy = serde_json::to_value(&state).unwrap();
    legacy["undo"] = serde_json::to_value(&previous_change).unwrap();
    // An old pending journal also contains record_undo; it must still recover.
    legacy["pending"] = serde_json::json!({ "change": previous_change, "record_undo": true });
    fs::write(
        temp.path().join("state/state.json"),
        serde_json::to_vec(&legacy).unwrap(),
    )
    .unwrap();
    drop(first);
    let updated = Manager::open(
        vec![Source {
            kind: ClientKind::ClaudeDesktop,
            path,
        }],
        temp.path().join("state"),
    )
    .unwrap();
    assert!(!updated.snapshot().unwrap().clients[0].servers[0].enabled);
    toggle(&updated, "a");
    assert!(updated.snapshot().unwrap().clients[0].servers[0].enabled);
    assert!(updated.store.load().unwrap().parked.is_empty());
    let saved: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(temp.path().join("state/state.json")).unwrap())
            .unwrap();
    assert!(saved.get("undo").is_none());
    assert!(saved["pending"].is_null());
}

#[test]
fn replacement_size_limit_counts_bytes_and_accepts_the_exact_limit() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.json");
    write(&path, "{}");
    let boundary = format!("{{}}{}", " ".repeat(MAX_CONFIG_SIZE as usize - 2));
    storage::replace_config(&path, "{}", &boundary).unwrap();
    assert_eq!(storage::read_config(&path).unwrap(), boundary);

    // Character count is below the limit, but the UTF-8 byte count exceeds it.
    let oversized = "é".repeat(MAX_CONFIG_SIZE as usize / 2 + 1);
    assert!(oversized.chars().count() < MAX_CONFIG_SIZE as usize);
    let error = storage::replace_config(&path, &boundary, &oversized).unwrap_err();
    assert!(error.to_string().contains("8 MiB"));
    assert_eq!(storage::read_config(&path).unwrap(), boundary);
}

#[test]
fn oversized_toggle_keeps_configuration_readable_and_other_clients_usable() {
    let temp = tempfile::tempdir().unwrap();
    let codex = temp.path().join("config.toml");
    let claude = temp.path().join("config.json");
    let base = "[mcp_servers.a]\ncommand = 'test'\n#";
    let before = format!(
        "{base}{}",
        "x".repeat(MAX_CONFIG_SIZE as usize - base.len())
    );
    write(&codex, &before);
    write(&claude, "{\"mcpServers\":{\"b\":{\"command\":\"test\"}}}");
    let sources = vec![
        Source {
            kind: ClientKind::Codex,
            path: codex.clone(),
        },
        Source {
            kind: ClientKind::ClaudeDesktop,
            path: claude,
        },
    ];
    let state_dir = temp.path().join("state");
    let manager = Manager::open(sources.clone(), state_dir.clone()).unwrap();
    let view = manager.snapshot().unwrap().clients.remove(0);
    let error = manager.toggle(&view, &view.servers[0]).unwrap_err();
    assert!(error.to_string().contains("8 MiB"));
    assert_eq!(storage::read_config(&codex).unwrap(), before);
    assert!(manager.store.load().unwrap().pending.is_none());

    let other = manager.snapshot().unwrap().clients.remove(1);
    manager.toggle(&other, &other.servers[0]).unwrap();
    drop(manager);
    let reopened = Manager::open(sources, state_dir).unwrap();
    let snapshot = reopened.snapshot().unwrap();
    assert!(snapshot.clients.iter().all(|view| view.error.is_none()));
    assert!(snapshot.clients[0].servers[0].enabled);
    assert!(!snapshot.clients[1].servers[0].enabled);
}

#[test]
fn oversized_restore_keeps_disabled_definition_and_allows_retry_after_restart() {
    let temp = tempfile::tempdir().unwrap();
    let manager = manager(
        temp.path(),
        ClientKind::ClaudeDesktop,
        "{\"mcpServers\":{\"a\":{\"command\":\"test\"}}}",
    );
    toggle(&manager, "a");
    let parked = manager.store.load().unwrap().parked;
    let path = temp.path().join("config.json");
    let base = "{\"mcpServers\":{},\"padding\":\"\"}";
    let before = format!(
        "{{\"mcpServers\":{{}},\"padding\":\"{}\"}}",
        "x".repeat(MAX_CONFIG_SIZE as usize - base.len())
    );
    write(&path, &before);
    let view = manager.snapshot().unwrap().clients.remove(0);
    let error = manager.toggle(&view, &view.servers[0]).unwrap_err();
    assert!(error.to_string().contains("8 MiB"));
    assert_eq!(storage::read_config(&path).unwrap(), before);
    let state = manager.store.load().unwrap();
    assert!(state.pending.is_none());
    assert_eq!(state.parked, parked);
    drop(manager);

    let reopened = Manager::open(
        vec![Source {
            kind: ClientKind::ClaudeDesktop,
            path: path.clone(),
        }],
        temp.path().join("state"),
    )
    .unwrap();
    assert!(!reopened.snapshot().unwrap().clients[0].servers[0].enabled);
    write(&path, "{\"mcpServers\":{}}");
    toggle(&reopened, "a");
    assert!(reopened.snapshot().unwrap().clients[0].servers[0].enabled);
    assert!(reopened.store.load().unwrap().parked.is_empty());
}
