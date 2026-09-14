/** 设置 · 语音识别：调用外部识别服务（App 本身不含任何识别模型） */
import { useEffect, useState } from "react";
import { api } from "../../lib/api";
import {
  type AsrConnectionTestResult,
  type AsrPreset,
  type AsrProvider,
  type AsrSettings,
} from "../../lib/contract";
import { guard, useStore } from "../../store";
import { Badge, Button, Field, Select, Slider, SwitchRow } from "../ui";
import { AsrProviderEditor } from "./AsrProviderEditor";
import type { TabProps } from "./SettingsDialog";

function providerFromPreset(preset: AsrPreset, id = preset.id): AsrProvider {
  return {
    id,
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

export function AsrTab({ draft, set }: TabProps) {
  const toast = useStore((s) => s.toast);
  const asr: AsrSettings = draft.asr;

  const [presets, setPresets] = useState<AsrPreset[]>([]);
  const [presetId, setPresetId] = useState<string>("");
  const [selectedId, setSelectedId] = useState<string>(asr.activeProviderId || asr.providers[0]?.id || "");
  const [testing, setTesting] = useState(false);
  const [result, setResult] = useState<AsrConnectionTestResult | null>(null);

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

  const patchAsr = (patch: Partial<AsrSettings>) => set("asr", { ...asr, ...patch });
  const patchProvider = (patch: Partial<AsrProvider>) => {
    if (!provider) return;
    patchAsr({ providers: asr.providers.map((p) => (p.id === provider.id ? { ...p, ...patch } : p)) });
  };

  /** 选中预设：已存在该预设的服务商就切过去，否则按预设新建并自动填好各字段 */
  const choosePreset = (id: string) => {
    setPresetId(id);
    setResult(null);
    const preset = presets.find((p) => p.id === id);
    if (!preset) return;
    const existing = asr.providers.find((p) => p.id === preset.id);
    if (existing) {
      setSelectedId(existing.id);
      return;
    }
    const created = providerFromPreset(preset);
    patchAsr({
      providers: [...asr.providers, created],
      activeProviderId: asr.providers.length === 0 ? created.id : asr.activeProviderId,
    });
    setSelectedId(created.id);
  };

  /** 强制新增一个服务商（允许同一预设多份，例如两套自建网关） */
  const addProvider = () => {
    const preset = presets.find((p) => p.id === presetId) ?? presets[0];
    if (!preset) return;
    const used = new Set(asr.providers.map((p) => p.id));
    let id = preset.id;
    let n = 2;
    while (used.has(id)) {
      id = `${preset.id}-${n}`;
      n += 1;
    }
    const created = providerFromPreset(preset, id);
    patchAsr({
      providers: [...asr.providers, created],
      activeProviderId: asr.providers.length === 0 ? created.id : asr.activeProviderId,
    });
    setSelectedId(id);
    setResult(null);
  };

  const removeProvider = (id: string) => {
    const rest = asr.providers.filter((p) => p.id !== id);
    patchAsr({
      providers: rest,
      activeProviderId: asr.activeProviderId === id ? (rest[0]?.id ?? "") : asr.activeProviderId,
    });
    if (selectedId === id) setSelectedId(rest[0]?.id ?? "");
    setResult(null);
  };

  const testConnection = async () => {
    if (!provider) return;
    setTesting(true);
    setResult(null);
    const r = await guard("测试识别服务连接", () => api.testAsrConnection(provider));
    setTesting(false);
    if (!r) return;
    setResult(r);
    toast(
      r.ok ? "success" : "error",
      "测试连接",
      r.ok ? `连接正常，延迟 ${r.latencyMs} ms` : (r.error ?? "连接失败"),
    );
  };

  return (
    <div className="tab-body">
      {/* 说明压成一行 + 可展开详情：这些命令与首次向导第 2 步重复，
          常驻在设置页里只是噪音，需要的人点开就能看到。 */}
      <div className="warn-bar" data-testid="asr-service-notice">
        <strong>本应用不含识别模型，需要外部识别服务</strong>
        <span>转写通过 HTTP 调用你配置的服务商；没配好就无法开始录音。</span>
        <details className="notice-details">
          <summary>想完全离线？点开看自建服务的命令</summary>
          <code className="notice-code">
            whisper-server -m ggml-large-v3-turbo.bin --port 8080 --language auto
          </code>
          <span>
            <strong>务必带上 --language auto</strong>：whisper-server 的默认语言是英文，不加这个参数，
            中文语音会被按英文识别，只会得到一段英文乱码。识别语言归服务端管，应用不参与。
          </span>
          <span>重启应用后在「服务商」里选「本地 whisper.cpp server」，Base URL 填 http://127.0.0.1:8080，路径 /inference。</span>
          <code className="notice-code">docker run -p 8000:8000 fedirz/faster-whisper-server</code>
          <span>或任何 OpenAI 兼容的 /audio/transcriptions 服务，Base URL 填对应地址即可。</span>
        </details>
      </div>

      <section className="card">
        <h3>总开关</h3>
        <SwitchRow
          label="启用语音识别"
          hint="关闭后无法开始录音；音频采集与 AI 纪要不受影响"
          checked={asr.enabled}
          onChange={(v) => patchAsr({ enabled: v })}
          testId="asr-enabled"
        />
      </section>

      <section className="card">
        <h3>
          识别服务商 <Badge>{asr.providers.length}</Badge>
        </h3>
        <p className="dim small">音频会上传到选中的服务商；云端需要 API Key，本地服务留空。</p>

        <div className="provider-list" data-testid="asr-provider-list">
          {asr.providers.map((p) => (
            <button
              key={p.id}
              className={p.id === provider?.id ? "provider-item active" : "provider-item"}
              data-testid={`asr-provider-${p.id}`}
              onClick={() => {
                setSelectedId(p.id);
                setResult(null);
              }}
            >
              <span className="p-name">{p.name}</span>
              <span className="p-url mono">{p.baseUrl || "未填写 Base URL"}</span>
              {p.id === asr.activeProviderId ? <Badge tone="ok">当前使用</Badge> : null}
            </button>
          ))}
          {asr.providers.length === 0 ? (
            <p className="dim small">还没有识别服务商，从下面的预设里选一个即可。</p>
          ) : null}
        </div>

        <div className="row">
          <div className="grow">
            <Select
              value={presetId}
              options={presets.map((p) => ({ value: p.id, label: `${p.name} — ${p.note}` }))}
              onChange={choosePreset}
              ariaLabel="识别服务预设"
              testId="asr-preset-select"
            />
          </div>
          <Button variant="primary" onClick={addProvider} testId="add-asr-provider" ariaLabel="新增识别服务商">
            ＋ 新增服务商
          </Button>
        </div>

        {provider ? (
          <AsrProviderEditor
            provider={provider}
            active={asr.activeProviderId === provider.id}
            canRemove={asr.providers.length > 1}
            testing={testing}
            result={result}
            patchProvider={patchProvider}
            onSetActive={() => patchAsr({ activeProviderId: provider.id })}
            onRemove={() => removeProvider(provider.id)}
            onTest={() => void testConnection()}
          />
        ) : null}
      </section>

      <section className="card">
        <h3>实时策略</h3>
        <SwitchRow
          label="边说边出字"
          hint="开启后每 800ms 会对当前这句话重新识别一次，会显著增加 API 调用次数；云端服务注意费用，本地/自建服务建议开启"
          checked={asr.livePreview}
          onChange={(v) => patchAsr({ livePreview: v })}
          testId="asr-live-preview"
        />
        <Field label="单次上传时长上限" hint="超过该长度的句子会被强制断句后分别上传（5 ~ 120 秒）">
          <Slider
            value={asr.maxChunkSecs}
            min={5}
            max={120}
            step={5}
            onChange={(v) => patchAsr({ maxChunkSecs: v })}
            ariaLabel="单次上传时长上限"
            testId="asr-max-chunk"
            format={(v) => `${v} s`}
          />
        </Field>
      </section>

      <section className="card">
        <h3>识别参数</h3>
        <Field label="采样温度" hint="0 = 确定性输出，识别任务建议保持 0">
          <Slider
            value={asr.temperature}
            min={0}
            max={1}
            step={0.05}
            onChange={(v) => patchAsr({ temperature: v })}
            ariaLabel="采样温度"
            testId="asr-temperature"
            format={(v) => v.toFixed(2)}
          />
        </Field>
        <SwitchRow
          label="上下文提示"
          hint="把上一句已确认文本作为 prompt 传给服务，提升人名与术语一致性"
          checked={asr.contextPrompt}
          onChange={(v) => patchAsr({ contextPrompt: v })}
          testId="asr-context-prompt"
        />
      </section>
    </div>
  );
}
