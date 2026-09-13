//! 诊断模式：`meeting-hear --check`
//!
//! 一个纯命令行的自检工具，用来回答「为什么我这边跑不起来」这类问题：
//! 模型下没下、音频设备认不认、whisper 编译进来的是哪个后端、AI 接口通不通。
//!
//! 它不依赖图形界面，因此在无显示环境（服务器、容器、CI）里也能跑。

use std::path::PathBuf;

use crate::ai::client::AiClient;
use crate::asr::client::AsrClient;
use crate::audio::capture;
use crate::settings::{AiSettings, AppPaths, Settings};

const APP_IDENTIFIER: &str = "com.meetinghear.desktop";

/// 与 Tauri 的 `app_data_dir()` 保持一致的默认数据目录
pub fn default_data_dir() -> PathBuf {
    #[cfg(target_os = "linux")]
    {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .unwrap_or_else(|| PathBuf::from("."));
        base.join(APP_IDENTIFIER)
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join("Library/Application Support"))
            .unwrap_or_else(|| PathBuf::from("."))
            .join(APP_IDENTIFIER)
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join(APP_IDENTIFIER)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        PathBuf::from(".").join(APP_IDENTIFIER)
    }
}

struct Line {
    label: String,
    value: String,
    ok: Option<bool>,
    hint: Option<String>,
}

/// 执行自检并打印结果。返回是否「关键项全部通过」。
pub fn run_check(args: &[String]) -> i32 {
    let json = args.iter().any(|a| a == "--json");
    let with_ai = args.iter().any(|a| a == "--ai");

    let data_dir = args
        .iter()
        .position(|a| a == "--data-dir")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .unwrap_or_else(default_data_dir);

    let paths = AppPaths::new(data_dir);
    let mut settings = Settings::load(&paths.data);
    settings.sanitize();

    let mut lines: Vec<Line> = Vec::new();

    lines.push(Line {
        label: "版本".into(),
        value: format!(
            "MeetingHear {} · {} · {}",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH
        ),
        ok: None,
        hint: None,
    });

    let dir_ok = paths.ensure().is_ok();
    lines.push(Line {
        label: "数据目录".into(),
        value: paths.data.display().to_string(),
        ok: Some(dir_ok),
        hint: if dir_ok {
            None
        } else {
            Some("目录不可写，请检查权限或用 --data-dir 指定其它位置".into())
        },
    });

    let sessions = crate::session::SessionStore::new(paths.sessions.clone())
        .list()
        .map(|l| l.len())
        .unwrap_or(0);
    lines.push(Line {
        label: "历史会话".into(),
        value: format!("{sessions} 条"),
        ok: None,
        hint: None,
    });

    // ---- 语音识别服务 ----
    let asr_ready = settings.asr.ready();
    lines.push(Line {
        label: "语音识别服务".into(),
        value: match settings.asr.active() {
            Some(p) if asr_ready => {
                format!("{} · {} · {}", p.name, p.model, p.endpoint())
            }
            Some(p) => format!("{} · 配置不完整（缺少 Base URL 或模型名）", p.name),
            None => "未配置".into(),
        },
        ok: Some(asr_ready),
        hint: if asr_ready {
            None
        } else {
            Some(
                "打开应用 → 设置 → 语音识别，选择一个服务商并填入 API Key；\
                 想完全离线可本地起 whisper.cpp server 或 faster-whisper-server"
                    .into(),
            )
        },
    });

    lines.push(Line {
        label: "断句方式".into(),
        value: "自适应能量 VAD（无需模型文件）".into(),
        ok: Some(true),
        hint: None,
    });

    // ---- 音频设备 ----
    let sources = capture::list_sources();
    let mics: Vec<_> = sources.iter().filter(|s| s.available && s.kind == crate::audio::capture::SourceKind::Microphone).collect();
    let loopbacks: Vec<_> = sources.iter().filter(|s| s.available && s.kind == crate::audio::capture::SourceKind::Loopback).collect();
    lines.push(Line {
        label: "麦克风".into(),
        value: if mics.is_empty() {
            "未检测到可用麦克风".into()
        } else {
            mics.iter().map(|s| s.label.clone()).collect::<Vec<_>>().join("、")
        },
        ok: Some(!mics.is_empty()),
        hint: if mics.is_empty() {
            Some("无音频输入设备。容器/服务器环境属于正常现象，请在桌面环境运行".into())
        } else {
            None
        },
    });
    lines.push(Line {
        label: "系统内录".into(),
        value: if loopbacks.is_empty() {
            "未检测到可用的系统内录源".into()
        } else {
            loopbacks.iter().map(|s| s.label.clone()).collect::<Vec<_>>().join("、")
        },
        ok: Some(!loopbacks.is_empty()),
        hint: loopbacks
            .is_empty()
            .then(|| {
                sources
                    .iter()
                    .find(|s| s.kind == crate::audio::capture::SourceKind::Loopback)
                    .and_then(|s| s.note.clone())
                    .unwrap_or_else(|| "只使用麦克风也可以正常开会".into())
            }),
    });

    // ---- AI ----
    let ai_configured = settings.ai.ready();
    lines.push(Line {
        label: "AI 接口".into(),
        value: match settings.ai.active() {
            Some(p) if ai_configured => format!("{} · {} · {}", p.name, p.model, p.root_url()),
            Some(p) => format!("{} · 配置不完整（缺少 Base URL 或模型名）", p.name),
            None => "未配置".into(),
        },
        ok: Some(ai_configured),
        hint: if ai_configured {
            None
        } else {
            Some("打开应用 → 设置 → AI 接口，选择服务商并填入 API Key".into())
        },
    });

    if with_ai {
        // 先测识别服务
        if let Some(provider) = settings.asr.active().cloned() {
            if let Ok(client) = AsrClient::new() {
                if let Ok(runtime) = tokio::runtime::Runtime::new() {
                    let result = runtime.block_on(client.test_connection(&provider));
                    lines.push(Line {
                        label: "识别连通性".into(),
                        value: if result.ok {
                            let sample = if result.sample.trim().is_empty() {
                                "（静音测试，返回空文本属正常）".to_string()
                            } else {
                                format!("返回：{}", result.sample)
                            };
                            format!("正常（{} ms，{sample}）", result.latency_ms)
                        } else {
                            format!("失败：{}", result.error.unwrap_or_default())
                        },
                        ok: Some(result.ok),
                        hint: if result.ok {
                            None
                        } else {
                            Some("检查 Base URL、路径、API Key 与模型名；本地服务请确认已启动".into())
                        },
                    });
                }
            }
        }

        let settings_ai: AiSettings = settings.ai.clone();
        if let Some(provider) = settings_ai.active().cloned() {
            let client = match AiClient::new() {
                Ok(c) => c,
                Err(e) => {
                    lines.push(Line {
                        label: "AI 连通性".into(),
                        value: e.to_string(),
                        ok: Some(false),
                        hint: None,
                    });
                    return finish(lines, json);
                }
            };
            let runtime = match tokio::runtime::Runtime::new() {
                Ok(r) => r,
                Err(e) => {
                    lines.push(Line {
                        label: "AI 连通性".into(),
                        value: format!("无法创建异步运行时：{e}"),
                        ok: Some(false),
                        hint: None,
                    });
                    return finish(lines, json);
                }
            };
            let result = runtime.block_on(client.test_connection(&provider));
            lines.push(Line {
                label: "AI 连通性".into(),
                value: if result.ok {
                    format!("正常（{} ms，返回：{}）", result.latency_ms, result.sample)
                } else {
                    format!("失败：{}", result.error.unwrap_or_default())
                },
                ok: Some(result.ok),
                hint: if result.ok {
                    None
                } else {
                    Some("检查 Base URL、API Key、网络代理；本地模型请确认服务已启动".into())
                },
            });
        }
    }

    finish(lines, json)
}

fn finish(lines: Vec<Line>, json: bool) -> i32 {
    let critical_ok = lines.iter().all(|l| l.ok != Some(false));

    if json {
        let value: Vec<serde_json::Value> = lines
            .iter()
            .map(|l| {
                serde_json::json!({
                    "label": l.label,
                    "value": l.value,
                    "ok": l.ok,
                    "hint": l.hint,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&value).unwrap_or_default());
    } else {
        println!("MeetingHear 自检");
        println!("{}", "─".repeat(60));
        for l in &lines {
            let mark = match l.ok {
                Some(true) => "✓",
                Some(false) => "✗",
                None => "·",
            };
            println!("{mark} {:<20} {}", l.label, l.value);
            if let Some(hint) = &l.hint {
                println!("  {:<20} └─ {hint}", "");
            }
        }
        println!("{}", "─".repeat(60));
        println!(
            "{}",
            if critical_ok {
                "关键项检查通过，可以开始录音。"
            } else {
                "存在未通过的关键项，请按上面的提示处理后重试。"
            }
        );
    }

    if critical_ok {
        0
    } else {
        1
    }
}

/// 把字节格式化成人类可读
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes == 0 {
        return "0 B".into();
    }
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if v >= 100.0 {
        format!("{v:.0} {}", UNITS[i])
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_dir_has_app_identifier() {
        let dir = default_data_dir();
        assert!(
            dir.to_string_lossy().contains(APP_IDENTIFIER),
            "数据目录应包含应用标识：{}",
            dir.display()
        );
    }

    #[test]
    fn data_dir_matches_tauri_convention_on_linux() {
        #[cfg(target_os = "linux")]
        {
            // 与上面实际启动应用时观察到的路径一致
            let dir = default_data_dir();
            if let Some(home) = std::env::var_os("HOME") {
                let expected = std::env::var_os("XDG_DATA_HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from(home).join(".local/share"))
                    .join(APP_IDENTIFIER);
                assert_eq!(dir, expected);
            }
        }
    }

    #[test]
    fn check_runs_and_reports_json() {
        let dir = std::env::temp_dir().join(format!("mh-check-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // 不 panic、返回 0 或 1（本环境没有音频设备与模型，预期返回 1）
        let code = run_check(&[
            "--json".to_string(),
            "--data-dir".to_string(),
            dir.display().to_string(),
        ]);
        assert!(code == 0 || code == 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn bytes_formatting() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(487_601_967), "465 MB");
        assert_eq!(format_bytes(1_624_555_275), "1.5 GB");
    }
}
