//! 录音落盘（16kHz 单声道 16bit WAV）。
//!
//! 用途：回听、事后重新转写、给用户留一份原始证据。

use std::io::BufWriter;
use std::path::{Path, PathBuf};

use crate::error::{AppError, AppResult};

pub struct WavRecorder {
    writer: hound::WavWriter<BufWriter<std::fs::File>>,
    path: PathBuf,
    samples: u64,
}

impl WavRecorder {
    /// 创建 16kHz / 单声道 / 16bit 的 WAV
    pub fn create(path: &Path) -> AppResult<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: crate::audio::vad::RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let writer = hound::WavWriter::create(path, spec)
            .map_err(|e| AppError::audio(format!("创建录音文件失败（{}）：{e}", path.display())))?;
        Ok(Self { writer, path: path.to_path_buf(), samples: 0 })
    }

    pub fn write(&mut self, samples: &[f32]) -> AppResult<()> {
        for s in samples {
            let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            self.writer
                .write_sample(v)
                .map_err(|e| AppError::audio(format!("写入录音失败：{e}")))?;
        }
        self.samples += samples.len() as u64;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn duration_ms(&self) -> i64 {
        self.samples as i64 * 1000 / crate::audio::vad::RATE as i64
    }

    /// 写完文件头（必须调用，否则 WAV 的 data 长度不正确）
    pub fn finalize(self) -> AppResult<()> {
        let path = self.path.clone();
        self.writer
            .finalize()
            .map_err(|e| AppError::audio(format!("完成录音文件失败：{e}")))?;
        tracing::info!("录音已保存：{}", path.display());
        Ok(())
    }
}

/// 把 16kHz 单声道 f32 编码成内存里的 WAV 字节（用于上传给识别服务）。
///
/// 选 16bit PCM 是因为它是所有识别服务的最大公约数；
/// 25 秒音频约 800KB，远低于各家 25MB 的上限。
pub fn encode_wav_16k_mono(samples: &[f32]) -> AppResult<Vec<u8>> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: crate::audio::vad::RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut cursor = std::io::Cursor::new(Vec::with_capacity(samples.len() * 2 + 64));
    {
        let mut writer = hound::WavWriter::new(&mut cursor, spec)
            .map_err(|e| AppError::audio(format!("编码 WAV 失败：{e}")))?;
        for s in samples {
            let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            writer
                .write_sample(v)
                .map_err(|e| AppError::audio(format!("编码 WAV 失败：{e}")))?;
        }
        writer
            .finalize()
            .map_err(|e| AppError::audio(format!("编码 WAV 失败：{e}")))?;
    }
    Ok(cursor.into_inner())
}

/// 读取 WAV 文件为 16k 单声道 f32（其它采样率/声道会自动转换）
pub fn read_wav_16k_mono(path: &Path) -> AppResult<Vec<f32>> {
    let mut reader = hound::WavReader::open(path)
        .map_err(|e| AppError::audio(format!("打开音频文件失败（{}）：{e}", path.display())))?;
    let spec = reader.spec();
    let channels = spec.channels.max(1) as usize;

    // 先取原始样本（可能是 i16 / i32 / f32）
    let raw: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Float, _) => reader
            .samples::<f32>()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AppError::audio(format!("读取音频数据失败：{e}")))?,
        (hound::SampleFormat::Int, bits) if bits <= 16 => reader
            .samples::<i16>()
            .map(|s| s.map(|v| v as f32 / 32768.0))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AppError::audio(format!("读取音频数据失败：{e}")))?,
        (hound::SampleFormat::Int, _) => {
            let scale = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 / scale))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| AppError::audio(format!("读取音频数据失败：{e}")))?
        }
    };

    let mut mono = Vec::with_capacity(raw.len() / channels);
    crate::audio::resample::downmix_to_mono(&raw, channels as u16, &mut mono);

    let mut resampler = crate::audio::resample::Resampler::new(spec.sample_rate, crate::audio::vad::RATE);
    let mut out = Vec::with_capacity(mono.len() * crate::audio::vad::RATE as usize / spec.sample_rate.max(1) as usize + 8);
    resampler.process(&mono, &mut out);
    resampler.flush(&mut out);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mh-wav-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn record_and_read_back() {
        let dir = tmpdir("roundtrip");
        let path = dir.join("rec.wav");
        let mut rec = WavRecorder::create(&path).unwrap();
        // 1 秒 440Hz
        let samples: Vec<f32> = (0..16_000)
            .map(|i| 0.5 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16_000.0).sin())
            .collect();
        for chunk in samples.chunks(1600) {
            rec.write(chunk).unwrap();
        }
        assert_eq!(rec.duration_ms(), 1_000);
        rec.finalize().unwrap();

        assert!(path.is_file());
        let read = read_wav_16k_mono(&path).unwrap();
        assert_eq!(read.len(), 16_000);
        // 幅度应基本一致（16bit 量化误差很小）
        let peak = read.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((peak - 0.5).abs() < 0.01, "峰值偏差过大：{peak}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn reads_stereo_48k_and_converts() {
        let dir = tmpdir("convert");
        let path = dir.join("stereo.wav");
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        // 0.5 秒，左右声道同相
        for i in 0..24_000 {
            let v = (0.4 * (2.0 * std::f32::consts::PI * 500.0 * i as f32 / 48_000.0).sin()
                * 32767.0) as i16;
            w.write_sample(v).unwrap();
            w.write_sample(v).unwrap();
        }
        w.finalize().unwrap();

        let read = read_wav_16k_mono(&path).unwrap();
        // 0.5 秒 @16k ≈ 8000 个样本
        assert!((read.len() as i64 - 8_000).abs() <= 4, "长度 {}", read.len());
        let peak = read.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((peak - 0.4).abs() < 0.02, "峰值偏差过大：{peak}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn encoded_wav_roundtrips() {
        let samples: Vec<f32> = (0..8_000)
            .map(|i| 0.4 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16_000.0).sin())
            .collect();
        let bytes = encode_wav_16k_mono(&samples).unwrap();
        // 有合法的 RIFF/WAVE 头
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");

        // 能被我们自己读回来
        let dir = tmpdir("encode");
        let path = dir.join("up.wav");
        std::fs::write(&path, &bytes).unwrap();
        let back = read_wav_16k_mono(&path).unwrap();
        assert_eq!(back.len(), samples.len());
        let peak = back.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((peak - 0.4).abs() < 0.01, "峰值偏差过大：{peak}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn encoding_clips_out_of_range() {
        let bytes = encode_wav_16k_mono(&[3.0, -3.0, 0.0, 0.5]).unwrap();
        assert!(!bytes.is_empty());
    }

    #[test]
    fn missing_file_reports_readable_error() {
        let err = read_wav_16k_mono(Path::new("/definitely/not/here.wav"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("音频文件"), "实际：{err}");
    }

    #[test]
    fn clips_out_of_range_samples() {
        let dir = tmpdir("clip");
        let path = dir.join("clip.wav");
        let mut rec = WavRecorder::create(&path).unwrap();
        rec.write(&[2.0, -2.0, 0.0]).unwrap();
        rec.finalize().unwrap();
        let read = read_wav_16k_mono(&path).unwrap();
        assert!(read.iter().all(|v| v.abs() <= 1.0001), "超范围样本应被限幅");
        let _ = std::fs::remove_dir_all(dir);
    }
}
