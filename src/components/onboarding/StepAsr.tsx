/** 向导第 2 步：语音识别服务（决定音频发给谁） */
import { useEffect, useState } from "react";
import { api } from "../../lib/api";
import type { AsrConnectionTestResult, AsrPreset, AsrProvider } from "../../lib/contract";
import { WHISPER_CPP_CMD, consoleLinks } from "../../lib/links";
import { copyText } from "../../lib/util";
import { guard, useStore } from "../../store";
import { Button, Field, Select, TextInput } from "../ui";
import { LinkButtons, StepHead, TestBox, type StepProps } from "./parts";

function providerFromPreset(preset: AsrPreset): AsrProvider {
  return {
    id: preset.id,
    name: preset.name,
    baseUrl: preset.baseUrl,
    transcriptionPath: preset.transcriptionPath,
    apiKey: "",
    model: preset.model,
    responseFormat: preset.responseFormat,
    timeoutSecs: 120,
    extraHeaders: [],
  };
}

/** 是否是 whisper.cpp server 这类本地服务（要额外提示启动命令与端口一致） */
function isWhisperCpp(p: AsrProvider | null): boolean {
  if (!p) return false;
  return /whisper\.cpp|whispercpp/i.test(`${p.id} ${p.name} ${p.baseUrl}`);
}

/** 与服务端保持一致：whisper.cpp server 的 /inference 不校验模型名（见 settings.rs） */
function modelRequired(p: AsrProvider | null): boolean {
  if (!p) return false;
  return !p.transcriptionPath.trim().replace(/\/$/, "").endsWith("/inference");
}

export function StepAsr({ draft, set }: StepProps) {
  const toast = useStore((s) => s.toast);
  const asr = draft.asr;

  const [presets, setPresets] = useState<AsrPreset[]>([]);
  const [presetId, setPresetId] = useState("");
  const [selectedId, setSelectedId] = useState(asr.activeProviderId || asr.providers[0]?.id || "");
  const [testing, setTesting] = useState(false);
  const [result, setResult] = useState<AsrConnectionTestResult | null>(null);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    void guard("读取识别服务预设", () => api.listAsrPresets()).then((list) => {
      if (list && list.length) {
        setPresets(list);
        setPresetId(list[0].id);
      }
    });
  }, []);

  const provider: AsrProvider | null =
    asr.providers.find((p) => p.id === selectedId) ?? asr.providers[0] ?? null;

  const patchAsr = (patch: Partial<typeof asr>) => set("asr", { ...asr, ...patch });
  const patchProvider = (patch: Partial<AsrProvider>) => {
    if (!provider) return;
    patchAsr({ providers: asr.providers.map((p) => (p.id === provider.id ? { ...p, ...patch } : p)) });
  };

  /** 选中预设：已有的服务商就切过去，否则按预设新建一份并设为当前使用 */
  const choosePreset = (id: string) => {
    setPresetId(id);
    setResult(null);
    const preset = presets.find((p) => p.id === id);
    if (!preset) return;
    const existing = asr.providers.find((p) => p.id === preset.id);
    if (existing) {
      setSelectedId(existing.id);
      patchAsr({ activeProviderId: existing.id });
      return;
    }
    const created = providerFromPreset(preset);
    patchAsr({
      providers: [...asr.providers, created],
      activeProviderId: created.id,
    });
    setSelectedId(created.id);
  };

  const testConnection = async () => {
    if (!provider) return;
    setTesting(true);
    setResult(null);
    const r = await guard("测试识别服务连接", () => api.testAsrConnection(provider));
    setTesting(false);
    if (!r) return;
    setResult(r);
    toast(r.ok ? "success" : "error", "测试连接", r.ok ? `连接正常，延迟 ${r.latencyMs} ms` : (r.error ?? "连接失败"));
  };

  const local = isWhisperCpp(provider);

  return (
    <div className="ob-body-inner">
      <StepHead
        title="语音识别服务"
        desc="音频会按句上传到这里。云端服务需要 API Key；本地服务 Key 留空。"
      />

      <Field label="服务商预设" hint="选一个会自动填好下面的字段，之后可以改">
        <Select
          value={presetId}
          options={presets.map((p) => ({ value: p.id, label: `${p.name} — ${p.note}` }))}
          onChange={choosePreset}
          ariaLabel="识别服务预设"
          testId="onboarding-asr-preset"
        />
      </Field>

      <Field
        label="Base URL"
        hint={
          local
            ? "本地服务填 http://localhost:端口 —— 但注意：localhost 指的是这台电脑自己。服务跑在另一台机器上时，要填那台机器在这边能访问到的地址（例如 Tailscale IP），否则连不上"
            : "服务根地址，云端通常以 /v1 结尾"
        }
      >
        <TextInput
          value={provider?.baseUrl ?? ""}
          onChange={(v) => patchProvider({ baseUrl: v })}
          testId="onboarding-asr-baseurl"
          ariaLabel="识别服务 Base URL"
          mono
          placeholder="https://api.groq.com/openai/v1"
        />
      </Field>
      <Field label="接口路径" hint="OpenAI 兼容填 /audio/transcriptions，whisper.cpp server 填 /inference">
        <TextInput
          value={provider?.transcriptionPath ?? ""}
          onChange={(v) => patchProvider({ transcriptionPath: v })}
          testId="onboarding-asr-path"
          ariaLabel="识别接口路径"
          mono
        />
      </Field>
      <Field label="API Key" hint="本地/自建服务可以留空">
        <TextInput
          value={provider?.apiKey ?? ""}
          onChange={(v) => patchProvider({ apiKey: v })}
          testId="onboarding-asr-apikey"
          ariaLabel="识别服务 API Key"
          type="password"
          mono
          placeholder="gsk_…"
        />
      </Field>
      <Field
        label="模型名"
        hint={
          modelRequired(provider)
            ? "必填：Groq 用 whisper-large-v3-turbo，OpenAI 用 whisper-1"
            : "可留空：whisper.cpp server 不校验模型名，它只认启动时 -m 指定的模型"
        }
      >
        <TextInput
          value={provider?.model ?? ""}
          onChange={(v) => patchProvider({ model: v })}
          testId="onboarding-asr-model"
          ariaLabel="识别模型名"
          mono
        />
      </Field>

      {local ? (
        <div className="ob-tip" data-testid="onboarding-asr-local-hint">
          <strong>本机跑 whisper.cpp server</strong>
          <span className="small">
            先编译 whisper.cpp，然后（在 whisper.cpp 目录下）启动服务。<b>命令里的端口要和上面的 Base URL 一致</b>，
            例如命令用 <code className="mono">--port 8080</code>，Base URL 就要是{" "}
            <code className="mono">http://localhost:8080</code>。
          </span>
          <div className="cmd-row">
            <code className="notice-code grow">{WHISPER_CPP_CMD}</code>
            <Button
              variant="ghost"
              ariaLabel="复制命令"
              onClick={() => {
                void copyText(WHISPER_CPP_CMD).then((ok) => {
                  setCopied(ok);
                  if (ok) window.setTimeout(() => setCopied(false), 2500);
                });
              }}
            >
              {copied ? "已复制" : "复制"}
            </Button>
          </div>
          <span className="dim tiny">
            命令里的 <code>--language auto</code> 不能省：whisper-server 默认按英文识别，中文会变成英文乱码。
            识别语言由服务端决定，应用里没有、也不需要有语言设置。
          </span>
          <span className="dim tiny">
            想要更省事也可以用 faster-whisper-server，它是 OpenAI 兼容接口，Base URL 填 http://localhost:8000/v1。
          </span>
        </div>
      ) : null}

      <div className="row">
        <Button variant="primary" onClick={() => void testConnection()} disabled={testing} testId="onboarding-asr-test">
          {testing ? "测试中…" : "⚡ 测试连接"}
        </Button>
        <span className="dim tiny">
          测试会发送 0.6 秒静音，只验证「地址 + 鉴权 + 模型名」是否可用；<b>返回文本为空是正常的</b>。
        </span>
      </div>

      {result ? (
        <TestBox
          result={result}
          testId="onboarding-asr-test-result"
          emptySampleNote="返回文本为空是正常的 —— 服务收到了请求，只是这段静音里没有语音。"
        />
      ) : null}

      <LinkButtons links={consoleLinks(provider?.id, provider?.name, provider?.baseUrl)} testId="onboarding-asr-open-console" />

      <p className="dim tiny">
        测试没通过也可以先跳过：先把界面跑起来，之后在「设置 → 语音识别」里补齐即可（没有可用的识别服务时无法开始录音）。
      </p>
    </div>
  );
}
