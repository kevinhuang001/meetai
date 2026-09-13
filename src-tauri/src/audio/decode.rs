//! 音频文件解码（用于「导入音频转写」）。
//!
//! WAV 用 Rust 直接解析；其它格式（mp3/m4a/flac/ogg…）交给系统里的 ffmpeg。
//! 不自带解码器是有意为之：把 ffmpeg 打进安装包会让体积翻好几倍。

use std::path::Path;
use std::process::{Command, Stdio};

use crate::error::{AppError, AppResult};

/// 支持的扩展名（用于文件选择对话框的过滤提示）
pub const COMMON_AUDIO_EXTENSIONS: &[&str] =
    &["wav", "mp3", "m4a", "aac", "flac", "ogg", "opus", "wma", "amr", "mp4", "mkv", "webm"];

/// 解码为 16kHz 单声道 f32
pub fn decode_to_16k_mono(path: &Path) -> AppResult<Vec<f32>> {
    if !path.is_file() {
        return Err(AppError::audio(format!("文件不存在：{}", path.display())));
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    if ext == "wav" {
        return crate::audio::wav::read_wav_16k_mono(path);
    }

    decode_with_ffmpeg(path)
}

fn decode_with_ffmpeg(path: &Path) -> AppResult<Vec<f32>> {
    let output = Command::new("ffmpeg")
        .arg("-v")
        .arg("error")
        .arg("-i")
        .arg(path)
        .arg("-f")
        .arg("f32le")
        .arg("-ac")
        .arg("1")
        .arg("-ar")
        .arg(crate::audio::vad::RATE.to_string())
        .arg("-")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AppError::audio(format!(
                    "解码 {} 需要 ffmpeg，但系统里没有找到 ffmpeg。\
                     请安装 ffmpeg，或先把文件转成 WAV 再导入。",
                    path.display()
                ))
            } else {
                AppError::audio(format!("调用 ffmpeg 失败：{e}"))
            }
        })?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::audio(format!(
            "ffmpeg 解码失败（{}）：{}",
            path.display(),
            crate::util::truncate_chars(err.trim(), 300)
        )));
    }

    let bytes = output.stdout;
    if bytes.len() < 4 {
        return Err(AppError::audio("解码结果为空，文件可能损坏或没有音频轨道"));
    }

    let mut samples = Vec::with_capacity(bytes.len() / 4);
    for chunk in bytes.chunks_exact(4) {
        samples.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }
    Ok(samples)
}

/// 该文件能否被处理（给出更友好的前置校验）
pub fn is_supported(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    COMMON_AUDIO_EXTENSIONS.contains(&ext.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mh-decode-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn decodes_wav_without_ffmpeg() {
        let dir = tmpdir("wav");
        let path = dir.join("tone.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for i in 0..8_000 {
            let v = (0.3 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16_000.0).sin()
                * 32767.0) as i16;
            w.write_sample(v).unwrap();
        }
        w.finalize().unwrap();

        let samples = decode_to_16k_mono(&path).unwrap();
        assert_eq!(samples.len(), 8_000);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_file_is_rejected() {
        assert!(decode_to_16k_mono(Path::new("/no/such/file.wav")).is_err());
    }

    #[test]
    fn extension_support_check() {
        assert!(is_supported(Path::new("a/b.wav")));
        assert!(is_supported(Path::new("a/b.MP3")));
        assert!(is_supported(Path::new("meeting.m4a")));
        assert!(!is_supported(Path::new("notes.txt")));
        assert!(!is_supported(Path::new("noext")));
    }

    #[test]
    fn ffmpeg_error_is_readable_when_missing_or_bad_input() {
        let dir = tmpdir("bad");
        let fake = dir.join("fake.mp3");
        std::fs::write(&fake, b"this is not audio").unwrap();
        let err = decode_to_16k_mono(&fake).unwrap_err().to_string();
        // 要么提示需要安装 ffmpeg，要么给出 ffmpeg 的错误摘要
        assert!(
            err.contains("ffmpeg") || err.contains("解码"),
            "错误信息应可读：{err}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
