//! 契约一致性测试：确保前端调用与后端实现不会悄悄跑偏。
//!
//! 这个项目里最容易出现的低级事故是：
//!   * 后端加了命令但前端没接（或反之，前端调了一个不存在的命令 → 运行时才报错）
//!   * 事件名拼写不一致（`asr:partial` vs `asr:partials` → 前端永远收不到消息）
//!
//! 这类问题靠人眼 review 很难发现，所以这里直接从源码里抽取两边的事实来对比。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("找不到仓库根目录")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("读取 {} 失败：{e}", path.display()))
}

/// 从 `src/lib/api.ts` 里抽出所有被调用的后端命令名
fn frontend_commands() -> BTreeSet<String> {
    let src = read("src/lib/api.ts");
    let mut out = BTreeSet::new();
    let mut rest = src.as_str();
    // 形如：call<Settings>("get_settings", { ... })
    while let Some(idx) = rest.find("call<") {
        rest = &rest[idx + 5..];
        let Some(open) = rest.find('(') else { break };
        let after = &rest[open + 1..];
        let trimmed = after.trim_start();
        if let Some(stripped) = trimmed.strip_prefix('"') {
            if let Some(end) = stripped.find('"') {
                let name = &stripped[..end];
                if !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                {
                    out.insert(name.to_string());
                }
            }
        }
        rest = after;
    }
    out
}

/// 从 `src-tauri/src/lib.rs` 的 `generate_handler!` 里抽出注册的命令
fn backend_commands() -> BTreeSet<String> {
    let src = read("src-tauri/src/lib.rs");
    let start = src
        .find("generate_handler![")
        .expect("lib.rs 里找不到 generate_handler!");
    let rest = &src[start..];
    let end = rest.find(']').expect("generate_handler! 没有闭合");
    let block = &rest[..end];

    let mut out = BTreeSet::new();
    let mut cursor = block;
    while let Some(idx) = cursor.find("commands::") {
        cursor = &cursor[idx + "commands::".len()..];
        let name: String = cursor
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            out.insert(name);
        }
    }
    out
}

/// 从 `src/lib/contract.ts` 的 `EV` 常量里抽出事件名
fn frontend_events() -> BTreeSet<String> {
    let src = read("src/lib/contract.ts");
    let mut out = BTreeSet::new();
    for line in src.lines() {
        let line = line.trim();
        // 形如：segment: "asr:segment",
        if let Some(idx) = line.find(':') {
            let after = line[idx + 1..].trim_start();
            if let Some(stripped) = after.strip_prefix('"') {
                if let Some(end) = stripped.find('"') {
                    let value = &stripped[..end];
                    if value.contains(':') && !value.contains(' ') {
                        out.insert(value.to_string());
                    }
                }
            }
        }
    }
    out
}

/// 从 `src-tauri/src/events.rs` 的事件名常量里抽出值
fn backend_events() -> BTreeSet<String> {
    let src = read("src-tauri/src/events.rs");
    let mut out = BTreeSet::new();
    for line in src.lines() {
        let line = line.trim();
        if !line.starts_with("pub const ") {
            continue;
        }
        // pub const SEGMENT: &str = "asr:segment";
        if let Some(eq) = line.find('=') {
            let after = line[eq + 1..].trim_start();
            if let Some(stripped) = after.strip_prefix('"') {
                if let Some(end) = stripped.find('"') {
                    let value = &stripped[..end];
                    if value.contains(':') {
                        out.insert(value.to_string());
                    }
                }
            }
        }
    }
    out
}

#[test]
fn frontend_and_backend_expose_the_same_commands() {
    let fe = frontend_commands();
    let be = backend_commands();

    assert!(
        fe.len() >= 20,
        "只从 api.ts 抽到 {} 个命令，抽取逻辑可能失效了：{fe:?}",
        fe.len()
    );
    assert!(
        be.len() >= 20,
        "只从 lib.rs 抽到 {} 个命令，抽取逻辑可能失效了：{be:?}",
        be.len()
    );

    let missing_in_backend: Vec<_> = fe.difference(&be).collect();
    let missing_in_frontend: Vec<_> = be.difference(&fe).collect();

    assert!(
        missing_in_backend.is_empty(),
        "前端调用了后端没有注册的命令：{missing_in_backend:?}"
    );
    assert!(
        missing_in_frontend.is_empty(),
        "后端注册了前端没用的命令（要么补前端封装，要么删掉）：{missing_in_frontend:?}"
    );
}

#[test]
fn frontend_and_backend_use_the_same_event_names() {
    let fe = frontend_events();
    let be = backend_events();

    assert!(fe.len() >= 8, "只抽到 {} 个前端事件：{fe:?}", fe.len());
    assert!(be.len() >= 8, "只抽到 {} 个后端事件：{be:?}", be.len());

    let missing_in_backend: Vec<_> = fe.difference(&be).collect();
    let missing_in_frontend: Vec<_> = be.difference(&fe).collect();

    assert!(
        missing_in_backend.is_empty(),
        "前端监听了后端不会发出的事件：{missing_in_backend:?}"
    );
    assert!(
        missing_in_frontend.is_empty(),
        "后端发出了前端没有监听的事件：{missing_in_frontend:?}"
    );
}

/// 前端发出的参数名必须是 camelCase（Tauri 默认把 Rust 的 snake_case 转成 camelCase）
#[test]
fn frontend_sends_camel_case_arguments() {
    let src = read("src/lib/api.ts");
    // 这些多词参数名如果写成 snake_case 就会在运行时静默失败
    for bad in [
        "session_id",
        "model_id",
        "mic_device_id",
        "loopback_device_id",
        "enable_mic",
        "provider_id",
        "active_provider_id",
    ] {
        assert!(
            !src.contains(&format!("{{ {bad}")),
            "api.ts 里出现了 snake_case 参数 `{bad}`，Tauri 期望 camelCase"
        );
    }
    // 正向确认当前唯一的多词参数确实用 camelCase 传
    assert!(src.contains("sessionId"), "api.ts 应该用 sessionId 传参");
    assert!(
        !src.contains("session_id"),
        "api.ts 不应出现 session_id"
    );
}

/// 冻结文件（契约与 mock）必须存在且非空
#[test]
fn contract_files_exist() {
    for rel in ["src/lib/contract.ts", "src/lib/api.ts", "src/lib/mock.ts"] {
        let text = read(rel);
        assert!(text.len() > 500, "{rel} 内容过少，可能被误删");
    }
}
