//! 流式重采样：任意采样率 → 16000 Hz 单声道 f32。
//!
//! 为什么自己写而不用现成库：音频设备常见 48000/44100/32000，目标是固定的 16000，
//! 需求非常单一，自己实现一个多相 FIR 可以完全掌控混叠抑制（抗混叠）行为，
//! 而且能离线做精确的数值测试。
//!
//! 实现：加窗 sinc（Kaiser 窗）多相滤波器
//!   - 截止频率 `fc = 0.95 * min(1, out/in)`（相对输入 Nyquist），降采样时抑制混叠
//!   - 每个相位单独归一化，保证直流增益精确为 1（不会忽大忽小）
//!   - 流式：内部保留输入历史，允许任意大小的输入分片

/// Kaiser 窗的 beta 参数：越大旁瓣越低（阻带衰减越好），主瓣越宽
const KAISER_BETA: f32 = 8.6;
/// 相位表精度
const PHASES: usize = 256;
/// 目标采样率，whisper 要求 16 kHz
pub const TARGET_RATE: u32 = 16_000;

pub struct Resampler {
    in_rate: u32,
    out_rate: u32,
    /// 单边抽头数
    half: usize,
    /// 相位表：phases × (2*half)
    table: Vec<f32>,
    /// 待处理的输入样本
    buf: Vec<f32>,
    /// 下一个输出样本对应的输入位置（以「真实输入样本序号」计，可为小数）
    next_pos: f64,
    /// 累计喂入的输入样本数（flush 时据此推算总共该产出多少输出）
    total_in: u64,
    /// 累计产出的输出样本数
    total_out: u64,
    /// 是否直通（采样率相同）
    passthrough: bool,
}

impl Resampler {
    pub fn new(in_rate: u32, out_rate: u32) -> Self {
        let in_rate = in_rate.max(1);
        let out_rate = out_rate.max(1);
        if in_rate == out_rate {
            return Self {
                in_rate,
                out_rate,
                half: 0,
                table: Vec::new(),
                buf: Vec::new(),
                next_pos: 0.0,
                total_in: 0,
                total_out: 0,
                passthrough: true,
            };
        }

        // 降采样倍数越大，需要越长的滤波器才能压住混叠
        let ratio = in_rate as f64 / out_rate as f64;
        let half = ((20.0 * ratio.max(1.0)).ceil() as usize).clamp(16, 200);
        let table = build_table(in_rate, out_rate, half);

        Self {
            in_rate,
            out_rate,
            half,
            table,
            buf: Vec::new(),
            next_pos: 0.0,
            total_in: 0,
            total_out: 0,
            passthrough: false,
        }
    }

    pub fn in_rate(&self) -> u32 {
        self.in_rate
    }

    pub fn out_rate(&self) -> u32 {
        self.out_rate
    }

    /// 输入与输出采样率相同，直接拷贝
    pub fn is_passthrough(&self) -> bool {
        self.passthrough
    }

    /// 每次调用会追加输出样本到 `out`（不自动清空）
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if input.is_empty() {
            return;
        }
        if self.passthrough {
            self.total_in += input.len() as u64;
            self.total_out += input.len() as u64;
            out.extend_from_slice(input);
            return;
        }

        self.total_in += input.len() as u64;
        self.buf.extend_from_slice(input);

        let step = self.in_rate as f64 / self.out_rate as f64;
        let taps = 2 * self.half;

        // 需要 i0 + half 处的样本才完整，故只需 buf 长度 > next_pos + half
        while (self.next_pos + self.half as f64) < self.buf.len() as f64 {
            let sample = self.convolve(taps);
            out.push(sample);
            self.total_out += 1;
            self.next_pos += step;
        }

        // 丢弃已经不可能再被用到的历史样本
        let consumed = self.next_pos - self.half as f64 + 1.0;
        if consumed > 0.0 {
            let drop = (consumed as usize).min(self.buf.len());
            if drop > 0 {
                self.buf.drain(..drop);
                self.next_pos -= drop as f64;
            }
        }
    }

    /// 结束输入时的收尾：按总输入量推算应有的输出样本数，用零填充补齐尾部
    pub fn flush(&mut self, out: &mut Vec<f32>) {
        if self.passthrough {
            return;
        }
        let taps = 2 * self.half;
        let step = self.in_rate as f64 / self.out_rate as f64;
        let target = ((self.total_in as f64) * self.out_rate as f64 / self.in_rate as f64).floor() as u64;

        while self.total_out < target {
            // 卷积窗口越过缓冲区末端时按零（静音）处理
            let need = self.next_pos.floor().max(0.0) as usize + self.half + 1;
            if need > self.buf.len() {
                self.buf.resize(need, 0.0);
            }
            let sample = self.convolve(taps);
            out.push(sample);
            self.total_out += 1;
            self.next_pos += step;
        }
    }

    /// 当前缓冲里还剩多少样本（用于诊断）
    pub fn pending(&self) -> usize {
        self.buf.len()
    }

    /// 复位到初始状态（保留滤波器系数）
    pub fn reset(&mut self) {
        self.buf.clear();
        self.next_pos = 0.0;
        self.total_in = 0;
        self.total_out = 0;
    }

    #[inline]
    fn convolve(&self, taps: usize) -> f32 {
        let mut i0 = self.next_pos.floor();
        let frac = (self.next_pos - i0) as f32;
        let mut phase = (frac * PHASES as f32).round() as usize;
        // frac 逼近 1 时 round 会得到 PHASES：此时等价于「下一个整数位置 + 相位 0」，
        // 必须同时把 i0 前进 1，否则系数与样本位错开一格（会造成 ~1 个采样的相位误差，
        // 且与输入分片方式相关，属于隐蔽 bug）。
        if phase >= PHASES {
            phase = 0;
            i0 += 1.0;
        }
        let coeffs = &self.table[phase * taps..phase * taps + taps];

        // 真实输入序号 i0 + k 对应 buf 下标 i0 + k，k 从 -(half-1) 到 half
        let base = i0 as i64 - self.half as i64 + 1;
        let mut acc = 0.0f32;
        for (k, &c) in coeffs.iter().enumerate() {
            let idx = base + k as i64;
            if idx < 0 {
                // 起始处的热身区，视作静音
                continue;
            }
            let idx = idx as usize;
            if idx >= self.buf.len() {
                break;
            }
            acc += self.buf[idx] * c;
        }
        acc
    }
}

/// 生成相位表，并对每个相位做直流增益归一化
fn build_table(in_rate: u32, out_rate: u32, half: usize) -> Vec<f32> {
    let taps = 2 * half;
    // 截止频率（相对输入 Nyquist）：降采样时压到输出 Nyquist 以下留 5% 过渡带
    let ratio = out_rate as f64 / in_rate as f64;
    let cutoff = 0.95 * ratio.min(1.0);
    let mut table = vec![0.0f32; PHASES * taps];

    for phase in 0..PHASES {
        let frac = phase as f64 / PHASES as f64;
        let row = &mut table[phase * taps..phase * taps + taps];
        let mut sum = 0.0f64;
        for k in 0..taps {
            // k = 0 对应 t = -(half-1)，k = taps-1 对应 t = +half
            let t = k as f64 - (half as f64 - 1.0) - frac;
            let x = cutoff * t;
            let sinc = if x.abs() < 1e-9 {
                1.0
            } else {
                (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
            };
            // Kaiser 窗
            let r = t / half as f64;
            let w = if r.abs() >= 1.0 { 0.0 } else { kaiser(r, KAISER_BETA as f64) };
            let v = cutoff * sinc * w;
            row[k] = v as f32;
            sum += v;
        }
        // 归一化：保证每个相位的系数和恰为 1，直流增益精确
        if sum.abs() > 1e-12 {
            for v in row.iter_mut() {
                *v = (*v as f64 / sum) as f32;
            }
        }
    }
    table
}

/// Kaiser 窗：I0(beta*sqrt(1-r^2)) / I0(beta)
fn kaiser(r: f64, beta: f64) -> f64 {
    let arg = beta * (1.0 - r * r).max(0.0).sqrt();
    bessel_i0(arg) / bessel_i0(beta)
}

/// 零阶第一类修正贝塞尔函数（级数展开，收敛很快）
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0f64;
    let mut term = 1.0f64;
    let half_x = x / 2.0;
    for k in 1..=40 {
        term *= (half_x / k as f64) * (half_x / k as f64);
        sum += term;
        if term < 1e-16 * sum {
            break;
        }
    }
    sum
}

/// 多声道 → 单声道
pub fn downmix_to_mono(interleaved: &[f32], channels: u16, out: &mut Vec<f32>) {
    let ch = channels.max(1) as usize;
    if ch == 1 {
        out.extend_from_slice(interleaved);
        return;
    }
    out.reserve(interleaved.len() / ch);
    for frame in interleaved.chunks_exact(ch) {
        let sum: f32 = frame.iter().copied().sum();
        out.push(sum / ch as f32);
    }
}

/// 一阶高通，去掉直流偏置与低频隆隆声（对 ASR 有正向作用）
pub struct DcBlocker {
    prev_in: f32,
    prev_out: f32,
    r: f32,
}

impl DcBlocker {
    /// `cutoff_hz` 建议 60~100 Hz
    pub fn new(sample_rate: u32, cutoff_hz: f32) -> Self {
        let rc = 1.0 / (2.0 * std::f32::consts::PI * cutoff_hz.max(1.0));
        let dt = 1.0 / sample_rate.max(1) as f32;
        Self { prev_in: 0.0, prev_out: 0.0, r: rc / (rc + dt) }
    }

    pub fn process_in_place(&mut self, buf: &mut [f32]) {
        for x in buf.iter_mut() {
            let y = self.r * (self.prev_out + *x - self.prev_in);
            self.prev_in = *x;
            self.prev_out = y;
            *x = y;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, rate: u32, len: usize, amp: f32) -> Vec<f32> {
        (0..len)
            .map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin())
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        if x.is_empty() {
            return 0.0;
        }
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    /// 主频能量估计：做过零率统计（对纯音足够准）
    fn zero_crossing_freq(x: &[f32], rate: u32) -> f32 {
        let mut crossings = 0usize;
        for w in x.windows(2) {
            if (w[0] <= 0.0) != (w[1] <= 0.0) {
                crossings += 1;
            }
        }
        crossings as f32 * rate as f32 / (2.0 * x.len() as f32)
    }

    fn resample_all(input: &[f32], in_rate: u32, out_rate: u32, chunk: usize) -> Vec<f32> {
        let mut r = Resampler::new(in_rate, out_rate);
        let mut out = Vec::new();
        for c in input.chunks(chunk) {
            r.process(c, &mut out);
        }
        r.flush(&mut out);
        out
    }

    #[test]
    fn output_length_matches_ratio() {
        for (in_rate, out_rate) in [(48_000u32, 16_000u32), (44_100, 16_000), (32_000, 16_000), (16_000, 16_000)] {
            let input = sine(440.0, in_rate, in_rate as usize, 0.5); // 1 秒
            let out = resample_all(&input, in_rate, out_rate, 1024);
            let expected = out_rate as i64;
            let diff = (out.len() as i64 - expected).abs();
            assert!(
                diff <= 2,
                "{in_rate}→{out_rate} 期望约 {expected} 个样本，实际 {}",
                out.len()
            );
        }
    }

    #[test]
    fn passthrough_is_bit_exact() {
        let input = sine(440.0, 16_000, 1_000, 0.5);
        let out = resample_all(&input, 16_000, 16_000, 128);
        assert_eq!(out, input);
    }

    #[test]
    fn passband_tones_pass_unchanged() {
        // 语音有效频段（<4kHz）必须原样保留
        for freq in [300.0f32, 1_000.0, 3_000.0] {
            let input = sine(freq, 48_000, 48_000, 0.8);
            let out = resample_all(&input, 48_000, 16_000, 997); // 故意用非整除的分片
            let body = &out[400..out.len() - 400];
            let expected = 0.8 / std::f32::consts::SQRT_2;
            let got = rms(body);
            assert!(
                (got - expected).abs() / expected < 0.05,
                "{freq} Hz 通带幅度偏差过大：期望 {expected:.4}，实际 {got:.4}"
            );
            let f = zero_crossing_freq(body, 16_000);
            assert!((f - freq).abs() < freq * 0.05, "{freq} Hz 频率估计偏差过大：{f}");
        }
    }

    #[test]
    fn stopband_tones_are_suppressed() {
        // 高于输出 Nyquist(8kHz) 的成分必须被抗混叠滤波器压掉，否则会混叠成假音
        for freq in [10_000.0f32, 15_000.0, 20_000.0] {
            let input = sine(freq, 48_000, 48_000, 0.8);
            let out = resample_all(&input, 48_000, 16_000, 1024);
            let body = &out[600..out.len() - 600];
            let got = rms(body);
            assert!(
                got < 0.02,
                "混叠抑制不足：{freq} Hz 残留 RMS = {got:.5}（应 < 0.02，即衰减 >32dB）"
            );
        }
    }

    #[test]
    fn transition_band_is_attenuated_not_amplified() {
        // 8kHz 位于 7.6k~9k 过渡带内：不要求压死，但绝不能放大
        let input = sine(8_000.0, 48_000, 48_000, 0.8);
        let out = resample_all(&input, 48_000, 16_000, 1024);
        let body = &out[600..out.len() - 600];
        let got = rms(body);
        let expected = 0.8 / std::f32::consts::SQRT_2;
        assert!(got <= expected * 1.02, "过渡带被放大：{got:.4} > {expected:.4}");
    }

    #[test]
    fn dc_gain_is_unity() {
        let input = vec![0.5f32; 48_000];
        let out = resample_all(&input, 48_000, 16_000, 512);
        // 跳过暂态后应稳定在 0.5
        let body = &out[500..out.len() - 500];
        let mean: f32 = body.iter().sum::<f32>() / body.len() as f32;
        assert!((mean - 0.5).abs() < 1e-3, "直流增益异常：{mean}");
    }

    #[test]
    fn chunking_does_not_change_output() {
        // 分片大小不应影响结果（流式正确性）
        let input = sine(600.0, 44_100, 44_100, 0.7);
        let a = resample_all(&input, 44_100, 16_000, 128);
        let b = resample_all(&input, 44_100, 16_000, 4_096);
        assert_eq!(a.len(), b.len());
        let max_diff = a
            .iter()
            .zip(b.iter())
            .map(|(x, y)| (x - y).abs())
            .fold(0.0f32, f32::max);
        assert!(max_diff < 1e-6, "分片导致结果不一致，最大偏差 {max_diff}");
    }

    #[test]
    fn silence_stays_silent() {
        let input = vec![0.0f32; 16_000];
        let out = resample_all(&input, 48_000, 16_000, 256);
        assert!(out.iter().all(|v| v.abs() < 1e-6), "静音输入产生了噪声");
    }

    #[test]
    fn downmix_averages_channels() {
        // 立体声：左 1.0 右 0.0 → 0.5
        let interleaved = vec![1.0, 0.0, 1.0, 0.0];
        let mut out = Vec::new();
        downmix_to_mono(&interleaved, 2, &mut out);
        assert_eq!(out, vec![0.5, 0.5]);

        let mut out2 = Vec::new();
        downmix_to_mono(&interleaved, 1, &mut out2);
        assert_eq!(out2, interleaved);
    }

    #[test]
    fn dc_blocker_removes_offset_but_keeps_tone() {
        let rate = 16_000u32;
        let mut signal: Vec<f32> = sine(1_000.0, rate, 16_000, 0.5)
            .iter()
            .map(|v| v + 0.3) // 直流偏置
            .collect();
        let mut dc = DcBlocker::new(rate, 80.0);
        dc.process_in_place(&mut signal);
        let body = &signal[2_000..];
        let mean: f32 = body.iter().sum::<f32>() / body.len() as f32;
        assert!(mean.abs() < 0.01, "直流未被去除：{mean}");
        let got = rms(body);
        let expected = 0.5 / std::f32::consts::SQRT_2;
        assert!((got - expected).abs() / expected < 0.05, "音调被削弱：{got}");
    }

    #[test]
    fn bessel_is_accurate() {
        // 标准值：I0(0)=1, I0(1)=1.2660658777520084, I0(2)=2.2795853023360673, I0(3)=4.880792585865024
        assert!((bessel_i0(0.0) - 1.0).abs() < 1e-12);
        assert!((bessel_i0(1.0) - 1.266_065_877_752_008_4).abs() < 1e-12);
        assert!((bessel_i0(2.0) - 2.279_585_302_336_067_3).abs() < 1e-12);
        assert!((bessel_i0(3.0) - 4.880_792_585_865_024).abs() < 1e-12);
        // 单调递增性（大 x 时的数值稳定性）
        assert!(bessel_i0(8.6) > bessel_i0(8.0));
        assert!(bessel_i0(8.6).is_finite());
    }

    #[test]
    fn kaiser_window_endpoints() {
        let beta = KAISER_BETA as f64;
        // 端点值为 1/I0(beta)，并不为 0（Kaiser 窗的特性）
        assert!((kaiser(0.0, beta) - 1.0).abs() < 1e-12);
        assert!((kaiser(1.0, beta) - 1.0 / bessel_i0(beta)).abs() < 1e-12);
        assert!(kaiser(1.0, beta) < 0.01, "端点值应很小，实际 {}", kaiser(1.0, beta));
        // 中间单调下降
        assert!(kaiser(0.2, beta) > kaiser(0.5, beta));
        assert!(kaiser(0.5, beta) > kaiser(0.8, beta));
    }
}
