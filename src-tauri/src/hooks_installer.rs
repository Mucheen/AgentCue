use crate::models::{AgentProvider, HookProviderStatus};
use chrono::Utc;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

pub const HOOK_MARKER: &str = "agent-ing-hook";

pub fn resolve_hook_binary_path(home: &Path) -> PathBuf {
    let bin_name = if cfg!(windows) {
        "agent-ing-hook.exe"
    } else {
        "agent-ing-hook"
    };

    let mut source_candidate: Option<PathBuf> = None;

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join(bin_name);
            if sibling.exists() {
                source_candidate = Some(sibling);
            } else {
                // Check macOS .app Contents/Resources/resources/ or Windows resources/
                let res_macos = dir
                    .parent()
                    .map(|contents| contents.join("Resources").join("resources").join(bin_name));
                let res_win = Some(dir.join("resources").join(bin_name));
                for cand in [res_macos, res_win].into_iter().flatten() {
                    if cand.exists() {
                        source_candidate = Some(cand);
                        break;
                    }
                }
            }
        }
    }

    // Stage into stable canonical path ~/.agent-ing/bin/agent-ing-hook so moving the .app never breaks hooks
    if let Some(src) = source_candidate {
        let bin_dir = home.join(".agent-ing").join("bin");
        if std::fs::create_dir_all(&bin_dir).is_ok() {
            let staged = bin_dir.join(bin_name);
            if std::fs::copy(&src, &staged).is_ok() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755));
                }
                return staged;
            }
        }
        return src;
    }

    PathBuf::from(bin_name)
}

pub fn config_path_for(home: &Path, provider: AgentProvider) -> PathBuf {
    match provider {
        AgentProvider::ClaudeCode => home.join(".claude").join("settings.json"),
        AgentProvider::Codex => home.join(".codex").join("hooks.json"),
        AgentProvider::Antigravity => home.join(".gemini").join("config").join("hooks.json"),
    }
}

fn backup_dir(home: &Path) -> PathBuf {
    home.join(".agent-ing").join("backups")
}

fn backup_prefix_for(provider: AgentProvider) -> &'static str {
    match provider {
        AgentProvider::ClaudeCode => "claude-settings",
        AgentProvider::Codex => "codex-hooks",
        AgentProvider::Antigravity => "antigravity-hooks",
    }
}

fn has_backup(home: &Path, provider: AgentProvider) -> bool {
    let dir = backup_dir(home);
    let prefix = backup_prefix_for(provider);
    std::fs::read_dir(dir)
        .ok()
        .map(|entries| {
            entries
                .flatten()
                .any(|e| e.file_name().to_string_lossy().starts_with(prefix))
        })
        .unwrap_or(false)
}

fn write_backup_if_exists(home: &Path, provider: AgentProvider, target: &Path) -> Result<(), String> {
    if !target.exists() {
        return Ok(());
    }
    let bdir = backup_dir(home);
    std::fs::create_dir_all(&bdir).map_err(|e| format!("Failed creating backup dir: {e}"))?;
    let stamp = Utc::now().format("%Y%m%d-%H%M%S");
    let dest = bdir.join(format!("{}-{}.json.bak", backup_prefix_for(provider), stamp));
    std::fs::copy(target, dest).map_err(|e| format!("Failed writing config backup: {e}"))?;
    Ok(())
}

fn build_hook_command(hook_bin: &Path, cli_agent: &str, event: &str) -> String {
    format!("\"{}\" --agent {} --event {}", hook_bin.display(), cli_agent, event)
}

fn value_contains_marker(v: &Value) -> bool {
    match v {
        Value::String(s) => s.contains(HOOK_MARKER),
        Value::Array(arr) => arr.iter().any(value_contains_marker),
        Value::Object(map) => map.values().any(value_contains_marker),
        _ => false,
    }
}

pub fn get_all_hooks_status(home: &Path) -> Vec<HookProviderStatus> {
    [
        AgentProvider::ClaudeCode,
        AgentProvider::Codex,
        AgentProvider::Antigravity,
    ]
    .into_iter()
    .map(|p| get_provider_status(home, p))
    .collect()
}

pub fn get_provider_status(home: &Path, provider: AgentProvider) -> HookProviderStatus {
    let path = config_path_for(home, provider);
    let config_exists = path.exists();
    let installed = if config_exists {
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .map(|v| is_installed_in_json(provider, &v))
            .unwrap_or(false)
    } else {
        false
    };

    let note = match provider {
        AgentProvider::Codex => Some(
            "首次接入后在 Codex 终端输入 /hooks 信任一次钩子即可生效；未信任前自动由 JSONL 日志兜底。"
                .to_string(),
        ),
        AgentProvider::Antigravity => Some(
            "仅挂载状态与完成事件（不拦截 PreToolUse），不影响原有权限审批缓存。".to_string(),
        ),
        AgentProvider::ClaudeCode => {
            Some("保留已有第三方钩子（如 orca / herdr），支持实时捕获权限等待弹窗。".to_string())
        }
    };

    HookProviderStatus {
        provider,
        installed,
        config_path: path.display().to_string(),
        config_exists,
        backup_exists: has_backup(home, provider),
        note,
    }
}

pub fn is_installed_in_json(provider: AgentProvider, root: &Value) -> bool {
    match provider {
        AgentProvider::Antigravity => root
            .get("agent-ing")
            .map(value_contains_marker)
            .unwrap_or(false),
        AgentProvider::ClaudeCode | AgentProvider::Codex => root
            .get("hooks")
            .map(value_contains_marker)
            .unwrap_or(false),
    }
}

pub fn merge_install_json(provider: AgentProvider, mut root: Value, hook_bin: &Path) -> Value {
    if !root.is_object() {
        root = json!({});
    }
    // First strip any stale agent-ing entries for clean idempotence
    root = merge_uninstall_json(provider, root);

    match provider {
        AgentProvider::ClaudeCode => {
            let events = [
                "SessionStart",
                "UserPromptSubmit",
                "PreToolUse",
                "PostToolUse",
                "PostToolUseFailure",
                "PermissionRequest",
                "Notification",
                "Stop",
                "SessionEnd",
            ];
            let root_obj = root.as_object_mut().unwrap();
            let hooks_val = root_obj
                .entry("hooks")
                .or_insert_with(|| Value::Object(Map::new()));
            if !hooks_val.is_object() {
                *hooks_val = Value::Object(Map::new());
            }
            let hooks_obj = hooks_val.as_object_mut().unwrap();
            for ev in events {
                let arr_val = hooks_obj
                    .entry(ev.to_string())
                    .or_insert_with(|| Value::Array(vec![]));
                if !arr_val.is_array() {
                    *arr_val = Value::Array(vec![]);
                }
                let cmd = build_hook_command(hook_bin, "claude", ev);
                arr_val.as_array_mut().unwrap().push(json!({
                    "hooks": [
                        {
                            "type": "command",
                            "command": cmd,
                            "timeout": 5
                        }
                    ]
                }));
            }
        }
        AgentProvider::Codex => {
            let events = [
                "SessionStart",
                "UserPromptSubmit",
                "PreToolUse",
                "PermissionRequest",
                "PostToolUse",
                "Stop",
                "Interrupt",
            ];
            let root_obj = root.as_object_mut().unwrap();
            let hooks_val = root_obj
                .entry("hooks")
                .or_insert_with(|| Value::Object(Map::new()));
            if !hooks_val.is_object() {
                *hooks_val = Value::Object(Map::new());
            }
            let hooks_obj = hooks_val.as_object_mut().unwrap();
            for ev in events {
                let arr_val = hooks_obj
                    .entry(ev.to_string())
                    .or_insert_with(|| Value::Array(vec![]));
                if !arr_val.is_array() {
                    *arr_val = Value::Array(vec![]);
                }
                let cmd = build_hook_command(hook_bin, "codex", ev);
                arr_val.as_array_mut().unwrap().push(json!({
                    "hooks": [
                        {
                            "type": "command",
                            "command": cmd,
                            "timeout": 5
                        }
                    ]
                }));
            }
        }
        AgentProvider::Antigravity => {
            let root_obj = root.as_object_mut().unwrap();
            root_obj.insert(
                "agent-ing".to_string(),
                json!({
                    "enabled": true,
                    "SessionStart": [
                        {
                            "type": "command",
                            "command": build_hook_command(hook_bin, "antigravity", "SessionStart"),
                            "timeout": 5
                        }
                    ],
                    "PreInvocation": [
                        {
                            "type": "command",
                            "command": build_hook_command(hook_bin, "antigravity", "PreInvocation"),
                            "timeout": 5
                        }
                    ],
                    "PostToolUse": [
                        {
                            "matcher": "*",
                            "hooks": [
                                {
                                    "type": "command",
                                    "command": build_hook_command(hook_bin, "antigravity", "PostToolUse"),
                                    "timeout": 5
                                }
                            ]
                        }
                    ],
                    "Stop": [
                        {
                            "type": "command",
                            "command": build_hook_command(hook_bin, "antigravity", "Stop"),
                            "timeout": 5
                        }
                    ]
                }),
            );
        }
    }
    root
}

pub fn merge_uninstall_json(provider: AgentProvider, mut root: Value) -> Value {
    let Some(root_obj) = root.as_object_mut() else {
        return root;
    };

    match provider {
        AgentProvider::Antigravity => {
            root_obj.remove("agent-ing");
        }
        AgentProvider::ClaudeCode | AgentProvider::Codex => {
            if let Some(hooks_obj) = root_obj.get_mut("hooks").and_then(|v| v.as_object_mut()) {
                for (_event_name, groups_val) in hooks_obj.iter_mut() {
                    if let Some(groups) = groups_val.as_array_mut() {
                        groups.retain(|group| {
                            if let Some(inner) =
                                group.get("hooks").and_then(|h| h.as_array())
                            {
                                // Keep any group that does NOT solely consist of agent-ing-hook
                                !inner.iter().any(value_contains_marker)
                            } else {
                                !value_contains_marker(group)
                            }
                        });
                    }
                }
            }
        }
    }
    root
}

pub fn install_provider_hooks(
    home: &Path,
    provider: AgentProvider,
    hook_bin: &Path,
) -> Result<HookProviderStatus, String> {
    let target = config_path_for(home, provider);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Failed creating dir: {e}"))?;
    }
    write_backup_if_exists(home, provider, &target)?;

    let existing: Value = if target.exists() {
        let raw = std::fs::read_to_string(&target)
            .map_err(|e| format!("Failed reading {}: {e}", target.display()))?;
        if raw.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(&raw)
                .map_err(|e| format!("Invalid JSON in {}: {e}", target.display()))?
        }
    } else {
        json!({})
    };

    let updated = merge_install_json(provider, existing, hook_bin);
    let serialized = serde_json::to_string_pretty(&updated)
        .map_err(|e| format!("Failed serializing JSON: {e}"))?;
    std::fs::write(&target, format!("{serialized}\n"))
        .map_err(|e| format!("Failed writing {}: {e}", target.display()))?;

    Ok(get_provider_status(home, provider))
}

pub fn uninstall_provider_hooks(
    home: &Path,
    provider: AgentProvider,
) -> Result<HookProviderStatus, String> {
    let target = config_path_for(home, provider);
    if !target.exists() {
        return Ok(get_provider_status(home, provider));
    }
    write_backup_if_exists(home, provider, &target)?;

    let raw = std::fs::read_to_string(&target)
        .map_err(|e| format!("Failed reading {}: {e}", target.display()))?;
    if !raw.trim().is_empty() {
        let existing: Value = serde_json::from_str(&raw)
            .map_err(|e| format!("Invalid JSON in {}: {e}", target.display()))?;
        let updated = merge_uninstall_json(provider, existing);
        let serialized = serde_json::to_string_pretty(&updated)
            .map_err(|e| format!("Failed serializing JSON: {e}"))?;
        std::fs::write(&target, format!("{serialized}\n"))
            .map_err(|e| format!("Failed writing {}: {e}", target.display()))?;
    }

    Ok(get_provider_status(home, provider))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn preserves_third_party_claude_and_codex_hooks() {
        let dir = tempdir().unwrap();
        let home = dir.path();
        let claude_path = config_path_for(home, AgentProvider::ClaudeCode);
        std::fs::create_dir_all(claude_path.parent().unwrap()).unwrap();

        let original = json!({
            "model": "claude-sonnet-4-5",
            "theme": "dark",
            "hooks": {
                "SessionStart": [
                    {
                        "matcher": "^(startup|resume)$",
                        "hooks": [
                            {
                                "type": "command",
                                "command": "bash '/Users/test/.claude/hooks/herdr-agent-state.sh' session",
                                "timeout": 10
                            }
                        ]
                    },
                    {
                        "hooks": [
                            {
                                "type": "command",
                                "command": "/bin/sh ~/.orca/agent-hooks/claude-hook.sh",
                                "timeout": 10
                            }
                        ]
                    }
                ]
            }
        });
        std::fs::write(&claude_path, serde_json::to_string_pretty(&original).unwrap()).unwrap();

        let bin = PathBuf::from("/Applications/agent-ing.app/Contents/MacOS/agent-ing-hook");
        let st = install_provider_hooks(home, AgentProvider::ClaudeCode, &bin).unwrap();
        assert!(st.installed);
        assert!(st.backup_exists);

        // Idempotent second install should not duplicate entries
        let st2 = install_provider_hooks(home, AgentProvider::ClaudeCode, &bin).unwrap();
        assert!(st2.installed);

        let after_install: Value =
            serde_json::from_str(&std::fs::read_to_string(&claude_path).unwrap()).unwrap();
        let session_start = after_install["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(session_start.len(), 3);

        // Uninstall restores exact third-party entries and top-level settings
        let st_un = uninstall_provider_hooks(home, AgentProvider::ClaudeCode).unwrap();
        assert!(!st_un.installed);

        let after_uninstall: Value =
            serde_json::from_str(&std::fs::read_to_string(&claude_path).unwrap()).unwrap();
        assert_eq!(after_uninstall["model"], "claude-sonnet-4-5");
        assert_eq!(after_uninstall["hooks"]["SessionStart"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn antigravity_hooks_install_and_remove_cleanly() {
        let dir = tempdir().unwrap();
        let home = dir.path();
        let agy_path = config_path_for(home, AgentProvider::Antigravity);
        std::fs::create_dir_all(agy_path.parent().unwrap()).unwrap();
        std::fs::write(
            &agy_path,
            r#"{"custom-lint":{"PostToolUse":[{"matcher":"edit_file","hooks":[{"command":"./lint.sh"}]}]}}"#,
        )
        .unwrap();

        let bin = PathBuf::from("/usr/local/bin/agent-ing-hook");
        let st = install_provider_hooks(home, AgentProvider::Antigravity, &bin).unwrap();
        assert!(st.installed);

        let v: Value = serde_json::from_str(&std::fs::read_to_string(&agy_path).unwrap()).unwrap();
        assert!(v.get("custom-lint").is_some());
        assert!(v.get("agent-ing").is_some());
        assert!(v["agent-ing"].get("PreToolUse").is_none());

        let st_un = uninstall_provider_hooks(home, AgentProvider::Antigravity).unwrap();
        assert!(!st_un.installed);
        let v_after: Value =
            serde_json::from_str(&std::fs::read_to_string(&agy_path).unwrap()).unwrap();
        assert!(v_after.get("custom-lint").is_some());
        assert!(v_after.get("agent-ing").is_none());
    }
}
