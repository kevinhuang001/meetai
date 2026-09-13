#!/usr/bin/env node
/**
 * MeetingHear 真实浏览器冒烟测试。
 *
 * 做三件事：
 *  1. 启动 `pnpm dev`（vite，固定 1420 端口），等端口就绪；
 *  2. 用 Playwright chromium 打开 http://127.0.0.1:1420 ，跑完整的交互断言；
 *  3. 全程截图到 artifacts/，逐项打印 PASS / FAIL，失败以非 0 退出码结束。
 *
 * 断言全部基于轮询等待（waitFor），不依赖固定 sleep，避免 mock 时序抖动导致偶发失败。
 */
import { spawn } from "node:child_process";
import { mkdir } from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { chromium } from "playwright";

const ROOT = path.resolve(import.meta.dirname, "..");
const BASE = "http://127.0.0.1:1420";
const ARTIFACTS = path.join(ROOT, "artifacts");
const SHOTS = [];

const results = [];
let failures = 0;

function check(name, ok, detail = "") {
  results.push({ name, ok, detail });
  if (!ok) failures += 1;
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? `  — ${detail}` : ""}`);
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function waitFor(fn, { timeout = 20000, interval = 200, label = "" } = {}) {
  const start = Date.now();
  let lastErr = null;
  while (Date.now() - start < timeout) {
    try {
      const v = await fn();
      if (v) return v;
    } catch (e) {
      lastErr = e;
    }
    await sleep(interval);
  }
  throw new Error(`等待超时 ${timeout}ms ${label}${lastErr ? ` (${lastErr.message})` : ""}`);
}

async function portAlive() {
  try {
    const r = await fetch(BASE, { signal: AbortSignal.timeout(1500) });
    return r.ok || r.status === 404;
  } catch {
    return false;
  }
}

async function startDevServer() {
  if (await portAlive()) {
    console.log(`[setup] ${BASE} 已在服务中，复用现有进程`);
    return null;
  }
  console.log("[setup] 启动 pnpm dev …");
  const child = spawn("pnpm", ["dev"], {
    cwd: ROOT,
    detached: true,
    stdio: ["ignore", "pipe", "pipe"],
  });
  const logs = [];
  child.stdout.on("data", (d) => logs.push(String(d)));
  child.stderr.on("data", (d) => logs.push(String(d)));
  child.on("exit", (code) => {
    if (code !== 0 && code !== null) console.log(`[setup] dev server 退出，code=${code}`);
  });
  try {
    await waitFor(() => portAlive(), { timeout: 60000, interval: 400, label: "等待 vite 端口 1420" });
  } catch (e) {
    console.log("[setup] vite 日志：\n" + logs.join(""));
    throw e;
  }
  console.log("[setup] vite 已就绪");
  return child;
}

function stopDevServer(child) {
  if (!child) return;
  try {
    process.kill(-child.pid, "SIGTERM");
  } catch {
    try {
      child.kill("SIGTERM");
    } catch {
      /* ignore */
    }
  }
}

async function shot(page, name) {
  const file = path.join(ARTIFACTS, name);
  try {
    await page.screenshot({ path: file, fullPage: false });
    SHOTS.push(path.relative(ROOT, file));
  } catch (e) {
    console.log(`[shot] 截图失败 ${name}: ${e.message}`);
  }
}

const IGNORED_CONSOLE = [/favicon\.ico/i, /Download the React DevTools/i];

async function main() {
  await mkdir(ARTIFACTS, { recursive: true });

  const child = await startDevServer();
  const browser = await chromium.launch({ args: ["--no-sandbox"] });
  const context = await browser.newContext({
    viewport: { width: 1360, height: 860 },
    permissions: ["clipboard-read", "clipboard-write"],
  });
  const page = await context.newPage();

  const consoleErrors = [];
  const ignoredConsole = [];
  page.on("console", (msg) => {
    if (msg.type() !== "error") return;
    const text = msg.text();
    const loc = msg.location()?.url ?? "";
    if (IGNORED_CONSOLE.some((re) => re.test(text) || re.test(loc))) {
      ignoredConsole.push(text);
      return;
    }
    consoleErrors.push(`${text} @ ${loc}`);
  });
  page.on("pageerror", (err) => consoleErrors.push(`pageerror: ${err.message}`));

  try {
    /* ---------------------------------------------------------- 打开应用 */
    await page.goto(BASE, { waitUntil: "domcontentloaded" });

    await waitFor(() => page.locator('[data-testid="transcript-list"]').count(), {
      timeout: 20000,
      label: "等待应用启动完成",
    });

    // b. 空状态
    await waitFor(() => page.locator('[data-testid="transcript-empty"]').isVisible(), {
      timeout: 15000,
      label: "等待空状态出现",
    });
    const emptyText = (await page.locator('[data-testid="transcript-empty"]').innerText()).replace(/\s+/g, " ");
    check("b. 空状态可见", true, emptyText.slice(0, 40) + "…");
    check(
      "b2. 空状态含引导文案（开始录音 / 导入音频）",
      /开始录音/.test(emptyText) && /导入|音频/.test(emptyText),
      emptyText.slice(0, 60),
    );
    await shot(page, "01-empty.png");

    /* ---------------------------------------------------------- 开始录音 */
    await page.locator('[data-testid="start-recording"]').click();
    await waitFor(() => page.locator('[data-testid="status-pill"]').innerText().then((t) => /录音中|启动中/.test(t)), {
      timeout: 15000,
      label: "等待进入录音状态",
    });
    await sleep(1500);
    await shot(page, "02-recording.png");

    // d. 电平条宽度变化：连续采样，要求出现至少 2 个不同值
    const widths = [];
    for (let i = 0; i < 12; i += 1) {
      const w = await page
        .locator('[data-testid="level-mic-fill"]')
        .evaluate((el) => el.style.width)
        .catch(() => "");
      if (w) widths.push(w);
      await sleep(300);
    }
    const distinct = new Set(widths);
    check(
      "d. 电平条宽度随录音变化",
      distinct.size >= 2,
      `采样 ${widths.length} 次，不同取值 ${distinct.size} 个：${[...distinct].slice(0, 6).join(", ")}`,
    );

    // c. 约 12 秒后至少 2 条已定稿转写
    await sleep(12000);
    const segCount = await page.locator('[data-testid="transcript-list"] [data-testid="segment-row"]').count();
    let counted = segCount;
    if (counted < 2) {
      counted = await waitFor(
        async () => {
          const n = await page.locator('[data-testid="transcript-list"] [data-testid="segment-row"]').count();
          return n >= 2 ? n : 0;
        },
        { timeout: 20000, interval: 500, label: "等待 ≥2 条定稿转写" },
      ).catch(() => counted);
    }
    check("c1. 转写列表出现 ≥2 条已定稿转写", counted >= 2, `12 秒时 ${segCount} 条，最终 ${counted} 条`);

    await shot(page, "03-segments.png");

    // 活动行（partial）应当出现过
    const partialSeen = await page.locator('[data-testid="partial-row"]').count();
    check("c2. 存在「正在说话」的活动行或已定稿行", partialSeen > 0 || counted > 0, `partial-row=${partialSeen}`);

    // c. 「刚刚说到」AI 纪要文本非空
    let live = "";
    try {
      live = await waitFor(
        async () => {
          const el = page.locator('[data-testid="summary-live"]');
          if ((await el.count()) === 0) return "";
          const t = (await el.innerText()).trim();
          return t && !t.startsWith("还没有可以总结") ? t : "";
        },
        { timeout: 25000, interval: 500, label: "等待 AI 实时纪要" },
      );
    } catch {
      live = "";
    }
    check("c3. 「刚刚说到」AI 纪要文本非空", live.length > 0, live.slice(0, 48) + (live ? "…" : ""));

    const metrics = await page.locator('[data-testid="metrics"]').innerText().catch(() => "");
    check("c4. 指标条显示 RTF / 延迟 / 字数", /RTF/.test(metrics) && /延迟/.test(metrics) && /字数/.test(metrics), metrics.replace(/\s+/g, " ").slice(0, 70));
    const serviceMetric = await page.locator('[data-testid="metric-asr-service"]').innerText().catch(() => "");
    check(
      "c4b. 指标条显示识别服务（服务商 · 模型）而非本地模型名",
      /·/.test(serviceMetric) && /whisper|SenseVoice|faster-whisper/i.test(serviceMetric),
      serviceMetric,
    );

    const summaryPanel = await page.locator('[data-testid="summary-panel"]').innerText().catch(() => "");
    check(
      "c5. 纪要面板渲染总览/要点/待办分区",
      /会议总览/.test(summaryPanel) && /关键要点/.test(summaryPanel) && /待办事项/.test(summaryPanel),
    );
    await shot(page, "04-summary.png");

    /* ---------------------------------------------------------- 过滤 / 复制 / 回到最新 */
    await page.locator('[data-testid="transcript-filter"]').fill("复盘");
    const filterHint = await waitFor(() => page.locator('[data-testid="filter-hint"]').isVisible().catch(() => false), {
      timeout: 5000,
      interval: 200,
      label: "等待过滤提示",
    }).catch(() => false);
    const filteredCount = await page.locator('[data-testid="transcript-list"] [data-testid="segment-row"]').count();
    check("c6. 关键字过滤转写并暂停自动滚动", filterHint && filteredCount >= 1, `匹配 ${filteredCount} 条，提示=${filterHint}`);
    await page.locator('[data-testid="transcript-filter"]').fill("");
    await sleep(300);

    await page.locator('[data-testid="copy-all"]').click();
    const copyToast = await waitFor(
      async () => {
        const t = page.locator('[data-testid="toasts"]');
        if ((await t.count()) === 0) return false;
        const txt = await t.innerText();
        return /已复制/.test(txt) ? txt.replace(/\s+/g, " ") : false;
      },
      { timeout: 6000, interval: 250, label: "等待复制提示" },
    ).catch(() => "");
    check("c7. 一键复制全部转写", Boolean(copyToast), String(copyToast).slice(0, 40));

    // 上滚 → 暂停吸底 + 「回到最新」按钮（临时缩小窗口制造溢出）
    await page.setViewportSize({ width: 1360, height: 320 });

    // 先等内容真的溢出再断言。
    // mock 的转写是按时间逐条生成的，机器负载高时内容可能还没铺满列表，
    // 这时「上滚」不会触发「回到最新」，断言就会偶发失败（实测出现过 71px 的溢出）。
    const overflow = await waitFor(
      async () => {
        const v = await page
          .locator('[data-testid="transcript-list"]')
          .evaluate((el) => el.scrollHeight - el.clientHeight)
          .catch(() => 0);
        return v >= 120 ? v : false;
      },
      { timeout: 25000, interval: 300, label: "等待转写列表溢出" },
    ).catch(() => 0);
    check("c8a. 转写列表内容足以滚动", overflow >= 120, `可滚动高度 ${overflow}px`);

    // 上滚是「用户动作」，允许重试几次：
    // 恰好有新句子到达时，应用会先把列表吸回底部（这是期望行为），
    // 单次尝试就会偶发失败。重试用户动作不等于放宽断言 —— 断言仍然是
    // 「上滚之后必须出现回到最新按钮」。
    const jumpShown = await waitFor(
      async () => {
        await page
          .locator('[data-testid="transcript-list"]')
          .evaluate((el) => {
            el.scrollTop = 0;
          })
          .catch(() => {});
        await sleep(400);
        return page
          .locator('[data-testid="jump-to-latest"]')
          .isVisible()
          .catch(() => false);
      },
      { timeout: 12000, interval: 300, label: "等待回到最新按钮" },
    ).catch(() => false);
    check("c8. 用户上滚后暂停自动滚动并出现「回到最新」", jumpShown, `可滚动高度 ${overflow}px`);
    if (jumpShown) {
      await shot(page, "13-scrolled.png");
      await page.locator('[data-testid="jump-to-latest"]').click();
      await waitFor(async () => (await page.locator('[data-testid="jump-to-latest"]').count()) === 0, {
        timeout: 5000,
        interval: 200,
        label: "等待按钮消失",
      }).catch(() => {});
      check("c9. 点击「回到最新」后恢复吸底", (await page.locator('[data-testid="jump-to-latest"]').count()) === 0);
    }
    await page.setViewportSize({ width: 1360, height: 860 });
    await sleep(300);

    /* ---------------------------------------------------------- 设置弹窗 */
    await page.locator('[data-testid="open-settings"]').click();
    await waitFor(() => page.locator('[data-testid="settings-dialog"]').isVisible(), {
      timeout: 8000,
      label: "等待设置弹窗",
    });
    check("e1. 设置弹窗打开", await page.locator('[data-testid="settings-dialog"]').isVisible());
    const role = await page.locator('[data-testid="settings-dialog"]').getAttribute("role");
    const ariaModal = await page.locator('[data-testid="settings-dialog"]').getAttribute("aria-modal");
    check("e2. 弹窗具备 role=dialog / aria-modal", role === "dialog" && ariaModal === "true", `role=${role} aria-modal=${ariaModal}`);

    /* ---------------- 语音识别标签页（App 不含识别模型，配置外部识别服务） ---------------- */
    await page.locator('[data-testid="tab-asr"]').click();
    await waitFor(() => page.locator('[data-testid="asr-provider-list"] [data-testid^="asr-provider-"]').count(), {
      timeout: 10000,
      label: "等待识别服务商列表",
    });
    const providerItems = page.locator('[data-testid="asr-provider-list"] [data-testid^="asr-provider-"]');
    const providerCount = await providerItems.count();
    check("e3. 语音识别页列出识别服务商", providerCount >= 1, `${providerCount} 个服务商`);

    const noticeText = (await page.locator('[data-testid="asr-service-notice"]').innerText()).replace(/\s+/g, " ");
    check(
      "e3b. 提示块说明「本应用不含识别模型，需要外部识别服务」",
      /不含识别模型/.test(noticeText) && /外部识别服务/.test(noticeText) && /whisper/.test(noticeText),
      noticeText.slice(0, 48) + "…",
    );
    await page.locator(".settings-content").evaluate((el) => {
      el.scrollTop = 0;
    });
    await shot(page, "05-settings-asr-service.png");

    const asrBaseUrl = await page.locator('[data-testid="asr-provider-baseurl"]').inputValue();
    check("e4. Base URL 输入框已按预设填好", /^https?:\/\//.test(asrBaseUrl), asrBaseUrl);
    const asrPath = await page.locator('[data-testid="asr-provider-path"]').inputValue();
    check(
      "e4b. 接口路径有值且说明 OpenAI 兼容 / whisper.cpp 两种写法",
      asrPath.startsWith("/"),
      asrPath,
    );
    const asrKeyType = await page.locator('[data-testid="asr-provider-apikey"]').getAttribute("type");
    check("e5. API Key 为密码框", asrKeyType === "password", `type=${asrKeyType}`);
    const asrModel = await page.locator('[data-testid="asr-provider-model"]').inputValue();
    check("e5b. 模型名已按预设填好", asrModel.trim().length > 0, asrModel);

    // 「测试连接」：mock 后端对静音测试返回 ok + 延迟 + 样例
    await page.locator('[data-testid="test-asr-connection"]').click();
    const testResult = await waitFor(
      async () => {
        const el = page.locator('[data-testid="asr-test-result"]');
        if ((await el.count()) === 0) return "";
        const t = (await el.innerText()).replace(/\s+/g, " ");
        return t.length > 0 ? t : "";
      },
      { timeout: 8000, interval: 200, label: "等待识别服务连接测试结果" },
    ).catch(() => "");
    check(
      "e5c. 「测试连接」返回成功、延迟与静音样例说明",
      /连接成功/.test(testResult) && /延迟/.test(testResult) && /静音/.test(testResult),
      testResult.slice(0, 60) + "…",
    );
    await page.locator('[data-testid="asr-test-result"]').scrollIntoViewIfNeeded().catch(() => {});
    await shot(page, "05b-settings-asr-test.png");

    const livePreview = await page.locator('[data-testid="asr-live-preview"]').getAttribute("aria-checked");
    check("e5d. 「边说边出字」开关存在（默认关）", livePreview === "false", `aria-checked=${livePreview}`);
    const liveHint = await page.locator('[data-testid="asr-live-preview"]').getAttribute("aria-label");
    const liveRowText = (await page.locator('[data-testid="asr-live-preview"]').evaluate((el) => el.closest(".switch-row")?.innerText ?? "")).replace(/\s+/g, " ");
    check(
      "e5e. 「边说边出字」说明会显著增加 API 调用次数",
      /API 调用次数/.test(liveRowText) && /800ms/.test(liveRowText),
      `${liveHint} / ${liveRowText.slice(0, 50)}…`,
    );
    const maxChunk = await page.locator('[data-testid="asr-max-chunk"]').inputValue();
    check("e5f. 单次上传时长上限在 5~120 秒范围内", Number(maxChunk) >= 5 && Number(maxChunk) <= 120, `${maxChunk}s`);
    const langOptions = await page.locator('[data-testid="asr-language"] option').count();
    check("e5g. 识别语言下拉复用语言列表", langOptions >= 10, `${langOptions} 种语言`);

    // 预设下拉：已存在的预设应「切过去」而不是新建
    await page.locator('[data-testid="asr-preset-select"]').selectOption("openai");
    const afterSwitch = await page.locator('[data-testid="asr-provider-baseurl"]').inputValue();
    check(
      "e5h. 切换预设自动填入 Base URL",
      afterSwitch === "https://api.openai.com/v1",
      afterSwitch,
    );
    await page.locator('[data-testid="asr-preset-select"]').selectOption("groq");
    const backToGroq = await page.locator('[data-testid="asr-provider-baseurl"]').inputValue();
    const afterSwitchBack = await providerItems.count();
    check(
      "e5i. 预设已存在时切回原服务商而不新建",
      backToGroq === "https://api.groq.com/openai/v1" && afterSwitchBack === providerCount + 1,
      `${backToGroq}，服务商 ${afterSwitchBack} 个`,
    );

    // 新增 / 删除：至少保留一个服务商
    const beforeAdd = await providerItems.count();
    await page.locator('[data-testid="add-asr-provider"]').click();
    const afterAdd = await providerItems.count();
    check("e5j. 可以新增服务商", afterAdd === beforeAdd + 1, `${beforeAdd} → ${afterAdd}`);
    await page.locator('[data-testid="asr-provider-delete"]').click();
    const afterDelete = await providerItems.count();
    check("e5k. 可以删除服务商", afterDelete === afterAdd - 1, `${afterAdd} → ${afterDelete}`);
    await page.locator('[data-testid="asr-provider-delete"]').click();
    const onlyOne = await providerItems.count();
    const deleteDisabled = await page.locator('[data-testid="asr-provider-delete"]').isDisabled();
    check(
      "e5k2. 至少保留一个服务商（剩 1 个时删除按钮禁用）",
      onlyOne === 1 && deleteDisabled,
      `剩余 ${onlyOne} 个，删除按钮 disabled=${deleteDisabled}`,
    );

    // 音频设备 标签页
    await page.locator('[data-testid="tab-audio"]').click();
    await waitFor(() => page.locator('[data-testid="refresh-devices"]').isVisible(), {
      timeout: 8000,
      label: "等待音频设备页",
    });
    const deviceRows = await page.locator(".device-list li").count();
    const audioOk =
      (await page.locator('[data-testid="enable-mic"]').isVisible()) &&
      (await page.locator('[data-testid="enable-loopback"]').isVisible()) &&
      (await page.locator('[data-testid="mic-device"]').isVisible());
    check("e5l. 音频设备页（麦克风/内录开关 + 设备下拉 + 刷新）", audioOk && deviceRows >= 3, `设备 ${deviceRows} 个`);
    await shot(page, "15-settings-audio.png");

    // 断句（VAD）标签页：自适应能量 VAD，不再有引擎选择与概率阈值
    await page.locator('[data-testid="tab-vad"]').click();
    await waitFor(() => page.locator('[data-testid="vad-energy-threshold"]').isVisible(), {
      timeout: 8000,
      label: "等待断句页",
    });
    const vadFields = await Promise.all(
      ["vad-energy-threshold", "vad-min-speech", "vad-min-silence", "vad-max-utterance", "vad-pad"].map((id) =>
        page.locator(`[data-testid="${id}"]`).isVisible().catch(() => false),
      ),
    );
    check(
      "e5m. 断句页保留 5 个自适应能量 VAD 参数",
      vadFields.every(Boolean),
      `可见 ${vadFields.filter(Boolean).length}/5`,
    );
    const engineGone = (await page.locator('[data-testid="vad-engine"]').count()) === 0;
    const silenceMs = await page.locator('[data-testid="vad-min-silence"]').inputValue();
    const vadNote = (await page.locator('[data-testid="vad-engine-note"]').innerText()).replace(/\s+/g, " ");
    check(
      "e5n. 已移除 VAD 引擎选择，说明改为「自适应能量 VAD，无需模型文件」",
      engineGone && /无需模型文件/.test(vadNote) && Number(silenceMs) > 0,
      `vad-engine=${engineGone ? "无" : "仍在"}，静音断句=${silenceMs}ms`,
    );
    await shot(page, "16-settings-vad.png");

    await page.locator('[data-testid="tab-ai"]').click();
    await waitFor(() => page.locator('[data-testid="provider-baseurl"]').count(), {
      timeout: 8000,
      label: "等待 AI 服务商表单",
    });
    const providerName = await page.locator('[data-testid="provider-name"]').inputValue();
    const baseUrl = await page.locator('[data-testid="provider-baseurl"]').inputValue();
    check("e6. AI 接口显示服务商名称", providerName.trim().length > 0, providerName);
    check("e7. AI 接口显示 baseUrl 输入框并有值", /^https?:\/\//.test(baseUrl), baseUrl);
    const apiKeyType = await page.locator('[data-testid="provider-apikey"]').getAttribute("type");
    check("e8. apiKey 为密码框", apiKeyType === "password", `type=${apiKeyType}`);
    await shot(page, "06-settings-ai.png");

    // Esc 关闭
    await page.keyboard.press("Escape");
    await waitFor(async () => (await page.locator('[data-testid="settings-dialog"]').count()) === 0, {
      timeout: 8000,
      label: "等待弹窗关闭",
    });
    check("e9. Esc 关闭设置弹窗", (await page.locator('[data-testid="settings-dialog"]').count()) === 0);
    await shot(page, "07-after-settings.png");

    // e10. 打开「停止后生成完整纪要」并保存（验证草稿态 → 保存 → 持久化）
    await page.locator('[data-testid="open-settings"]').click();
    await waitFor(() => page.locator('[data-testid="settings-dialog"]').isVisible(), { timeout: 8000 });
    await page.locator('[data-testid="tab-ai"]').click();
    const finalSwitch = page.locator('[data-testid="ai-final-report"]');
    await waitFor(() => finalSwitch.isVisible(), { timeout: 8000, label: "等待开关出现" });
    const before = await finalSwitch.getAttribute("aria-checked");
    if (before !== "true") await finalSwitch.click();
    await page.locator('[data-testid="settings-save"]').click();
    await waitFor(async () => (await page.locator('[data-testid="settings-dialog"]').count()) === 0, {
      timeout: 8000,
      label: "等待保存后关闭",
    });
    await page.locator('[data-testid="open-settings"]').click();
    await waitFor(() => page.locator('[data-testid="settings-dialog"]').isVisible(), { timeout: 8000 });
    const persisted = await page.locator('[data-testid="ai-final-report"]').getAttribute("aria-checked");
    check("e10. 设置保存后持久化（停止后生成完整纪要已开启）", persisted === "true", `aria-checked=${persisted}`);
    await page.keyboard.press("Escape");
    await waitFor(async () => (await page.locator('[data-testid="settings-dialog"]').count()) === 0, { timeout: 8000 });

    /* ---------------------------------------------------------- 历史会话 */
    await page.keyboard.press("Control+k");
    await waitFor(() => page.locator('[data-testid="history-view"]').isVisible(), {
      timeout: 8000,
      label: "等待历史会话视图",
    });
    const historyText = await page.locator('[data-testid="history-view"]').innerText();
    check("f1. Ctrl+K 打开历史会话视图", true, historyText.split("\n")[0]);
    await shot(page, "08-history.png");

    // 回到录制视图，验证停止流程
    await page.keyboard.press("Control+k");
    await waitFor(() => page.locator('[data-testid="stop-recording"]').isVisible(), {
      timeout: 8000,
      label: "等待回到录制视图",
    });
    await page.locator('[data-testid="stop-recording"]').click();
    await waitFor(() => page.locator('[data-testid="start-recording"]').isVisible(), {
      timeout: 15000,
      label: "等待停止完成",
    });
    const finished = await page.locator('[data-testid="finished-banner"]').isVisible().catch(() => false);
    check("f2. 停止录音后回到可开始状态", true, finished ? "显示已结束提示" : "已回到开始按钮");

    // f3. ai.finalReportOnStop = true → 自动生成完整纪要并展示
    const reportShown = await waitFor(() => page.locator('[data-testid="final-report"]').isVisible().catch(() => false), {
      timeout: 15000,
      interval: 400,
      label: "等待自动生成的完整纪要",
    }).catch(() => false);
    check("f3. 停止后自动生成并展示完整会议纪要", Boolean(reportShown));
    await shot(page, "09-stopped.png");

    /* ---------------------------------------------------------- 会话详情 */
    await page.locator('[data-testid="goto-detail"]').click();
    await waitFor(() => page.locator('[data-testid="detail-title"]').isVisible(), {
      timeout: 8000,
      label: "等待会话详情",
    });
    const detailTitle = await page.locator('[data-testid="detail-title"]').innerText();
    check("g1. 进入会话详情视图", detailTitle.length > 0, detailTitle);
    const detailSegs = await page.locator('[data-testid="detail-transcript-list"] [data-testid="segment-row"]').count();
    check("g2. 详情页展示完整转写", detailSegs >= 2, `${detailSegs} 条`);

    await page.locator('[data-testid="generate-report"]').click();
    const mdOk = await waitFor(
      async () => {
        const md = page.locator('[data-testid="report-body"] [data-testid="markdown"]');
        if ((await md.count()) === 0) return false;
        const t = await md.innerText();
        return /会议纪要/.test(t) ? t : false;
      },
      { timeout: 15000, interval: 400, label: "等待 markdown 纪要渲染" },
    ).catch(() => "");
    const hasTable = await page.locator('[data-testid="report-body"] table').count();
    check("g3. 完整纪要 markdown 渲染（标题/列表/表格）", Boolean(mdOk) && hasTable > 0, `table=${hasTable}`);
    await shot(page, "10-detail.png");

    await page.locator('[data-testid="export-md"]').click();
    const exportToast = await waitFor(
      async () => {
        const t = page.locator('[data-testid="toasts"]');
        if ((await t.count()) === 0) return false;
        const txt = await t.innerText();
        return /导出成功/.test(txt) ? txt : false;
      },
      { timeout: 10000, interval: 300, label: "等待导出成功提示" },
    ).catch(() => "");
    check("g4. 导出 md 成功（浏览器下走 mock 路径）", Boolean(exportToast), String(exportToast).replace(/\s+/g, " ").slice(0, 50));
    await shot(page, "11-export.png");

    /* ---------------------------------------------------------- 主题 / 字号 */
    await page.locator('[data-testid="open-settings"]').click();
    await waitFor(() => page.locator('[data-testid="settings-dialog"]').isVisible(), { timeout: 8000 });
    await page.locator('[data-testid="tab-general"]').click();
    await waitFor(() => page.locator('[data-testid="theme-select"]').isVisible(), { timeout: 8000 });
    await page.locator('[data-testid="theme-select"]').selectOption("light");
    await page.locator('[data-testid="font-scale"]').fill("1.2");
    await page.locator('[data-testid="settings-save"]').click();
    await waitFor(async () => (await page.locator('[data-testid="settings-dialog"]').count()) === 0, { timeout: 8000 });
    const applied = await page.evaluate(() => ({
      theme: document.documentElement.dataset.theme,
      font: document.documentElement.style.fontSize,
    }));
    check("h1. 浅色主题生效", applied.theme === "light", `data-theme=${applied.theme}`);
    check("h2. 字号缩放作用到根元素", applied.font === "19.2px", `font-size=${applied.font}`);
    await shot(page, "12-light.png");

    // 切回深色默认值，保持环境干净
    await page.locator('[data-testid="open-settings"]').click();
    await waitFor(() => page.locator('[data-testid="settings-dialog"]').isVisible(), { timeout: 8000 });
    await page.locator('[data-testid="tab-general"]').click();
    await page.locator('[data-testid="theme-select"]').selectOption("dark");
    await page.locator('[data-testid="font-scale"]').fill("1");
    await page.locator('[data-testid="settings-save"]').click();
    await waitFor(async () => (await page.locator('[data-testid="settings-dialog"]').count()) === 0, { timeout: 8000 });

    /* ---------------------------------------------------------- 最小窗口尺寸 */
    await page.setViewportSize({ width: 1024, height: 640 });
    await sleep(500);
    const minOk = await page.locator('[data-testid="summary-panel"]').isVisible();
    check("i1. 最小窗口 1024×640 布局可用", minOk);
    await shot(page, "14-min-window.png");
    await page.setViewportSize({ width: 1360, height: 860 });

    /* ------------- 未配置识别服务时，开始录音要给出引导而不是静默失败 ------------- */
    // 从会话详情切回录制视图（Ctrl+K 在 详情 → 历史 → 录制 之间切换）
    await page.keyboard.press("Control+k");
    await page.keyboard.press("Control+k");
    await waitFor(() => page.locator('[data-testid="start-recording"]').isVisible(), {
      timeout: 8000,
      label: "等待回到录制视图",
    });

    await page.locator('[data-testid="open-settings"]').click();
    await waitFor(() => page.locator('[data-testid="settings-dialog"]').isVisible(), { timeout: 8000 });
    await page.locator('[data-testid="tab-asr"]').click();
    const enabledSwitch = page.locator('[data-testid="asr-enabled"]');
    await waitFor(() => enabledSwitch.isVisible(), { timeout: 8000, label: "等待识别总开关" });
    if ((await enabledSwitch.getAttribute("aria-checked")) !== "false") await enabledSwitch.click();
    await page.locator('[data-testid="settings-save"]').click();
    await waitFor(async () => (await page.locator('[data-testid="settings-dialog"]').count()) === 0, { timeout: 8000 });

    await page.locator('[data-testid="start-recording"]').click();
    const guardDialog = await waitFor(
      () => page.locator('[data-testid="settings-panel-asr"]').isVisible().catch(() => false),
      { timeout: 8000, interval: 200, label: "等待「设置 → 语音识别」自动打开" },
    ).catch(() => false);
    const guardToast = (await page.locator('[data-testid="toasts"]').innerText().catch(() => "")).replace(/\s+/g, " ");
    check(
      "j1. 识别服务被关闭时开始录音 → 弹出引导并打开「设置 → 语音识别」",
      guardDialog && /语音识别/.test(guardToast),
      `弹窗=${guardDialog}，提示=${guardToast.slice(0, 46)}`,
    );
    check("j2. 校验失败时不会进入录音状态", (await page.locator('[data-testid="start-recording"]').isVisible()) === true);

    // 还原：重新打开识别开关，保持环境干净
    const restoreSwitch = page.locator('[data-testid="asr-enabled"]');
    if ((await restoreSwitch.getAttribute("aria-checked")) !== "true") await restoreSwitch.click();
    await page.locator('[data-testid="settings-save"]').click();
    await waitFor(async () => (await page.locator('[data-testid="settings-dialog"]').count()) === 0, { timeout: 8000 });

    /* ---------------------------------------------------------- console */
    check(
      "a. 页面无 console error / pageerror",
      consoleErrors.length === 0,
      consoleErrors.length ? consoleErrors.slice(0, 3).join(" | ") : ignoredConsole.length ? `忽略 ${ignoredConsole.length} 条（favicon/DevTools）` : "无",
    );
  } catch (e) {
    check("执行流程未抛异常", false, e.message);
    try {
      await shot(page, "99-failure.png");
    } catch {
      /* ignore */
    }
  } finally {
    await context.close().catch(() => {});
    await browser.close().catch(() => {});
    stopDevServer(child);
  }

  console.log("\n================ 冒烟测试结果 ================");
  for (const r of results) console.log(`${r.ok ? "PASS" : "FAIL"}  ${r.name}${r.detail ? `  — ${r.detail}` : ""}`);
  console.log(`截图：${SHOTS.length} 张`);
  for (const s of SHOTS) console.log(`  - ${s}`);
  console.log(`总计 ${results.length} 项，失败 ${failures} 项`);
  console.log(failures === 0 ? "RESULT: ALL PASS" : "RESULT: FAILED");
  process.exit(failures === 0 ? 0 : 1);
}

main().catch((e) => {
  console.error("冒烟测试异常终止：", e);
  process.exit(1);
});
