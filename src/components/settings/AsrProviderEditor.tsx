/** 设置 · 语音识别：单个服务商的编辑表单（供 AsrTab 使用） */
import { useState } from "react";
import type { AsrConnectionTestResult, AsrProvider } from "../../lib/contract";
import { Button, Field, Select, Slider, TextInput } from "../ui";

const RESPONSE_FORMATS: { value: string; label: string }[] = [
  { value: "json", label: "json（最兼容，推荐）" },
  { value: "verbose_json", label: "verbose_json（附带语言与分段）" },
];

export function AsrProviderEditor({
  provider,
  active,
  canRemove,
  testing,
  result,
  patchProvider,
  onSetActive,
  onRemove,
  onTest,
}: {
  provider: AsrProvider;
  active: boolean;
  canRemove: boolean;
  testing: boolean;
  result: AsrConnectionTestResult | null;
  patchProvider: (patch: Partial<AsrProvider>) => void;
  onSetActive: () => void;
  onRemove: () => void;
  onTest: () => void;
}) {
  const [showKey, setShowKey] = useState(false);

  return (
    <div className="provider-editor">
      <div className="editor-head">
        <h4>编辑：{provider.name}</h4>
        <div className="row-tight">
          <Button variant="ghost" onClick={onSetActive} disabled={active} testId="use-asr-provider">
            {active ? "当前使用中" : "设为当前使用"}
          </Button>
          <Button
            variant="danger"
            onClick={onRemove}
            disabled={!canRemove}
            testId="asr-provider-delete"
            ariaLabel={`删除识别服务商 ${provider.name}`}
          >
            删除
          </Button>
        </div>
      </div>

      <Field label="名称">
        <TextInput value={provider.name} onChange={(v) => patchProvider({ name: v })} testId="asr-provider-name" />
      </Field>
      <Field label="Base URL" hint="服务根地址，通常以 /v1 结尾；本地服务填 http://localhost:端口">
        <TextInput
          value={provider.baseUrl}
          onChange={(v) => patchProvider({ baseUrl: v })}
          testId="asr-provider-baseurl"
          mono
          placeholder="https://api.groq.com/openai/v1"
        />
      </Field>
      <Field label="接口路径" hint="OpenAI 兼容填 /audio/transcriptions，whisper.cpp server 填 /inference">
        <TextInput
          value={provider.transcriptionPath}
          onChange={(v) => patchProvider({ transcriptionPath: v })}
          testId="asr-provider-path"
          mono
          placeholder="/audio/transcriptions"
        />
      </Field>
      <Field label="API Key" hint="只在本地保存；本地/自建服务可以留空">
        <span className="row-tight grow">
          <TextInput
            value={provider.apiKey}
            onChange={(v) => patchProvider({ apiKey: v })}
            type={showKey ? "text" : "password"}
            testId="asr-provider-apikey"
            mono
            placeholder="gsk_…"
          />
          <Button variant="ghost" onClick={() => setShowKey((v) => !v)} ariaLabel={showKey ? "隐藏密钥" : "显示密钥"}>
            {showKey ? "隐藏" : "显示"}
          </Button>
        </span>
      </Field>
      <Field label="模型名" hint="例如 whisper-large-v3-turbo / whisper-1 / large-v3">
        <TextInput value={provider.model} onChange={(v) => patchProvider({ model: v })} testId="asr-provider-model" mono />
      </Field>
      <Field label="请求格式" hint="服务不支持 verbose_json 时会自动降级为 json">
        <Select
          value={provider.responseFormat}
          options={RESPONSE_FORMATS}
          onChange={(v) => patchProvider({ responseFormat: v })}
          ariaLabel="请求格式"
          testId="asr-provider-format"
        />
      </Field>
      <Field label="超时（秒）" hint="单次转写请求的最长等待时间">
        <Slider
          value={provider.timeoutSecs}
          min={10}
          max={300}
          step={5}
          onChange={(v) => patchProvider({ timeoutSecs: v })}
          ariaLabel="转写请求超时"
          testId="asr-provider-timeout"
          format={(v) => `${v}s`}
        />
      </Field>

      <div className="headers-editor">
        <div className="row">
          <span className="field-label">额外请求头</span>
          <span className="spacer" />
          <Button
            variant="ghost"
            onClick={() => patchProvider({ extraHeaders: [...provider.extraHeaders, ["", ""]] })}
            testId="add-asr-header"
          >
            ＋ 添加
          </Button>
        </div>
        {provider.extraHeaders.map(([k, v], i) => (
          <div className="row-tight" key={`asr-hdr-${i}`}>
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
        <Button variant="primary" onClick={onTest} disabled={testing} testId="test-asr-connection">
          {testing ? "测试中…" : "⚡ 测试连接"}
        </Button>
        <span className="dim tiny">
          测试会向服务发送 0.6 秒静音，只验证「地址 + 鉴权 + 模型名」是否可用，返回空文本属正常。
        </span>
      </div>

      {result ? (
        <div className={result.ok ? "test-result ok" : "test-result bad"} data-testid="asr-test-result">
          <div className="row">
            <strong>{result.ok ? "连接成功" : "连接失败"}</strong>
            <span className="spacer" />
            <span className="mono small">延迟 {result.latencyMs} ms</span>
          </div>
          {result.error ? <p className="small">{result.error}</p> : null}
          {result.ok ? (
            <p className="small">
              返回样例：<span className="mono">{result.sample || "（空文本，属正常）"}</span>
            </p>
          ) : null}
          {result.ok && result.sample.trim().length === 0 ? (
            <p className="dim tiny">静音测试返回空文本是正常的 —— 服务收到了请求，只是认为这段音频里没有语音。</p>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
