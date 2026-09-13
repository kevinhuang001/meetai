/** 设置 · AI 接口 */
import { useEffect, useState } from "react";
import { api } from "../../lib/api";
import type { AiPreset, AiProvider, AiSettings, ConnectionTestResult } from "../../lib/contract";
import { guard, useStore } from "../../store";
import { Badge, Button, Field, Select, Slider, SwitchRow, TextInput } from "../ui";
import type { TabProps } from "./SettingsDialog";

function newProviderFromPreset(preset: AiPreset): AiProvider {
  return {
    id: typeof crypto !== "undefined" && "randomUUID" in crypto ? crypto.randomUUID() : `p-${Date.now()}`,
    name: preset.name,
    baseUrl: preset.baseUrl,
    apiKey: "",
    model: preset.model,
    temperature: 0.2,
    maxTokens: 1200,
    jsonMode: preset.jsonMode,
    timeoutSecs: 60,
    extraHeaders: [],
  };
}

export function AiTab({ draft, set }: TabProps) {
  const toast = useStore((s) => s.toast);
  const ai: AiSettings = draft.ai;

  const [presets, setPresets] = useState<AiPreset[]>([]);
  const [presetId, setPresetId] = useState<string>("deepseek");
  const [selectedId, setSelectedId] = useState<string>(ai.activeProviderId || ai.providers[0]?.id || "");
  const [showKey, setShowKey] = useState(false);
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

  const patchAi = (patch: Partial<AiSettings>) => set("ai", { ...ai, ...patch });
  const patchProvider = (patch: Partial<AiProvider>) => {
    if (!provider) return;
    patchAi({ providers: ai.providers.map((p) => (p.id === provider.id ? { ...p, ...patch } : p)) });
  };

  const addProvider = () => {
    const preset = presets.find((p) => p.id === presetId) ?? presets[0];
    if (!preset) return;
    const p = newProviderFromPreset(preset);
    patchAi({
      providers: [...ai.providers, p],
      activeProviderId: ai.providers.length === 0 ? p.id : ai.activeProviderId,
    });
    setSelectedId(p.id);
  };

  const removeProvider = (id: string) => {
    const rest = ai.providers.filter((p) => p.id !== id);
    patchAi({
      providers: rest,
      activeProviderId: ai.activeProviderId === id ? (rest[0]?.id ?? "") : ai.activeProviderId,
    });
    if (selectedId === id) setSelectedId(rest[0]?.id ?? "");
    setResult(null);
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

  return (
    <div className="tab-body">
      <section className="card">
        <h3>总开关</h3>
        <SwitchRow
          label="启用 AI 实时纪要"
          hint="关闭后录音仍会转写，但不会调用任何 AI 接口"
          checked={ai.enabled}
          onChange={(v) => patchAi({ enabled: v })}
          testId="ai-enabled"
        />
        <SwitchRow
          label="录音过程中自动总结"
          hint={`每 ${ai.intervalSecs} 秒检查一次，新增内容不足 ${ai.minNewChars} 字时跳过`}
          checked={ai.autoSummary}
          onChange={(v) => patchAi({ autoSummary: v })}
          testId="ai-autosummary"
        />
        <SwitchRow
          label="停止录音后自动生成完整会议纪要"
          hint="停止后立刻调用一次 AI，产出结构化的完整纪要"
          checked={ai.finalReportOnStop}
          onChange={(v) => patchAi({ finalReportOnStop: v })}
          testId="ai-final-report"
        />
      </section>

      <section className="card">
        <h3>
          服务商 <Badge>{ai.providers.length}</Badge>
        </h3>

        <div className="provider-list" data-testid="provider-list">
          {ai.providers.map((p) => (
            <button
              key={p.id}
              className={p.id === provider?.id ? "provider-item active" : "provider-item"}
              data-testid={`provider-item-${p.name}`}
              onClick={() => {
                setSelectedId(p.id);
                setResult(null);
              }}
            >
              <span className="p-name">{p.name}</span>
              <span className="p-url mono">{p.baseUrl || "未填写 Base URL"}</span>
              {p.id === ai.activeProviderId ? <Badge tone="ok">当前使用</Badge> : null}
            </button>
          ))}
          {ai.providers.length === 0 ? <p className="dim small">还没有服务商，从下面的预设里添加一个。</p> : null}
        </div>

        <div className="row">
          <div className="grow">
            <Select
              value={presetId}
              options={presets.map((p) => ({ value: p.id, label: `${p.name} — ${p.note}` }))}
              onChange={setPresetId}
              ariaLabel="服务商预设"
              testId="preset-select"
            />
          </div>
          <Button variant="primary" onClick={addProvider} testId="add-provider" ariaLabel="添加服务商">
            ＋ 添加服务商
          </Button>
        </div>

        {provider ? (
          <div className="provider-editor">
            <div className="editor-head">
              <h4>编辑：{provider.name}</h4>
              <div className="row-tight">
                <Button
                  variant="ghost"
                  onClick={() => patchAi({ activeProviderId: provider.id })}
                  disabled={ai.activeProviderId === provider.id}
                  testId="use-provider"
                >
                  {ai.activeProviderId === provider.id ? "当前使用中" : "设为当前使用"}
                </Button>
                <Button
                  variant="danger"
                  onClick={() => removeProvider(provider.id)}
                  disabled={ai.providers.length <= 1}
                  testId={`provider-delete-${provider.name}`}
                  ariaLabel={`删除服务商 ${provider.name}`}
                >
                  删除
                </Button>
              </div>
            </div>

            <Field label="名称">
              <TextInput value={provider.name} onChange={(v) => patchProvider({ name: v })} testId="provider-name" />
            </Field>
            <Field label="Base URL" hint="OpenAI 兼容接口地址，通常以 /v1 结尾">
              <TextInput
                value={provider.baseUrl}
                onChange={(v) => patchProvider({ baseUrl: v })}
                testId="provider-baseurl"
                mono
                placeholder="https://api.deepseek.com/v1"
              />
            </Field>
            <Field label="API Key" hint="仅在本地保存，不会上传">
              <span className="row-tight grow">
                <TextInput
                  value={provider.apiKey}
                  onChange={(v) => patchProvider({ apiKey: v })}
                  type={showKey ? "text" : "password"}
                  testId="provider-apikey"
                  mono
                  placeholder="sk-…"
                />
                <Button variant="ghost" onClick={() => setShowKey((v) => !v)} ariaLabel={showKey ? "隐藏密钥" : "显示密钥"}>
                  {showKey ? "隐藏" : "显示"}
                </Button>
              </span>
            </Field>
            <Field label="模型名">
              <TextInput value={provider.model} onChange={(v) => patchProvider({ model: v })} testId="provider-model" mono />
            </Field>
            <Field label="温度" hint="总结任务建议 0.1 ~ 0.3">
              <Slider
                value={provider.temperature}
                min={0}
                max={2}
                step={0.05}
                onChange={(v) => patchProvider({ temperature: v })}
                ariaLabel="温度"
                format={(v) => v.toFixed(2)}
              />
            </Field>
            <Field label="最大输出 token">
              <Slider
                value={provider.maxTokens}
                min={256}
                max={8192}
                step={128}
                onChange={(v) => patchProvider({ maxTokens: v })}
                ariaLabel="最大输出 token"
              />
            </Field>
            <Field label="超时（秒）">
              <Slider
                value={provider.timeoutSecs}
                min={10}
                max={180}
                step={5}
                onChange={(v) => patchProvider({ timeoutSecs: v })}
                ariaLabel="请求超时"
                format={(v) => `${v}s`}
              />
            </Field>
            <SwitchRow
              label="JSON 模式"
              hint="发送 response_format=json_object，不支持的服务商会自动降级重试"
              checked={provider.jsonMode}
              onChange={(v) => patchProvider({ jsonMode: v })}
            />

            <div className="headers-editor">
              <div className="row">
                <span className="field-label">自定义请求头</span>
                <span className="spacer" />
                <Button
                  variant="ghost"
                  onClick={() => patchProvider({ extraHeaders: [...provider.extraHeaders, ["", ""]] })}
                  testId="add-header"
                >
                  ＋ 添加
                </Button>
              </div>
              {provider.extraHeaders.map(([k, v], i) => (
                <div className="row-tight" key={`hdr-${i}`}>
                  <TextInput
                    value={k}
                    onChange={(nv) => {
                      const next = provider.extraHeaders.map((h, j) => (j === i ? ([nv, h[1]] as [string, string]) : h));
                      patchProvider({ extraHeaders: next });
                    }}
                    placeholder="Header"
                    mono
                  />
                  <TextInput
                    value={v}
                    onChange={(nv) => {
                      const next = provider.extraHeaders.map((h, j) => (j === i ? ([h[0], nv] as [string, string]) : h));
                      patchProvider({ extraHeaders: next });
                    }}
                    placeholder="Value"
                    mono
                  />
                  <Button
                    variant="ghost"
                    ariaLabel="删除该请求头"
                    onClick={() => patchProvider({ extraHeaders: provider.extraHeaders.filter((_, j) => j !== i) })}
                  >
                    ✕
                  </Button>
                </div>
              ))}
            </div>

            <div className="row">
              <Button variant="primary" onClick={() => void testConnection()} disabled={testing} testId="test-connection">
                {testing ? "测试中…" : "⚡ 测试连接"}
              </Button>
              {ai.activeProviderId !== provider.id ? (
                <span className="dim small">当前使用的服务商是「{ai.providers.find((p) => p.id === ai.activeProviderId)?.name ?? "无"}」</span>
              ) : null}
            </div>

            {result ? (
              <div className={result.ok ? "test-result ok" : "test-result bad"} data-testid="test-result">
                <div className="row">
                  <strong>{result.ok ? "连接成功" : "连接失败"}</strong>
                  <span className="spacer" />
                  <span className="mono small">延迟 {result.latencyMs} ms</span>
                </div>
                {result.error ? <p className="small">{result.error}</p> : null}
                {result.sample ? (
                  <p className="small">
                    返回样例：<span className="mono">{result.sample}</span>
                  </p>
                ) : null}
                {result.models.length ? (
                  <div className="model-chips">
                    <span className="dim small">可用模型（点击填入）：</span>
                    {result.models.map((m) => (
                      <button key={m} className="chip" onClick={() => patchProvider({ model: m })}>
                        {m}
                      </button>
                    ))}
                  </div>
                ) : null}
              </div>
            ) : null}
          </div>
        ) : null}
      </section>

      <section className="card">
        <h3>总结策略</h3>
        <Field label="自动总结间隔" hint="秒">
          <Slider
            value={ai.intervalSecs}
            min={5}
            max={120}
            step={5}
            onChange={(v) => patchAi({ intervalSecs: v })}
            ariaLabel="自动总结间隔"
            format={(v) => `${v}s`}
          />
        </Field>
        <Field label="最小新增字数" hint="低于该字数跳过本次总结，省 token">
          <Slider
            value={ai.minNewChars}
            min={10}
            max={500}
            step={10}
            onChange={(v) => patchAi({ minNewChars: v })}
            ariaLabel="最小新增字数"
            format={(v) => `${v} 字`}
          />
        </Field>
        <Field label="Live 窗口" hint="「刚刚说到」覆盖的最近时长（秒）">
          <Slider
            value={ai.liveWindowSecs}
            min={20}
            max={300}
            step={10}
            onChange={(v) => patchAi({ liveWindowSecs: v })}
            ariaLabel="Live 窗口秒数"
            format={(v) => `${v}s`}
          />
        </Field>
        <Field label="上下文上限" hint="送给模型的转写字符数上限">
          <Slider
            value={ai.maxContextChars}
            min={1000}
            max={20000}
            step={500}
            onChange={(v) => patchAi({ maxContextChars: v })}
            ariaLabel="上下文上限"
            format={(v) => `${v} 字`}
          />
        </Field>
      </section>
    </div>
  );
}
