/** 向导第 3 步：AI 接口（实时纪要与完整会议纪要） */
import { useEffect, useState } from "react";
import { api } from "../../lib/api";
import type { AiPreset, AiProvider, ConnectionTestResult } from "../../lib/contract";
import { OLLAMA_PAGE, consoleLinks } from "../../lib/links";
import { guard, useStore } from "../../store";
import { Button, Field, Select, TextInput } from "../ui";
import { LinkButtons, StepHead, TestBox, type StepProps } from "./parts";

function providerFromPreset(preset: AiPreset): AiProvider {
  return {
    id: preset.id,
    name: preset.name,
    baseUrl: preset.baseUrl,
    apiKey: "",
    model: preset.model,
    temperature: 0.2,
    maxTokens: 1200,
    jsonMode: preset.jsonMode,
    noThinking: true,
    timeoutSecs: 60,
    extraHeaders: [],
  };
}

export function StepAi({ draft, set }: StepProps) {
  const toast = useStore((s) => s.toast);
  const ai = draft.ai;

  const [presets, setPresets] = useState<AiPreset[]>([]);
  const [presetId, setPresetId] = useState("");
  const [selectedId, setSelectedId] = useState(ai.activeProviderId || ai.providers[0]?.id || "");
  const [testing, setTesting] = useState(false);
  const [result, setResult] = useState<ConnectionTestResult | null>(null);

  useEffect(() => {
    void guard("读取 AI 预设", () => api.listAiPresets()).then((list) => {
      if (list && list.length) {
        setPresets(list);
        setPresetId(list[0].id);
      }
    });
  }, []);

  const provider: AiProvider | null =
    ai.providers.find((p) => p.id === selectedId) ?? ai.providers[0] ?? null;

  const patchAi = (patch: Partial<typeof ai>) => set("ai", { ...ai, ...patch });
  const patchProvider = (patch: Partial<AiProvider>) => {
    if (!provider) return;
    patchAi({ providers: ai.providers.map((p) => (p.id === provider.id ? { ...p, ...patch } : p)) });
  };

  const choosePreset = (id: string) => {
    setPresetId(id);
    setResult(null);
    const preset = presets.find((p) => p.id === id);
    if (!preset) return;
    const existing = ai.providers.find((p) => p.id === preset.id);
    if (existing) {
      setSelectedId(existing.id);
      patchAi({ activeProviderId: existing.id });
      return;
    }
    const created = providerFromPreset(preset);
    patchAi({ providers: [...ai.providers, created], activeProviderId: created.id });
    setSelectedId(created.id);
  };

  const testConnection = async () => {
    if (!provider) return;
    setTesting(true);
    setResult(null);
    const r = await guard("测试 AI 连接", () => api.testAiConnection(provider));
    setTesting(false);
    if (!r) return;
    setResult(r);
    toast(r.ok ? "success" : "error", "测试连接", r.ok ? `连接正常，延迟 ${r.latencyMs} ms` : (r.error ?? "连接失败"));
  };

  const isOllama = /ollama/i.test(`${provider?.id ?? ""} ${provider?.name ?? ""} ${provider?.baseUrl ?? ""}`);

  return (
    <div className="ob-body-inner">
      <StepHead
        title="AI 接口"
        desc="转写出来的文本会送给这个模型做实时纪要与完整会议纪要。用不到 AI 纪要的话，也可以把「启用 AI 实时纪要」关掉。"
      />

      <Field label="服务商预设" hint="选一个会自动填好下面的字段，之后可以改">
        <Select
          value={presetId}
          options={presets.map((p) => ({ value: p.id, label: `${p.name} — ${p.note}` }))}
          onChange={choosePreset}
          ariaLabel="AI 服务商预设"
          testId="onboarding-ai-preset"
        />
      </Field>

      <Field label="Base URL" hint="OpenAI 兼容接口地址，通常以 /v1 结尾">
        <TextInput
          value={provider?.baseUrl ?? ""}
          onChange={(v) => patchProvider({ baseUrl: v })}
          testId="onboarding-ai-baseurl"
          ariaLabel="AI Base URL"
          mono
          placeholder="https://api.deepseek.com/v1"
        />
      </Field>
      <Field label="API Key" hint="只在本地保存；本地 Ollama 不需要">
        <TextInput
          value={provider?.apiKey ?? ""}
          onChange={(v) => patchProvider({ apiKey: v })}
          testId="onboarding-ai-apikey"
          ariaLabel="AI API Key"
          type="password"
          mono
          placeholder="sk-…"
        />
      </Field>
      <Field label="模型名" hint="例如 deepseek-chat / gpt-4o-mini / qwen3.5:2b">
        <TextInput
          value={provider?.model ?? ""}
          onChange={(v) => patchProvider({ model: v })}
          testId="onboarding-ai-model"
          ariaLabel="AI 模型名"
          mono
        />
      </Field>

      <div className="ob-tip" data-testid="onboarding-ai-ollama-hint">
        <strong>不想花钱 / 不想联网：用本机 Ollama</strong>
        <span className="small">
          装好 Ollama 并拉一个模型后，Base URL 填 <code className="mono">http://localhost:11434/v1</code>，
          模型名填你已经 pull 下来的那个（例如 <code className="mono">qwen3.5:2b</code>），API Key 留空。
        </span>
        <div className="cmd-row">
          <code className="notice-code grow">ollama pull qwen3.5:2b</code>
        </div>
        <LinkButtons links={[OLLAMA_PAGE]} />
      </div>

      <div className="row">
        <Button variant="primary" onClick={() => void testConnection()} disabled={testing} testId="onboarding-ai-test">
          {testing ? "测试中…" : "⚡ 测试连接"}
        </Button>
        <span className="dim tiny">测试只会发一句很短的话，确认「地址 + 鉴权 + 模型名」可用。</span>
      </div>

      {result ? (
        <TestBox
          result={result}
          testId="onboarding-ai-test-result"
          emptySampleNote="服务返回了空内容，说明接口通了但模型没输出，检查一下模型名。"
        />
      ) : null}

      <LinkButtons links={consoleLinks(provider?.id, provider?.name, provider?.baseUrl)} testId="onboarding-ai-open-console" />
      {isOllama ? <p className="dim tiny">Ollama 默认监听 11434，若不是默认端口请改成实际端口。</p> : null}
    </div>
  );
}
