/** 向导第 1 步：欢迎与隐私说明 */
import { StepHead } from "./parts";

export function StepWelcome() {
  return (
    <div className="ob-body-inner">
      <StepHead
        title="欢迎使用 MeetingHear"
        desc="把会议里的说话实时转成文字，并让 AI 边听边整理纪要。应用本身不含识别模型，语音识别与 AI 纪要都通过 HTTP 调用你自己配置的服务。"
      />

      <div className="warn-bar" data-testid="onboarding-privacy">
        <strong>隐私：音频会发送到你配置的识别服务</strong>
        <span>
          启用录音后，采集到的会议音频会按句切分并上传到「设置 → 语音识别」里选中的服务商。
          如果那是一个云端服务（Groq / OpenAI / 硅基流动…），音频就会离开这台电脑。
        </span>
        <span>
          想完全不外传：把识别指向本机的 <b>whisper.cpp server</b> 或 <b>faster-whisper-server</b>，
          把 AI 指向本机的 <b>Ollama</b>，这样音频与文本都不会出本机。
        </span>
        <span>AI 纪要只会收到转写出来的<strong>文本</strong>，不会收到音频；关掉 AI 就完全不会调用它。</span>
      </div>

      <ul className="ob-list">
        <li>
          <b>第 2 步 · 语音识别服务</b>：决定音频发给谁，可以当场点「测试连接」验证。
        </li>
        <li>
          <b>第 3 步 · AI 接口</b>：负责实时纪要与完整会议纪要，同样可以当场验证。
        </li>
        <li>
          <b>第 4 步 · 音频设备</b>：选麦克风，并处理系统内录（对方的声音）。
        </li>
      </ul>

      <p className="dim small">整个向导大约需要 2 分钟；所有设置之后都可以在「设置」里随时修改。</p>
    </div>
  );
}
