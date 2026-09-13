# MeetingHear · 会议实时转写与 AI 纪要

类「讯飞听见」的跨平台桌面应用：**实时把会议语音转成文字，同时用 AI 实时总结刚刚说到的内容**。

识别与纪要走**两个独立的、可自由配置的接口**：

| 能力 | 走什么 | 可选服务 |
| --- | --- | --- |
| 语音转文字 | 识别服务的 HTTP 接口（OpenAI 兼容 `/audio/transcriptions`，或 whisper.cpp server `/inference`） | Groq / OpenAI / 硅基流动 / **本地 whisper.cpp server** / 本地 faster-whisper-server / 自建网关 |
| 会议纪要 | OpenAI 兼容 `/chat/completions` | DeepSeek / OpenAI / 通义千问 / 智谱 / Kimi / **本地 Ollama** / vLLM |

**应用本身不含任何识别模型**，所以安装包很小、构建不需要 C++/CUDA 工具链，三平台都能一条命令打包。
想完全离线也行：本地起一个 whisper.cpp server，把 Base URL 指向它，效果与内置模型完全一样。

```
┌──────────────────────────────────────────────────────────────────────┐
│  ● 录音中  00:12:34      会议 09-13 20:47     Groq·whisper-v3   ⚙  🗂 │
├───────────────────────────────────┬──────────────────────────────────┤
│ [00:00:01] 我  各位，我们今天主要…   │  刚刚说到                        │
│ [00:00:06] 对方 我先说复盘，三季度…  │  明确了留存优先，新版本 11/10 灰度 │
│ [00:00:14] 我  ▌那下季度目标我建议…  │  ────────────────────────────    │
│                                   │  会议总览 / 关键要点 / 待办事项    │
├───────────────────────────────────┴──────────────────────────────────┤
│ ▮▮▮▮▯ 麦克风      ▮▮▯▯▯ 系统声音    RTF 0.12  延迟 242ms  字数 126   │
│              [开始录音] [暂停] [停止] [导入音频] [立即总结]           │
└──────────────────────────────────────────────────────────────────────┘
```

界面截图见 `artifacts/`（16 张，含深/浅色主题、设置各标签页、历史与详情）。

---

## 一、功能

**实时转写**
- 采集麦克风 + 系统声音，本地做 VAD 断句，**按句**调用识别服务
- **按句定稿**：一句话说完立刻出结果，时间戳精确、延迟有上界（实测请求往返 ~250ms）
- **边说边出字**（可选）：说话过程中每 800ms 对当前这句话增量识别一次，已确认部分正常显示、未确认部分灰色显示。注意这是每次一个请求，云端服务会显著增加调用量
- 识别服务返回的语言自动检测并锁定；上一句文本可作为 `prompt` 传给服务，提升人名/术语一致性
- **说话人归属**：同时采集麦克风与系统声音时，按两路能量占比自动区分「我 / 对方 / 双方」
- **垃圾输出过滤**：挡掉「谢谢观看」「字幕由…提供」这类识别模型在静音上的幻觉，以及退化重复循环

**AI 实时纪要**
- 播放中每 N 秒（默认 20s）滚动更新一次纪要，**上下文长度恒定**，两小时会议也不会越来越慢、越来越贵
- 一次调用同时产出：**刚刚说到** / 会议总览 / 纪要要点 / 关键结论 / 已达成的决定 / 待办事项（含负责人与截止时间）/ 当前主题
- 支持随时「立即总结」，停止后可生成完整结构化会议纪要
- 内置「测试连接」，支持任意 OpenAI 兼容服务（含本地 Ollama，完全离线）

**音频来源**
- 麦克风（三平台通用）
- **系统内录**：Windows 用 WASAPI loopback 开箱可用；Linux 用 PulseAudio/PipeWire 的 monitor 源；macOS 需装虚拟声卡（见下文）
- 双路混音（各自独立增益、软限幅），两路电平表实时显示
- 可选把原始音频录成 16kHz WAV，便于回听

**其它**
- 音频文件导入转写（WAV 原生支持；mp3/m4a/flac 等走系统 ffmpeg）
- 会话历史、重命名、搜索、删除；导出 Markdown / TXT / SRT / JSON
- 命令行自检 `--check`，一键回答「为什么我这边跑不起来」

---

## 二、快速开始

### 1. 系统依赖

**Windows**：装 [Rust](https://rustup.rs) + [Node 20+](https://nodejs.org) + Visual Studio C++ 生成工具（Tauri 需要）。
**macOS**：`xcode-select --install`，再装 Rust 与 Node。
**Linux（Ubuntu/Debian）**：
```bash
sudo apt update
sudo apt install -y build-essential pkg-config libssl-dev \
  libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev \
  libasound2-dev libxdo-dev patchelf
# 系统内录需要 PulseAudio/PipeWire 的命令行工具
sudo apt install -y pulseaudio-utils
```

> 不需要 CMake、不需要 CUDA Toolkit、不需要下载模型 —— 因为应用里没有识别模型。

### 2. 安装依赖并运行

```bash
pnpm install
pnpm tauri:dev          # 开发模式（热重载）
pnpm tauri:build        # 打包安装包
```

### 3. 配置语音识别服务

**设置 → 语音识别** → 选服务商 → 填 API Key → 「测试连接」。

| 服务商 | Base URL | 路径 | 模型 | 备注 |
| --- | --- | --- | --- | --- |
| **Groq** | `https://api.groq.com/openai/v1` | `/audio/transcriptions` | `whisper-large-v3-turbo` | 速度极快、便宜，**首选** |
| OpenAI | `https://api.openai.com/v1` | `/audio/transcriptions` | `whisper-1` | 需要能访问 openai.com |
| 硅基流动 | `https://api.siliconflow.cn/v1` | `/audio/transcriptions` | `FunAudioLLM/SenseVoiceSmall` | 国内直连，有免费额度 |
| **本地 whisper.cpp server** | `http://localhost:8080` | `/inference` | `whisper-1` | **完全离线** |
| 本地 faster-whisper-server | `http://localhost:8000/v1` | `/audio/transcriptions` | `Systran/faster-whisper-large-v3` | **完全离线** |
| 自定义 | 任意 | 任意 | 任意 | 自建网关 |

> 本地服务会自动**绕过系统 HTTP 代理**（企业网络里很常见），无需额外配置。

**想完全离线 / 想用 GPU？在你自己的机器上起一个本地识别服务即可：**

```bash
# 方案 A：whisper.cpp server（C++，CPU 或 CUDA/Metal 都支持）
git clone --depth 1 https://github.com/ggml-org/whisper.cpp && cd whisper.cpp
cmake -B build -DWHISPER_BUILD_SERVER=ON -DGGML_CUDA=ON   # 不用 GPU 就去掉 -DGGML_CUDA=ON
cmake --build build -j
./models/download-ggml-model.sh large-v3-turbo
./build/bin/whisper-server -m models/ggml-large-v3-turbo.bin --port 8080

# 方案 B：faster-whisper-server（Python，GPU 支持好）
docker run -p 8000:8000 -v ~/.cache/huggingface:/root/.cache/huggingface \
  fedirz/faster-whisper-server:latest-cuda
```

**GPU 加速在服务端**：客户端只发 HTTP 请求，所以「用不用显卡」由你部署的识别服务决定，应用本身不需要任何 GPU 相关的构建开关。

### 4. 配置 AI 接口

**设置 → AI 接口** → 选服务商 → 填 API Key → 「测试连接」。

| 服务商 | Base URL | 推荐模型 |
| --- | --- | --- |
| DeepSeek | `https://api.deepseek.com/v1` | `deepseek-chat` |
| OpenAI | `https://api.openai.com/v1` | `gpt-4o-mini` |
| 阿里通义千问 | `https://dashscope.aliyuncs.com/compatible-mode/v1` | `qwen-plus` |
| 智谱 GLM | `https://open.bigmodel.cn/api/paas/v4` | `glm-4-flash` |
| 月之暗面 Kimi | `https://api.moonshot.cn/v1` | `moonshot-v1-8k` |
| 本地 Ollama | `http://localhost:11434/v1` | `qwen2.5:7b` |

### 5. 开始录音

底部勾选音频来源 → 「开始录音」。出问题先跑自检：

```bash
meeting-hear --check          # 人类可读
meeting-hear --check --json   # 机器可读
meeting-hear --check --ai     # 额外做识别服务与 AI 的连通性测试
```

```
MeetingHear 自检
────────────────────────────────────────────────────────────
· 版本                   MeetingHear 0.1.0 · linux · x86_64
✓ 数据目录                 /root/.local/share/com.meetinghear.desktop
✓ 语音识别服务              Groq（推荐） · whisper-large-v3-turbo · https://api.groq.com/...
✓ 断句方式                 自适应能量 VAD（无需模型文件）
✓ 麦克风                  MacBook Pro 麦克风
✓ 系统内录                 系统声音（RDPSink）
✓ AI 接口                 DeepSeek · deepseek-chat
✓ 识别连通性                正常（612 ms）
✓ AI 连通性                正常（842 ms，返回：正常）
────────────────────────────────────────────────────────────
关键项检查通过，可以开始录音。
```

---

## 三、核心原理

这个产品最难的不是「调通接口」，而是让它在**真实会议的时间尺度**上既快又稳。

### 1. 按句定稿：把非流式接口用出流式效果

识别服务只接受「一段完整音频」，所以本地先做 VAD 断句，每句话一个请求：

```
音频流 ──▶ VAD 滚动窗口检测 ──▶ 判定「一句话」的起止
                                    │
              说话过程中每 800ms ──┼──▶ 增量识别当前句 ──▶ 灰色未确认文本（可选）
                                    │
              句子结束（静音 ≥600ms）┴──▶ 整句送识别服务 ──▶ 定稿
```

- **前滚留白** 400ms：避免吃掉句首辅音
- **最长句 25s**：超长独白会在能量最低处强制断句，保证延迟与单次上传体积有上界
- **自适应噪声底**：说话期间冻结噪声底、句间重新估计，因此连续说话不会被误判为静音
- 整句送识别而不是拼接流式碎片，**定稿质量等于整句识别**

### 2. 请求合并：防止字幕越来越滞后

一次 HTTP 往返要几百毫秒，如果每个增量请求都排队执行，积压会滚雪球。所以识别线程入口有一个合并队列：

- 同一句话的旧增量请求被新请求**直接顶掉**
- 某句话一旦定稿，它遗留的增量请求**全部作废**
- 正在飞的过期请求返回后结果被丢弃，**不会白花一次调用的结果显示到界面上**

### 3. 滚动增量纪要：上下文恒定

```
已有纪要 + 本次新增转写 ──▶ 一次 LLM 调用 ──▶ 更新后的完整纪要（JSON）
                     ▲                              │
                     └──────────────────────────────┘
```

每轮只送「已有纪要 + 新增片段 + 最近 90 秒原文」，因此：

- 上下文长度恒定，长会议的成本与延迟不随时间增长
- 模型返回的 JSON 做了三层容错（去代码块包裹 / 下钻嵌套对象 / 字段别名与类型兜底）
- 解析失败**不推进已覆盖位置**，下一轮自动重试，不会丢内容

---

## 四、系统内录（各平台差异很大）

| 平台 | 方案 | 需要额外安装吗 |
| --- | --- | --- |
| **Windows** | WASAPI loopback（在渲染设备上以 Capture 方向打开） | 不需要，开箱可用 |
| **Linux** | PulseAudio / PipeWire 的 monitor 源（`parec`） | 需要 `pulseaudio-utils` |
| **macOS** | 系统层面**不允许**应用直接录系统声音，必须用虚拟声卡 | **需要 BlackHole** |

**macOS 配置步骤**（一次性，5 分钟）：
1. `brew install blackhole-2ch`
2. 打开「音频 MIDI 设置」→ 左下角 `+` → 创建**多输出设备** → 勾选你的扬声器和 BlackHole 2ch
3. 把系统输出切到这个多输出设备
4. 回到 MeetingHear → 设置 → 音频设备 → 刷新 → 选择 BlackHole

应用内也会显示同样的引导文案，不会让用户对着「没有设备」发呆。

---

## 五、持续集成与发布

`.github/workflows/ci.yml` 已配置好，推送到 GitHub 后自动运行：

| Job | 内容 | 平台 |
| --- | --- | --- |
| `test` | 前端类型检查 + 构建 + Rust 单元测试 + **前后端契约一致性测试** + `--all-targets` 编译 | **三平台矩阵**（Ubuntu / Windows / macOS） |
| `ui-smoke` | Playwright 界面冒烟测试（30+ 项断言），失败也保留截图 | Linux |
| `build` | `tauri build` 打包并上传安装包（.deb/.rpm/.AppImage、.msi/.exe、.dmg/.app） | **三平台矩阵** |
| `release` | 推 `v*` tag 时自动创建 Release 并附上三平台安装包 | Linux |

**缓存策略**（这是让二次构建从 5~8 分钟降到 1 分钟内的关键）：
- `Swatinem/rust-cache` 缓存 `~/.cargo` 与 `src-tauri/target`，按平台 + profile 分别建 key（test 用 dev profile、build 用 release profile，互不覆盖）
- `actions/setup-node` 的 `cache: pnpm` 缓存 pnpm store
- `actions/cache` 缓存 Playwright 浏览器

**契约一致性测试**（`src-tauri/tests/contract.rs`）会从源码里抽取前后端的事实并对比：
命令名集合、事件名集合、参数命名风格。改了后端命令名却忘了改前端，CI 会直接拦住。

发布新版本：
```bash
git tag v0.1.0 && git push origin v0.1.0
```

---

## 六、开发与测试

```bash
pnpm typecheck        # 前端类型检查
pnpm build            # 前端构建
pnpm test:rust        # Rust 单元测试（180+ 项，无需服务/网络）
pnpm test:ui          # 浏览器端 UI 冒烟测试（Playwright，30+ 项断言）
pnpm check            # typecheck + test:rust

# 端到端测试：需要一个真实识别服务（最方便是本地 whisper.cpp server）
mkdir -p .e2e/audio
curl -L -o .e2e/audio/jfk.wav \
  https://raw.githubusercontent.com/ggml-org/whisper.cpp/master/samples/jfk.wav
./whisper-server -m models/ggml-tiny.bin --port 8090 &   # 见上文「快速开始」
pnpm test:e2e         # 8 条端到端测试（真实识别服务 + 真实 LLM + 真实采集）
```

端到端测试覆盖：完整流水线跑真实音频、增量预览、识别客户端编码/解析、连通性失败提示、
真实麦克风采集、Linux 系统内录采集、真实 LLM 连通性与滚动纪要。

### 项目结构

```
meeting-hear/
├── .github/workflows/ci.yml    三平台 CI + 缓存 + 发布
├── src/                        前端（React 19 + TypeScript）
│   ├── lib/contract.ts         冻结的前后端契约（类型 + 事件名）
│   ├── lib/api.ts              后端调用封装（浏览器下自动走 mock）
│   ├── lib/mock.ts             浏览器 mock 后端（可脱离 Rust 开发/测试 UI）
│   ├── store.ts                状态层（高频电平事件独立 store，避免整树重渲染）
│   └── components/             界面组件
├── src-tauri/
│   ├── src/
│   │   ├── audio/              采集、重采样、VAD、混音、录音、解码
│   │   │   ├── resample.rs     自研多相 FIR 重采样（含混叠抑制测试）
│   │   │   ├── vad.rs          自适应能量 VAD（无需模型）
│   │   │   ├── capture.rs      设备枚举、采集线程、混音
│   │   │   ├── loopback.rs     三平台系统内录
│   │   │   └── wav.rs          录音落盘 + 上传用内存 WAV 编码
│   │   ├── asr/                识别服务客户端
│   │   │   ├── provider.rs     服务商预设
│   │   │   ├── client.rs       OpenAI 兼容 / whisper.cpp server 双形态 HTTP 客户端
│   │   │   ├── hypothesis.rs   LocalAgreement 流式文本合并（增量预览用）
│   │   │   └── filter.rs       幻听与退化输出过滤
│   │   ├── ai/                 OpenAI 兼容客户端 + 提示词 + 滚动纪要状态机
│   │   ├── pipeline.rs         断句 → 识别 → 纪要 的实时流水线
│   │   ├── session/            会话模型、持久化、导出
│   │   ├── commands.rs         全部 Tauri 命令
│   │   └── diagnostics.rs      `--check` 自检
│   └── tests/                  契约测试 + 端到端测试
└── scripts/ui-smoke.mjs        Playwright UI 冒烟测试
```

---

## 七、数据与隐私

> ⚠️ **与「全本地识别」的版本不同：识别走外部服务时，音频会被发送到你配置的识别服务。**

- **发出去的是什么**：每句话的音频（16kHz 单声道 WAV，一句话通常几百 KB）
- **发给谁**：由你在「设置 → 语音识别」里配置的服务商决定
- **想完全不外传**：把识别服务指向本机（whisper.cpp server / faster-whisper-server），
  同时把 AI 接口指向本地 Ollama —— 此时全部数据都在本机，且功能完全一样
- **AI 纪要**：只发送文本（已有纪要 + 新增转写），不发音频
- **API Key 存储**：默认保存在应用数据目录的 `settings.json`（Unix 下权限 0600）。
  不想落盘可在「设置 → 通用」关闭「保存 API Key」，此时只在内存中保留
- **录音文件**：可选，只写本地磁盘，不会上传
- **数据目录**：`%APPDATA%\com.meetinghear.desktop`（Windows）/ `~/Library/Application Support/com.meetinghear.desktop`（macOS）/ `~/.local/share/com.meetinghear.desktop`（Linux）

---

## 八、已知限制

- **识别质量取决于你选的服务与模型**：`whisper-large-v3-turbo` 明显好于 `tiny`；本地部署时建议 `large-v3-turbo` 起步
- **按句调用对短句不友好**：一句话单独送识别会丢失跨句上下文，实测比「整段音频一次识别」略差。应用会把上一句作为 `prompt` 传过去以缓解，但仍不如一次送整段
- **「边说边出字」是可选功能**：每次增量都是一次 API 调用，云端服务成本会明显上升；本地服务建议开启
- **断句用的是自适应能量 VAD**：对连续无停顿的长独白不如 Silero 之类的神经网络 VAD，但不需要任何模型文件，且已针对「说话中冻结噪声底」做了处理
- **macOS 的系统内录必须依赖 BlackHole**：这是系统限制，不是实现偷懒
- **说话人分离是「我/对方」二分**：基于双路能量占比，不是真正的声纹聚类
- **AI 纪要质量取决于模型**：1~2B 的小参数本地模型能跑通链路，但会编造内容
- **未做**：会议录音回放、多语言实时翻译（`translate_to_english` 已接到接口，UI 未暴露）
- **平台验证范围**：Linux（WSL2）上完成了全部编译、单元测试与端到端验证（含真实识别服务与真实 LLM）；
  Windows/macOS 的代码路径已按各自 API 实现并有 CI 编译保障，但未在对应真机上运行验证

---

## 九、常见问题

**Q：点了开始录音没反应 / 提示未配置识别服务？**
设置 → 语音识别 → 选一个服务商填好 Base URL、模型名与 API Key，点「测试连接」确认通过。

**Q：没有声音 / 电平条不动？**
先跑 `meeting-hear --check` 看音频设备是否被识别。容器、服务器、远程桌面里通常没有音频设备。

**Q：识别不准？**
1. 换更强的服务/模型（Groq 的 `whisper-large-v3-turbo` 性价比很高）
2. 在「语音识别」页把语言从「自动检测」改成「中文」
3. 打开「上下文提示」
4. 检查麦克风增益是否过低

**Q：字幕滞后？**
看底部的 RTF：大于 1.0 说明识别服务跟不上。换更快的服务（Groq），或改用本地 GPU 部署。

**Q：本地识别服务连不上？**
1. 确认服务已启动：`curl http://localhost:8080/` 有响应
2. 路径别填错：whisper.cpp server 是 `/inference`，faster-whisper-server 是 `/audio/transcriptions`
3. 程序已经对 `localhost` 自动绕过 HTTP 代理，若仍失败请检查代理软件的分流规则

**Q：AI 一直不总结？**
1. 设置 → AI 接口 → 「测试连接」看是否通
2. 检查是否开了「自动总结」，间隔与最小新增字数是否设得过大
3. 手动点「立即总结」可强制触发一次

**Q：纪要里出现「模型没有按要求返回 JSON」？**
说明该模型指令遵循能力不足。改用更强的模型，或换支持 `response_format: json_object` 的服务商；
错误信息里会附上模型的原始输出，便于判断。

---

## 十、技术栈

| 层 | 选型 |
| --- | --- |
| 桌面框架 | Tauri 2 |
| 界面 | React 19 · TypeScript · Vite 7 · zustand（无 UI 组件库） |
| 识别 | 任意 OpenAI 兼容 `/audio/transcriptions` 或 whisper.cpp `/inference` 服务 |
| 纪要 | 任意 OpenAI 兼容 `/chat/completions` 服务 |
| 音频 | cpal（麦克风）· WASAPI / PulseAudio·PipeWire / BlackHole（内录） |
| 重采样 / VAD / 混音 | 自研（见 `src-tauri/src/audio/`） |
| CI | GitHub Actions 三平台矩阵 + rust-cache / pnpm cache / Playwright cache |

## 十一、许可

MIT
