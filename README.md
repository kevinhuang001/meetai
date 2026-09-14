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

**界面**
- **首次运行向导**：5 步引导配置识别服务与 AI 接口，每一步都能当场「测试连接」；
  没配好这两件事应用其实什么都做不了，所以向导是默认路径（设置 → 通用可重新打开）
- **左侧历史 sidebar**：常驻展示历史会议（标题/时间/时长/段数/是否已出纪要），
  支持搜索、重命名、删除、点击回看；可折叠，窄窗口自动收起
- 主界面三栏：左历史 ｜ 中转写 ｜ 右 AI 纪要

**其它**
- 音频文件导入转写（WAV 原生支持；mp3/m4a/flac 等走系统 ffmpeg）
- 导出 Markdown / TXT / SRT / JSON
- 命令行自检 `--check`，一键回答「为什么我这边跑不起来」

---

## 二、直接下载安装包

GitHub Releases 上有三平台安装包（由 CI 自动构建）：

**https://github.com/kevinhuang001/meetai/releases/latest**

| 平台 | 文件 | 说明 |
| --- | --- | --- |
| Windows x64 | `MeetingHear_x.y.z_x64-setup.exe` | NSIS 安装包，自动补 WebView2 |
| Windows x64 | `MeetingHear_x.y.z_x64_en-US.msi` | MSI 安装包 |
| macOS（Apple Silicon） | `MeetingHear_x.y.z_aarch64.dmg` | 拖进 Applications 即可 |
| macOS（Apple Silicon） | `MeetingHear_x.y.z_aarch64.app.zip` | 不想挂载 dmg 时用这个 |
| Linux x64 | `MeetingHear_x.y.z_amd64.deb` | Debian/Ubuntu，依赖已声明会自动装 |
| Linux x64 | `MeetingHear_x.y.z_amd64.AppImage` | `chmod +x` 后直接运行 |

> 装好后第一次启动会进入配置向导，按提示填识别服务与 AI 接口即可开始用。

---

## 三、本地跑一套完整环境（推荐先这样验证）

不花一分钱、数据不出本机：**本地 whisper.cpp server 做识别 + 本地 Ollama 做纪要**。

```bash
# ① 识别服务：whisper.cpp server
git clone --depth 1 https://github.com/ggml-org/whisper.cpp && cd whisper.cpp
cmake -B build -DWHISPER_BUILD_SERVER=ON            # 有 N 卡加 -DGGML_CUDA=ON
cmake --build build -j --target whisper-server
./models/download-ggml-model.sh base                 # 中文建议 base 起步，追求质量用 large-v3-turbo
./build/bin/whisper-server -m models/ggml-base.bin --host 127.0.0.1 --port 8080 --language auto

#                                             ↑ 这一项别省：whisper-server 默认按英文识别，
#                                               中文语音会被转成英文乱码（详见「识别语言归谁管」）
# ② 纪要服务：Ollama
ollama pull qwen2.5:7b
ollama serve                                          # 默认就在 127.0.0.1:11434
```

然后在应用里这样填：

| 设置项 | 值 |
| --- | --- |
| 语音识别 → 服务商 | **本地 whisper.cpp server** |
| 语音识别 → Base URL | `http://127.0.0.1:8080`（端口要和你启动时的一致） |
| 语音识别 → 接口路径 | `/inference` |
| 语音识别 → 模型名 | `whisper-1`（whisper.cpp server 不校验这个名字，随便填一个非空的即可） |
| 语音识别 → API Key | 留空 |
| AI 接口 → Base URL | `http://127.0.0.1:11434/v1` |
| AI 接口 → 模型名 | 你 `ollama pull` 下来的模型，例如 `qwen2.5:7b` |
| AI 接口 → API Key | 留空 |

两个「测试连接」都应该立刻通过。**本地/局域网/Tailscale 地址都会自动绕过系统代理**
（覆盖回环、`10/8`、`172.16/12`、`192.168/16`、`169.254/16`、**Tailscale 的 `100.64.0.0/10`**、
`*.local`、`*.ts.net`），不需要额外配置 `NO_PROXY`。

`scripts/dev-services.sh` 可以一键启停这两个服务：

```bash
./scripts/dev-services.sh start     # 启动并做连通性检查，同时打印应用里该怎么填
./scripts/dev-services.sh status
./scripts/dev-services.sh stop
BIND_HOST=127.0.0.1 ./scripts/dev-services.sh start   # 只想本机用（默认监听 0.0.0.0）
```

---

## 四、远程接入（从另一台机器 / Mac 连过来）

服务默认监听 `0.0.0.0`，所以只要网络能通，从别的机器直接填本机地址即可。

**Windows + WSL2 用户注意**：WSL2 默认是 NAT 网络，Linux 侧的 `172.x.x.x` 地址
**局域网里的机器连不到**。要打通有两条路：

**方案 A：在 Windows 宿主机做端口转发（需要管理员权限）**

```powershell
# 把 WSL 的 IP 填到 connectaddress（在 WSL 里执行 hostname -I 获取）
netsh interface portproxy add v4tov4 listenaddress=0.0.0.0 listenport=8090 connectaddress=<WSL_IP> connectport=8090
netsh interface portproxy add v4tov4 listenaddress=0.0.0.0 listenport=11434 connectaddress=<WSL_IP> connectport=11434
netsh advfirewall firewall add rule name="MeetingHear ASR" dir=in action=allow protocol=TCP localport=8090
netsh advfirewall firewall add rule name="MeetingHear Ollama" dir=in action=allow protocol=TCP localport=11434
```

之后从 Mac 填 **宿主机** 的地址，例如走 Tailscale 就是 `http://100.x.y.z:8090`。
WSL 的 IP 重启后会变，届时重新执行一遍即可（或用方案 B）。

**方案 B：让 WSL 共享宿主机网络（一劳永逸）**

在 Windows 用户目录新建 `%UserProfile%\.wslconfig`：

```ini
[wsl2]
networkingMode=mirrored
```

然后 `wsl --shutdown` 重启 WSL。之后 WSL 里的服务直接监听在宿主机地址上，
Tailscale / 局域网都能直连，不需要端口转发。

**安全提醒**：`whisper-server` 与 `ollama` 都**没有鉴权**。把端口暴露到公网等于把
模型与算力开放给任何人，请只在可信网络（家庭/办公局域网、Tailscale 私有网络）里这样做，
用完可以 `BIND_HOST=127.0.0.1 ./scripts/dev-services.sh start` 收回本机。

---

## 五、快速开始（从源码构建）

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
./build/bin/whisper-server -m models/ggml-large-v3-turbo.bin --port 8080 --language auto

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

## 六、核心原理

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

### 4. 识别语言归服务端管，应用不碰

应用**没有**「识别语言」设置项，请求里也**不发** `language` 字段。原因是一次真实事故：

- whisper.cpp server 的 `language` 默认值是 `en`；
- 早先版本把「语言」做成了应用自己的设置项，默认「自动」时干脆不发该字段；
- 于是中文语音被按英文硬识别，模型吐出一段英文幻觉（`Welcome to the video...`）；
- 更糟的是这段幻觉随后被「幻听过滤器」丢弃，**界面上一个字都没有** ——
  用户完全无法判断是程序坏了、麦克风没声音，还是服务配错了。

现在两条约定：

1. **语言由识别服务配置**。whisper.cpp server 加 `--language auto`；OpenAI / Groq 等
   云端接口本身就自动检测，不用配。
2. **识别结果只标注、不丢弃**。可疑段（幻听 / 退化重复 / 服务没返回内容）照常显示在
   转写里，只是标灰加标记，并且不参与纪要生成 —— 幻觉文本足以把整份纪要带偏。

「服务在工作但没有结果」和「程序卡死」必须在界面上能区分开，这是这条规则的全部意义。

## 七、系统内录与平台依赖

### 「依赖都带好」到底能带到什么程度

| 平台 | 安装包里已带 | 必须用户自己装 | 原因 |
| --- | --- | --- | --- |
| **Windows** | 应用本体 + WebView2 自动补装 | 无 | WASAPI loopback 是系统自带能力，开箱可用 |
| **Linux** | 应用本体；`.deb` 里已声明 `libwebkit2gtk-4.1-0`、`libgtk-3-0`、`libasound2`、**`pulseaudio-utils`**（提供 `parec`/`pactl`，系统内录靠它） | 无（AppImage 用户需自行保证系统库存在） | monitor 源是 PulseAudio/PipeWire 的能力 |
| **macOS** | 应用本体（`.dmg` / `.app.zip`） | **BlackHole**（免费虚拟声卡） | 见下 |

**为什么 macOS 的虚拟声卡不能内置？**
macOS 从系统层面就不允许普通应用直接录制扬声器输出。要拿到系统声音，必须有一个**系统级音频驱动**（BlackHole / Soundflower 这类）参与音频路由，它需要以管理员权限安装、并涉及音频驱动的签名与公证 —— 这不是「应用里多放一个文件」能做到的事，任何应用都无法把自己的音频驱动悄悄塞进系统。

所以应用的做法是**检测 + 手把手引导**（首次向导第 4 步与「设置 → 音频设备」都有这张卡片）：

1. 一键复制 `brew install blackhole-2ch`
2. 一键打开 BlackHole 下载页（`api.openUrl`）
3. 给出「音频 MIDI 设置 → 创建多输出设备 → 同时勾选扬声器和 BlackHole → 设为系统输出」的三步图示说明
4. 装完点「重新检测设备」，检测到就直接可用，不用重启应用

装上之后功能与其它平台完全一致（包括「我 / 对方」的说话人归属）。

> 如果不想装任何驱动：macOS 上也可以只用麦克风录音 —— 开会时把手机/另一台设备开免提，或者直接对麦克风说话，功能不受影响，只是录不到对方系统的原始音频。

### 各平台内录实现

| 平台 | 方案 | 需要额外安装吗 |
| --- | --- | --- |
| **Windows** | WASAPI loopback（在渲染设备上以 Capture 方向打开） | 不需要，开箱可用 |
| **Linux** | PulseAudio / PipeWire 的 monitor 源（`parec`） | `.deb` 已带 `pulseaudio-utils`；AppImage 用户 `sudo apt install pulseaudio-utils` |
| **macOS** | 虚拟声卡输入设备（BlackHole） | **需要**，见上 |

**macOS 配置步骤**（一次性，5 分钟）：
1. `brew install blackhole-2ch`
2. 打开「音频 MIDI 设置」→ 左下角 `+` → 创建**多输出设备** → 勾选你的扬声器和 BlackHole 2ch
3. 把系统输出切到这个多输出设备
4. 回到 MeetingHear → 设置 → 音频设备 → 刷新 → 选择 BlackHole

## 八、持续集成与发布

`.github/workflows/ci.yml` 已配置好，推送到 GitHub 后自动运行：

| Job | 内容 | 平台 |
| --- | --- | --- |
| `test` | 前端类型检查 + 构建 + Rust 单元测试 + **前后端契约一致性测试** + `--all-targets` 编译 | **三平台矩阵**（Ubuntu / Windows / macOS） |
| `ui-smoke` | Playwright 界面冒烟测试（30+ 项断言），失败也保留截图 | Linux |
| `build` | `tauri build` 打包并上传安装包（.deb/.rpm/.AppImage、.msi/.exe、.dmg/.app） | **三平台矩阵** |
| `release` | 推 `v*` tag 时自动创建 Release 并附上三平台安装包 | Linux |

**缓存策略与实测效果**

三层缓存，各管一件事：

| 缓存 | 内容 | 谁在用 |
| --- | --- | --- |
| `Swatinem/rust-cache` | `~/.cargo`（含 registry 与 git）+ 本 profile 的 `src-tauri/target` | test（dev）与 build（release）各自一份，失败也保存（`cache-on-failure`） |
| `awalsh128/cache-apt-pkgs-action` | Linux 系统依赖（webkit2gtk 等 100MB+） | Linux 的 test 与 build |
| `actions/setup-node` + `actions/cache` | pnpm store、Playwright 浏览器 | 全部 job |

**实测（同一份代码，全流水线墙钟耗时）**：

| 运行 | 耗时 | 说明 |
| --- | --- | --- |
| 首次跑通三平台打包 | 21.6 min | 缓存全冷 |
| 加了缓存但**多了一层冗余** | 15.3 min | 我额外缓存了一次 cargo registry，而 rust-cache 本来就会缓存它 → 每个 job 白恢复几百 MB |
| 去掉冗余层 | **8.7 min** | 最终形态 |
| 带自动发布 | 8.9 min | 含建 Release 与上传三平台安装包 |

> 一个容易踩的坑：**不要给 rust-cache 已经覆盖的目录再加一层 `actions/cache`**。
> 每个 job 都在全新虚拟机上跑，磁盘不共享、只有 cache 跨 job 复用；
> 但重复的缓存层带来的「重复恢复」开销，比省下的「重复下载」还大。
> 真正无法避免的重来只有一件事：release profile 的编译（和测试用的 dev profile 是两份产物）。

**契约一致性测试**（`src-tauri/tests/contract.rs`）会从源码里抽取前后端的事实并对比：
命令名集合、事件名集合、参数命名风格。改了后端命令名却忘了改前端，CI 会直接拦住。

发布新版本：
```bash
git tag v0.1.0 && git push origin v0.1.0
```

---

## 九、开发与测试

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
./whisper-server -m models/ggml-tiny.bin --port 8090 --language auto &   # 见上文「快速开始」
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

## 十、数据与隐私

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

## 十一、已知限制

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

## 十二、常见问题

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

## 十三、技术栈

| 层 | 选型 |
| --- | --- |
| 桌面框架 | Tauri 2 |
| 界面 | React 19 · TypeScript · Vite 7 · zustand（无 UI 组件库） |
| 识别 | 任意 OpenAI 兼容 `/audio/transcriptions` 或 whisper.cpp `/inference` 服务 |
| 纪要 | 任意 OpenAI 兼容 `/chat/completions` 服务 |
| 音频 | cpal（麦克风）· WASAPI / PulseAudio·PipeWire / BlackHole（内录） |
| 重采样 / VAD / 混音 | 自研（见 `src-tauri/src/audio/`） |
| CI | GitHub Actions 三平台矩阵 + rust-cache / pnpm cache / Playwright cache |

## 十四、许可

MIT
