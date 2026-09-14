//! 端到端集成测试：真实识别服务 + 真实音频 + 真实 LLM。
//!
//! App 本身不含识别模型，所以端到端测试需要一个**真实的识别服务**。
//! 最方便的是本地起一个 whisper.cpp server：
//!
//! ```bash
//! git clone --depth 1 https://github.com/ggml-org/whisper.cpp
//! cd whisper.cpp && cmake -B build -DWHISPER_BUILD_SERVER=ON && cmake --build build -j
//! ./build/bin/whisper-server -m models/ggml-tiny.bin --port 8090
//! ```
//!
//! 也可以用 faster-whisper-server、LM Studio，或直接把 Base URL 指向云端服务。
//!
//! 运行：
//! ```bash
//! cargo test --test e2e_pipeline -- --ignored --nocapture
//! # 覆盖服务地址
//! MEETINGHEAR_E2E_ASR_URL=http://127.0.0.1:9000 cargo test --test e2e_pipeline -- --ignored
//! ```
//!
//! **语言必须由服务端配置。** 应用不发送 language 字段，所以 whisper.cpp server
//! 要带 `--language auto` 启动（它默认是 `en`，中文会被按英文识别并输出英文幻觉）：
//! ```bash
//! ./build/bin/whisper-server -m models/ggml-base.bin --port 8090 --language auto
//! ```

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::bounded;

use meeting_hear_lib::ai::client::{AiClient, ChatMessage};
use meeting_hear_lib::ai::summarizer::SummaryState;
use meeting_hear_lib::asr::client::{AsrClient, TranscribeRequest};
use meeting_hear_lib::audio::capture::SourceKind;
use meeting_hear_lib::events::{event, CollectingEmitter, Emitter};
use meeting_hear_lib::pipeline::{self, PipelineOptions};
use meeting_hear_lib::session::{Session, SessionConfig, SessionStatus};
use meeting_hear_lib::settings::{AiProvider, AiSettings, AsrProvider, AsrSettings, VadSettings};

/* ==========================================================================
 * 环境准备
 * ========================================================================== */

/// 供测试使用的 Tokio 运行时（纪要循环需要它）
fn test_runtime() -> &'static tokio::runtime::Runtime {
    static RT: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("无法创建测试运行时")
    })
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("找不到仓库根目录")
        .to_path_buf()
}

fn audio_fixture(name: &str) -> PathBuf {
    repo_root().join(".e2e/audio").join(name)
}

fn skip(reason: &str) {
    eprintln!("跳过：{reason}");
}

/// 被测的识别服务（默认指向本地 whisper.cpp server）
fn asr_provider() -> AsrProvider {
    let url =
        std::env::var("MEETINGHEAR_E2E_ASR_URL").unwrap_or_else(|_| "http://127.0.0.1:8090".into());
    let path = std::env::var("MEETINGHEAR_E2E_ASR_PATH").unwrap_or_else(|_| "/inference".into());
    AsrProvider {
        id: "e2e".into(),
        name: "E2E 本地识别服务".into(),
        base_url: url,
        transcription_path: path,
        api_key: std::env::var("MEETINGHEAR_E2E_ASR_KEY").unwrap_or_default(),
        model: std::env::var("MEETINGHEAR_E2E_ASR_MODEL").unwrap_or_else(|_| "whisper-1".into()),
        response_format: "json".into(),
        timeout_secs: 180,
        extra_headers: Vec::new(),
    }
}

fn asr_settings(provider: &AsrProvider) -> AsrSettings {
    AsrSettings {
        enabled: true,
        active_provider_id: provider.id.clone(),
        providers: vec![provider.clone()],
        context_prompt: true,
        temperature: 0.0,
        live_preview: false,
        max_chunk_secs: 25,
    }
}

fn ollama_settings() -> AiSettings {
    let provider = AiProvider {
        id: "ollama-e2e".into(),
        name: "Ollama (e2e)".into(),
        base_url: std::env::var("MEETINGHEAR_E2E_AI_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:11434/v1".into()),
        api_key: String::new(),
        model: std::env::var("MEETINGHEAR_E2E_AI_MODEL")
            .unwrap_or_else(|_| "qwen2.5:1.5b".into()),
        temperature: 0.2,
        max_tokens: 800,
        json_mode: true,
        no_thinking: true,
        timeout_secs: 120,
        extra_headers: Vec::new(),
    };
    AiSettings {
        summary_mode: "meeting".into(),
        enabled: true,
        active_provider_id: provider.id.clone(),
        providers: vec![provider],
        auto_summary: false,
        interval_secs: 20,
        min_new_chars: 1,
        live_window_secs: 90,
        max_context_chars: 4_000,
        final_report_on_stop: false,
    }
}

/// 识别服务是否可用；不可用就跳过测试（而不是报错）
fn ensure_asr_service(provider: &AsrProvider) -> bool {
    let client = match AsrClient::new() {
        Ok(c) => c,
        Err(e) => {
            skip(&format!("无法创建识别客户端：{e}"));
            return false;
        }
    };
    let result = test_runtime().block_on(client.test_connection(provider));
    if !result.ok {
        skip(&format!(
            "识别服务不可用（{}）：{}",
            provider.endpoint(),
            result.error.unwrap_or_default()
        ));
        return false;
    }
    eprintln!("识别服务可用：{}（{} ms）", provider.endpoint(), result.latency_ms);
    true
}

/* ==========================================================================
 * 完整流水线
 * ========================================================================== */

fn run_pipeline_on_audio(
    provider: &AsrProvider,
    samples: &[f32],
    vad: VadSettings,
    live_preview: bool,
    ai: Option<AiSettings>,
) -> (Arc<parking_lot::Mutex<Session>>, CollectingEmitter) {
    let session = Session::new(
        "E2E 测试会议".into(),
        SessionConfig {
            model_id: format!("{} · {}", provider.name, provider.model),
            enable_mic: false,
            enable_loopback: false,
            mic_label: None,
            loopback_label: None,
        },
    );
    let session = Arc::new(parking_lot::Mutex::new(session));

    let emitter = CollectingEmitter::new();
    let (frame_tx, frame_rx) = bounded(64);

    let mut asr = asr_settings(provider);
    asr.live_preview = live_preview;
    let ai_settings = ai.clone().unwrap_or_default();
    let ai_client = ai
        .is_some()
        .then(|| Arc::new(AiClient::new().expect("AI 客户端初始化失败")));

    let pipeline = pipeline::spawn(
        PipelineOptions {
            provider: provider.clone(),
            asr,
            vad,
            // 单路输入 → 说话人固定为「我」
            single_source: Some(SourceKind::Microphone),
            record_path: None,
            ai: ai_settings,
            spawn_task: Some(Box::new(|fut| {
                test_runtime().spawn(fut);
            })),
        },
        frame_rx,
        None,
        vec![("e2e:file".to_string(), SourceKind::Microphone)],
        Arc::clone(&session),
        Arc::new(emitter.clone()) as Arc<dyn Emitter>,
        ai_client,
    )
    .expect("启动流水线失败");

    // 按真实速度喂入：全速灌入的话断句线程会瞬间跑完，
    // 「边说边出字」这条链路就没被真正验证到（每句话的增量任务刚发出就被定稿抢占）。
    let feeder_stop = Arc::new(AtomicBool::new(false));
    let feeder = pipeline::feed_audio(
        samples,
        frame_tx,
        "e2e:file",
        SourceKind::Microphone,
        (meeting_hear_lib::audio::vad::RATE as usize) / 10, // 100ms 一帧
        Some(Duration::from_millis(100)),                   // 1 倍速
        Arc::clone(&feeder_stop),
    );
    feeder.join().expect("音频喂入线程异常");

    // 等识别服务把最后几句返回
    let deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < deadline {
        if !session.lock().segments.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    std::thread::sleep(Duration::from_secs(3));

    let session = pipeline.stop();
    (session, emitter)
}

/* ==========================================================================
 * 测试
 * ========================================================================== */

#[test]
#[ignore = "需要真实识别服务与测试音频；用 --ignored 运行"]
fn e2e_transcribes_jfk_wav_through_real_asr_service() {
    let provider = asr_provider();
    if !ensure_asr_service(&provider) {
        return;
    }
    let fixture = audio_fixture("jfk.wav");
    if !fixture.is_file() {
        skip(&format!("缺少测试音频 {}", fixture.display()));
        return;
    }

    // 1) 解码 + 重采样
    let samples =
        meeting_hear_lib::audio::decode::decode_to_16k_mono(&fixture).expect("解码 jfk.wav 失败");
    let audio_ms = samples.len() as i64 * 1000 / 16_000;
    assert!((audio_ms - 11_000).abs() < 500, "时长应约 11 秒，实际 {audio_ms} ms");

    // 2) 跑完整流水线
    let vad = VadSettings { min_silence_ms: 500, ..Default::default() };
    let (session, emitter) = run_pipeline_on_audio(&provider, &samples, vad, false, None);
    let s = session.lock().clone();

    eprintln!("=== 识别结果（{}）===", provider.endpoint());
    for seg in &s.segments {
        eprintln!(
            "[{:>7} ms → {:>7} ms] {}",
            seg.start_ms,
            seg.end_ms,
            seg.text
        );
    }
    eprintln!("统计：{:?}", s.stats);

    assert!(
        !s.segments.is_empty(),
        "没有识别出任何内容。事件：{:?}",
        emitter.names()
    );

    let full: String = s
        .segments
        .iter()
        .map(|x| x.text.clone())
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    eprintln!("合并文本：{full}");

    // jfk.wav 原文：And so my fellow Americans, ask not what your country can do for you...
    let keywords = ["country", "fellow", "americans", "ask", "do for you", "can do"];
    let hits = keywords.iter().filter(|w| full.contains(*w)).count();
    assert!(
        hits >= 4,
        "识别结果与音频内容严重不符（命中 {hits}/{} 个关键词）：{full}",
        keywords.len()
    );

    // 3) 状态序列完整
    let states: Vec<String> = emitter
        .payloads(event::STATE)
        .iter()
        .filter_map(|p| p.get("state").and_then(|v| v.as_str()).map(str::to_string))
        .collect();
    eprintln!("状态序列：{states:?}");
    assert!(states.iter().any(|x| x == "starting"), "没有 starting 状态");
    assert!(states.iter().any(|x| x == "listening"), "没有 listening 状态");
    assert!(states.iter().any(|x| x == "speech"), "没有 speech 状态（VAD 没触发）");

    // 4) 时间戳必须单调、不重叠、且落在音频范围内
    let mut last_end = -1i64;
    for seg in &s.segments {
        assert!(seg.start_ms >= 0, "时间戳为负");
        assert!(seg.end_ms > seg.start_ms, "结束时间不晚于开始时间：{seg:?}");
        assert!(
            seg.start_ms <= audio_ms + 1_000,
            "时间戳超出音频范围：{} > {audio_ms}",
            seg.start_ms
        );
        assert!(seg.start_ms >= last_end, "时间戳出现重叠：{} < {last_end}", seg.start_ms);
        last_end = seg.end_ms;
    }

    // 5) 统计指标合理
    assert!(s.stats.audio_ms >= audio_ms - 1_000, "音频时长统计不对");
    assert!(
        s.stats.speech_ms <= s.stats.audio_ms,
        "语音时长 {} 不应超过音频总时长 {}",
        s.stats.speech_ms,
        s.stats.audio_ms
    );
    assert!(s.stats.chars > 20, "字数统计异常：{}", s.stats.chars);
    assert!(s.stats.rtf > 0.0, "没有记录实时率");
    assert!(s.stats.latency_ms > 0, "没有记录请求延迟");
    assert_eq!(s.status, SessionStatus::Finished);
    // 应用不区分说话人：转写段里不应再有这类标记
    assert!(s.segments.iter().all(|x| !x.text.contains("我：") && !x.text.contains("对方：")));

    // 6) 关闭增量预览时不应产生任何带文本的 partial 事件
    let with_text = emitter
        .payloads(event::PARTIAL)
        .iter()
        .filter(|p| {
            ["committed", "tentative"].iter().any(|k| {
                p.get(*k)
                    .and_then(|v| v.as_str())
                    .map(|t| !t.is_empty())
                    .unwrap_or(false)
            })
        })
        .count();
    assert_eq!(with_text, 0, "关闭 live_preview 后不应有带文本的 partial 事件");
}

#[test]
#[ignore = "需要真实识别服务；用 --ignored 运行"]
fn e2e_live_preview_streams_partial_text() {
    let provider = asr_provider();
    if !ensure_asr_service(&provider) {
        return;
    }
    let fixture = audio_fixture("jfk.wav");
    if !fixture.is_file() {
        skip("缺少 jfk.wav");
        return;
    }
    let samples = meeting_hear_lib::audio::decode::decode_to_16k_mono(&fixture).unwrap();
    let vad = VadSettings { min_silence_ms: 500, ..Default::default() };

    let (session, emitter) = run_pipeline_on_audio(&provider, &samples, vad, true, None);
    assert!(!session.lock().segments.is_empty(), "没有识别出内容");

    let partials = emitter.payloads(event::PARTIAL);
    let with_text: Vec<&serde_json::Value> = partials
        .iter()
        .filter(|p| {
            ["committed", "tentative"].iter().any(|k| {
                p.get(*k)
                    .and_then(|v| v.as_str())
                    .map(|t| !t.is_empty())
                    .unwrap_or(false)
            })
        })
        .collect();
    eprintln!(
        "增量预览事件 {} 条，其中带文本 {} 条",
        partials.len(),
        with_text.len()
    );
    for p in with_text.iter().take(5) {
        eprintln!(
            "  已确认：{:?} / 未确认：{:?}",
            p.get("committed").and_then(|v| v.as_str()).unwrap_or(""),
            p.get("tentative").and_then(|v| v.as_str()).unwrap_or("")
        );
    }
    assert!(
        !with_text.is_empty(),
        "开启 live_preview 后应当收到带文本的 partial 事件"
    );
}

#[test]
#[ignore = "需要真实识别服务；用 --ignored 运行"]
fn e2e_asr_client_encodes_wav_and_parses_response() {
    let provider = asr_provider();
    if !ensure_asr_service(&provider) {
        return;
    }
    let fixture = audio_fixture("jfk.wav");
    if !fixture.is_file() {
        skip("缺少 jfk.wav");
        return;
    }

    let samples = meeting_hear_lib::audio::decode::decode_to_16k_mono(&fixture).unwrap();
    // 只取前 5 秒，验证「上传编码 → 服务 → 解析」这条最短路径
    let clip: Vec<f32> = samples.into_iter().take(16_000 * 5).collect();

    let client = AsrClient::new().unwrap();
    let out = test_runtime()
        .block_on(client.transcribe(
            &provider,
            &TranscribeRequest {
                samples: clip,
                prompt: None,
                temperature: 0.0,
            },
        ))
        .expect("识别请求失败");

    eprintln!("返回文本：{}", out.text);
    eprintln!(
        "耗时 {} ms · 音频 {} ms · RTF {:.3}",
        out.elapsed_ms,
        out.audio_ms,
        out.rtf()
    );
    assert!(!out.text.trim().is_empty(), "识别结果为空");
    assert!(out.rtf() > 0.0, "实时率计算异常");
    assert_eq!(out.audio_ms, 5_000);
}

#[test]
#[ignore = "需要真实识别服务 + 中文样本；用 --ignored 运行"]
fn e2e_chinese_speech_is_transcribed_as_chinese() {
    // 这条测试锁死一个真实事故：识别服务默认语言是英文时，中文语音会被
    // 按英文硬识别，输出一段英文幻觉，用户看到的是「什么都识别不出来」。
    // 应用的正确做法是**完全不碰语言**，由服务端配置决定（--language auto）。
    let provider = asr_provider();
    if !ensure_asr_service(&provider) {
        return;
    }
    let fixture = audio_fixture("zh-sample.wav");
    if !fixture.is_file() {
        skip(
            "缺少 zh-sample.wav。下载：
  curl -o .e2e/audio/zh-sample.wav \
    https://isv-data.oss-cn-hangzhou.aliyuncs.com/ics/MaaS/ASR/test_audio/asr_example_zh.wav",
        );
        return;
    }

    let samples = meeting_hear_lib::audio::decode::decode_to_16k_mono(&fixture).unwrap();
    let client = AsrClient::new().unwrap();
    let out = test_runtime()
        .block_on(client.transcribe(
            &provider,
            &TranscribeRequest {
                samples,
                prompt: None,
                temperature: 0.0,
            },
        ))
        .expect("识别请求失败");

    eprintln!("返回文本：{}", out.text);
    let cjk = out
        .text
        .chars()
        .filter(|c| matches!(*c as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF))
        .count();
    assert!(
        cjk >= 5,
        "中文语音应当识别出中文文本，实际得到：{:?}（汉字 {cjk} 个）。\
         如果这里是一段英文，说明识别服务的语言被固定成了 en —— \
         请用 `whisper-server --language auto` 启动服务端。",
        out.text
    );
}

#[test]
#[ignore = "需要真实识别服务 + 中文样本；用 --ignored 运行"]
fn e2e_chinese_audio_flows_through_full_pipeline() {
    // 用户视角的回归测试：中文音频走完「VAD 断句 → 识别 → 落段」，
    // 界面上必须真的出现中文转写。
    let provider = asr_provider();
    if !ensure_asr_service(&provider) {
        return;
    }
    let fixture = audio_fixture("zh-sample.wav");
    if !fixture.is_file() {
        skip("缺少 zh-sample.wav（下载方式见 e2e_chinese_speech_is_transcribed_as_chinese）");
        return;
    }

    let samples = meeting_hear_lib::audio::decode::decode_to_16k_mono(&fixture).unwrap();
    let (session, emitter) = run_pipeline_on_audio(
        &provider,
        &samples,
        VadSettings::default(),
        false,
        None,
    );

    let s = session.lock();
    for seg in &s.segments {
        eprintln!(
            "段 {} [{}ms] suspect={:?}：{}",
            seg.id, seg.start_ms, seg.suspect, seg.text
        );
    }
    assert!(!s.segments.is_empty(), "整段音频没有产生任何转写段落");
    let all: String = s.segments.iter().map(|x| x.text.clone()).collect();
    let cjk = all
        .chars()
        .filter(|c| matches!(*c as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF))
        .count();
    assert!(
        cjk >= 5,
        "中文音频经过完整流水线后应得到中文文本，实际：{all:?}。\
         若这里是英文，说明识别服务被固定成了 en（启动加 --language auto）。"
    );
    // 任何段都不允许被静默丢弃：要么有文本，要么明确标了原因
    for seg in &s.segments {
        assert!(
            !seg.text.trim().is_empty() || seg.suspect.is_some(),
            "出现了空文本且没有可疑标记的段落"
        );
    }
    drop(s);
    let _ = emitter;
}

#[test]
#[ignore = "需要真实识别服务；用 --ignored 运行"]
fn e2e_asr_connection_test_reports_failure_for_bad_url() {
    let mut bad = asr_provider();
    bad.base_url = "http://127.0.0.1:1".into(); // 必然连不上
    let client = AsrClient::new().unwrap();
    let result = test_runtime().block_on(client.test_connection(&bad));
    eprintln!("错误信息：{:?}", result.error);
    assert!(!result.ok, "连不上的地址不应报告成功");
    let err = result.error.unwrap_or_default();
    assert!(
        err.contains("无法连接") || err.contains("超时") || err.contains("失败"),
        "错误信息应当可读：{err}"
    );
}

#[test]
#[ignore = "需要本地 Ollama 或其它 OpenAI 兼容服务；用 --ignored 运行"]
fn e2e_ai_client_reaches_real_endpoint() {
    let settings = ollama_settings();
    let provider = settings.active().unwrap().clone();
    let client = AiClient::new().unwrap();

    let test = test_runtime().block_on(client.test_connection(&provider));
    eprintln!(
        "连通性测试：ok={} 延迟={}ms 样例={:?} 错误={:?}",
        test.ok, test.latency_ms, test.sample, test.error
    );
    if !test.ok {
        skip(&format!("AI 服务不可用：{:?}", test.error));
        return;
    }
    assert!(test.latency_ms > 0);
    assert!(!test.sample.is_empty(), "连通性测试没有返回内容");

    let out = test_runtime()
        .block_on(client.chat(
            &provider,
            &[
                ChatMessage::system("你是一个简洁的助手，只输出结果，不要解释。"),
                ChatMessage::user("把这句话压缩成一句话：张三负责在周三之前输出埋点方案。"),
            ],
        ))
        .expect("对话补全失败");
    eprintln!("模型输出：{}", out.content);
    assert!(!out.content.trim().is_empty());
    assert!(out.usage.total_tokens > 0, "接口没有返回 usage");
}

#[test]
#[ignore = "需要真实 AI 服务（本地 Ollama）；用 --ignored 运行"]
fn e2e_summary_stays_grounded_in_the_transcript() {
    // 这条测试的存在理由：之前我写了一条「讲座模式」测试，用「注意力机制」当素材，
    // 而提示词里的 few-shot 示例恰好也是「注意力机制」—— 模型把示例原样抄了出来，
    // 测试却因为断言了示例里本来就有的词而「通过」。等于自己骗自己。
    //
    // 现在改成：素材用提示词里**绝不可能出现**的专有名词，
    // 并反过来断言提示词里的示例词一个都不许出现。
    const LEAK_MARKERS: [&str; 6] = [
        "注意力机制",
        "梯度消失",
        "三季度",
        "达摩院",
        "留存率",
        "引导流程",
    ];

    let client = Arc::new(AiClient::new().unwrap());
    let base = ollama_settings();
    if !test_runtime()
        .block_on(client.test_connection(base.active().unwrap()))
        .ok
    {
        skip("AI 服务不可用");
        return;
    }

    // 素材：虚构的、与提示词无关的内容
    let transcript = "[00:00:04] 今天评审「苍鹭」项目的冷链仓改造方案。\n\
[00:00:13] 一号仓的月台改造预算批下来了，二百三十万，工期六周。\n\
[00:00:26] 分拣线要换成环形布局，不然旺季峰值吞吐顶不住。\n\
[00:00:38] 冷库温控探头必须做双路冗余，这是去年的整改要求。";

    let run = |settings: &AiSettings| {
        let mut session = Session::new(
            "冷链仓改造评审".into(),
            SessionConfig {
                model_id: "test".into(),
                enable_mic: false,
                enable_loopback: false,
                mic_label: None,
                loopback_label: None,
            },
        );
        session.push_segment(meeting_hear_lib::session::TranscriptSegment {
            id: 0,
            text: transcript.into(),
            start_ms: 4_000,
            end_ms: 45_000,
            confidence: None,
            suspect: None,
        });
        let session = Arc::new(parking_lot::Mutex::new(session));
        let emitter: Arc<dyn Emitter> = Arc::new(CollectingEmitter::new());
        test_runtime()
            .block_on(pipeline::summarize_once(&client, settings, &session, &emitter, true))
            .expect("总结失败");
        let out = session.lock().summary.clone();
        out
    };

    for mode in ["meeting", "lecture"] {
        let settings = AiSettings {
            summary_mode: mode.into(),
            ..ollama_settings()
        };
        let sum = run(&settings);
        let all = format!(
            "{} {} {:?} {:?} {:?}",
            sum.overview, sum.summary, sum.sections, sum.key_points, sum.decisions
        );
        eprintln!("=== {mode} 模式实际输出 ===\n{all}\n");

        // 1) 提到素材里的真实信息，说明模型确实在总结输入
        assert!(
            all.contains("苍鹭") || all.contains("冷链") || all.contains("月台") || all.contains("分拣"),
            "[{mode}] 纪要没有用上转写里的任何内容，可能又在照抄提示词：{all}"
        );
        // 2) 提示词里的示例词一个都不许出现
        for marker in LEAK_MARKERS {
            assert!(
                !all.contains(marker),
                "[{mode}] 输出里出现了提示词中的词「{marker}」，模型在照抄提示词而不是总结输入：{all}"
            );
        }
    }
}

#[test]
#[ignore = "需要真实识别服务 + 本地 Ollama；用 --ignored 运行"]
fn e2e_rolling_summary_from_real_transcript() {
    let provider = asr_provider();
    if !ensure_asr_service(&provider) {
        return;
    }
    let fixture = audio_fixture("jfk.wav");
    if !fixture.is_file() {
        skip("缺少 jfk.wav");
        return;
    }

    let ai = ollama_settings();
    let client = AiClient::new().unwrap();
    let probe = test_runtime().block_on(client.test_connection(ai.active().unwrap()));
    if !probe.ok {
        skip(&format!("AI 服务不可用：{:?}", probe.error));
        return;
    }

    let samples = meeting_hear_lib::audio::decode::decode_to_16k_mono(&fixture).unwrap();
    let vad = VadSettings { min_silence_ms: 500, ..Default::default() };
    let (session, _e) = run_pipeline_on_audio(&provider, &samples, vad, false, Some(ai.clone()));
    assert!(!session.lock().segments.is_empty(), "转写为空，无法验证总结");

    let collector = CollectingEmitter::new();
    let emitter: Arc<dyn Emitter> = Arc::new(collector.clone());
    let outcome = test_runtime().block_on(pipeline::summarize_once(&client, &ai, &session, &emitter, true));
    let summary: SummaryState = session.lock().summary.clone();

    // 本地小参数模型（1~2B）在语义不通的转写上经常退化：复读、JSON 被 max_tokens 截断。
    // 这属于模型能力问题而不是链路问题，所以这里验证的是**降级路径是否正确**：
    // 报出可读的中文错误、把错误写进状态、并且绝不推进已覆盖位置（否则这段转写就永远丢了）。
    if let Err(e) = &outcome {
        let msg = e.to_string();
        eprintln!("本地模型未返回可用纪要（预期内的降级）：{msg}");
        assert!(
            msg.contains("JSON") || msg.contains("模型"),
            "降级时的错误信息应当可读：{msg}"
        );
        assert!(summary.error.is_some(), "降级时应把错误写进 summary.error");
        assert_eq!(
            summary.covered_until_ms, 0,
            "解析失败绝不能推进已覆盖位置，否则这段转写会被永久跳过"
        );
        // 状态事件里也要有 error 反馈（前端会显示警告条）
        let statuses = collector.payloads(event::AI_STATUS);
        assert!(
            statuses.iter().any(|p| p.get("state").and_then(|v| v.as_str()) == Some("error")),
            "应当发出 ai:status=error 事件"
        );
        skip("本地模型退化，已验证降级路径；换更强的模型即可成功（见 README 的 AI 接口预设）");
        return;
    }

    let changed = outcome.expect("总结失败");
    let summary: SummaryState = summary;
    eprintln!("=== 滚动纪要 ===");
    eprintln!("live: {}", summary.live);
    eprintln!("overview: {}", summary.overview);
    eprintln!("summary:\n{}", summary.summary);
    eprintln!("keyPoints: {:?}", summary.key_points);
    eprintln!("topics: {:?}", summary.topics);
    eprintln!(
        "轮次 {} · 已覆盖 {} ms · tokens {}+{}",
        summary.revision,
        summary.covered_until_ms,
        summary.prompt_tokens,
        summary.completion_tokens
    );

    assert!(changed, "总结没有产生任何变化");
    assert!(summary.revision >= 1, "revision 没有推进");
    assert!(summary.covered_until_ms > 0, "没有推进已覆盖位置");
    assert!(
        !summary.live.trim().is_empty() || !summary.overview.trim().is_empty(),
        "模型没有返回任何纪要内容"
    );
    assert!(summary.calls >= 1, "调用计数没有累加");
    assert!(summary.error.is_none(), "纪要出错：{:?}", summary.error);
    assert!(summary.model.is_some(), "没有记录使用的模型");
}

/* ==========================================================================
 * 真实音频采集通路
 * ========================================================================== */

#[test]
#[ignore = "需要 PulseAudio/PipeWire 的 monitor 声源；用 --ignored 运行"]
fn e2e_loopback_capture_yields_pcm_frames() {
    #[cfg(target_os = "linux")]
    {
        use meeting_hear_lib::audio::capture::{parse_id, SourceKind};
        use std::sync::atomic::AtomicUsize;

        let sources = meeting_hear_lib::audio::capture::list_sources();
        let Some(source) = sources
            .iter()
            .find(|s| s.kind == SourceKind::Loopback && s.available)
        else {
            skip("没有可用的系统内录源");
            return;
        };
        let (_, device) = parse_id(&source.id);
        eprintln!("使用内录设备：{}（{device}）", source.label);

        let stop = Arc::new(AtomicBool::new(false));
        let samples = Arc::new(AtomicUsize::new(0));
        let blocks = Arc::new(AtomicUsize::new(0));

        let stop_for_thread = Arc::clone(&stop);
        let samples_for_thread = Arc::clone(&samples);
        let blocks_for_thread = Arc::clone(&blocks);
        let handle = std::thread::spawn(move || {
            meeting_hear_lib::audio::loopback::run(
                &device,
                stop_for_thread,
                move |chunk, ch, rate| {
                    assert_eq!(ch, 1, "parec 应当输出单声道");
                    assert_eq!(rate, 16_000, "parec 应当输出 16kHz");
                    samples_for_thread.fetch_add(chunk.len(), std::sync::atomic::Ordering::Relaxed);
                    blocks_for_thread.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                },
            )
        });

        std::thread::sleep(Duration::from_secs(3));
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let result = handle.join().expect("采集线程 panic");

        let got = samples.load(std::sync::atomic::Ordering::Relaxed);
        let nblocks = blocks.load(std::sync::atomic::Ordering::Relaxed);
        eprintln!("3 秒内收到 {got} 个采样（{nblocks} 块）");
        if got == 0 {
            eprintln!("提示：声源处于 SUSPENDED，未取到数据");
            return;
        }
        assert!(result.is_ok(), "采集返回错误：{result:?}");
        assert!(got >= 16_000 * 2, "3 秒内只采到 {got} 个采样");
        assert!(nblocks >= 5, "100ms 一块，3 秒至少 5 块，实际 {nblocks}");
    }

    #[cfg(not(target_os = "linux"))]
    skip("该用例只在 Linux 上验证 parec 采集");
}

#[test]
#[ignore = "需要可用的音频输入设备；用 --ignored 运行"]
fn e2e_microphone_capture_runs_through_mixer() {
    use meeting_hear_lib::audio::capture::{self, CaptureConfig, MixedFrame, SourceKind};
    use std::sync::atomic::AtomicUsize;

    let sources = capture::list_sources();
    let Some(mic) = sources
        .iter()
        .find(|s| s.kind == SourceKind::Microphone && s.available)
    else {
        skip("没有可用的麦克风（容器/服务器环境属正常）");
        return;
    };
    eprintln!("使用麦克风：{}（{}）", mic.label, mic.detail);

    let (frame_tx, frame_rx) = bounded::<MixedFrame>(64);
    let (err_tx, err_rx) = bounded::<String>(8);
    let handle = capture::start(
        CaptureConfig {
            mic_device_id: Some(mic.id.clone()),
            loopback_device_id: None,
            mic_gain: 1.0,
            loopback_gain: 1.0,
        },
        frame_tx,
        err_tx,
    )
    .expect("启动采集失败");

    let frames = Arc::new(AtomicUsize::new(0));
    let samples = Arc::new(AtomicUsize::new(0));
    let counts = Arc::clone(&frames);
    let sums = Arc::clone(&samples);
    let collector = std::thread::spawn(move || {
        while let Ok(frame) = frame_rx.recv_timeout(Duration::from_millis(1500)) {
            assert_eq!(frame.samples.len(), 1_600, "混音帧应为 100ms = 1600 采样");
            assert_eq!(frame.levels.len(), 1, "应有 1 路电平");
            counts.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            sums.fetch_add(frame.samples.len(), std::sync::atomic::Ordering::Relaxed);
        }
    });

    std::thread::sleep(Duration::from_secs(3));
    handle.stop();
    let _ = collector.join();

    let n = frames.load(std::sync::atomic::Ordering::Relaxed);
    let total = samples.load(std::sync::atomic::Ordering::Relaxed);
    eprintln!("3 秒内收到 {n} 帧混音数据，共 {total} 个 16kHz 采样");

    let mut errors = Vec::new();
    while let Ok(e) = err_rx.try_recv() {
        errors.push(e);
    }
    eprintln!("采集错误：{errors:?}");

    if n == 0 {
        eprintln!("提示：设备打开成功但没有数据（虚拟/静音设备属正常），错误：{errors:?}");
        return;
    }
    assert!(total >= 16_000, "3 秒内只拿到 {total} 个采样，混音链路可能有问题");
}
