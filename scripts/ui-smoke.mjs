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

// 探测候选地址：vite 绑定到 IPv4 还是 IPv6 取决于系统对 localhost 的解析顺序，
// 只探测一个地址会在部分环境（例如 GitHub Actions runner）上永远连不上。
const PROBE_URLS = [BASE, "http://127.0.0.1:1420", "http://[::1]:1420", "http://localhost:1420"];

async function probeOnce(url) {
  try {
    const r = await fetch(url, { signal: AbortSignal.timeout(1500) });
    return r.ok || r.status === 404;
  } catch {
    return false;
  }
}

async function portAlive() {
  for (const url of PROBE_URLS) {
    if (await probeOnce(url)) return true;
  }
  return false;
}

// 超时时把每个候选地址的结果都打出来，避免只看到一句「等待超时」
async function probeReport() {
  const lines = [];
  for (const url of PROBE_URLS) {
    lines.push(`  ${url} → ${(await probeOnce(url)) ? "可达" : "不可达"}`);
  }
  return lines.join("\n");
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
    console.log("[setup] 候选地址探测结果：\n" + (await probeReport()));
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

/**
 * 内录引导卡片：用一个独立 context 打开「指定平台 + 没有内录源」的 mock 环境，
 * 验证「设置 → 音频设备」里出现对应平台的安装引导、命令与「重新检测设备」。
 * （虚拟声卡是系统级驱动，应用无法内置安装，所以这里只能验证引导本身。）
 */
async function checkLoopbackGuide(browser, platform, expectCmd, shotName) {
  const ctx = await browser.newContext({
    viewport: { width: 1360, height: 860 },
    permissions: ["clipboard-read", "clipboard-write"],
  });
  const p = await ctx.newPage();
  const errs = [];
  p.on("console", (m) => {
    if (m.type() === "error") errs.push(m.text());
  });
  p.on("pageerror", (e) => errs.push(`pageerror: ${e.message}`));
  try {
    await p.goto(`${BASE}/?mockPlatform=${platform}&mockLoopback=none&mockOnboarding=1`, {
      waitUntil: "domcontentloaded",
    });
    await waitFor(() => p.locator('[data-testid="sidebar"]').isVisible().catch(() => false), {
      timeout: 20000,
      label: `等待 ${platform} 页面启动`,
    });
    await p.locator('[data-testid="sidebar-settings"]').click();
    await waitFor(() => p.locator('[data-testid="settings-dialog"]').isVisible(), { timeout: 8000 });
    await p.locator('[data-testid="tab-audio"]').click();
    const shown = await waitFor(
      () => p.locator('[data-testid="loopback-guide"]').isVisible().catch(() => false),
      { timeout: 8000, interval: 200, label: `等待 ${platform} 内录引导卡片` },
    ).catch(() => false);
    const attr = await p.locator('[data-testid="loopback-guide"]').getAttribute("data-platform").catch(() => "");
    const guideText = (await p.locator('[data-testid="loopback-guide"]').innerText().catch(() => "")).replace(/\s+/g, " ");
    await shot(p, shotName);
    check(
      `o-${platform}. 没有内录源时显示 ${platform} 平台的内录引导卡片`,
      shown && attr === platform && guideText.includes(expectCmd.split(" ").slice(0, 2).join(" ")),
      `卡片=${shown}，平台=${attr}`,
    );

    if (platform === "macos") {
      await p.locator('[data-testid="loopback-guide-open-page"]').click();
      const openedUrls = await p.evaluate(() => window.__MEETING_HEAR_OPENED_URLS__ ?? []);
      check(
        "o-macos-2. 引导卡片可一键打开 BlackHole 下载页（api.openUrl）",
        openedUrls.some((u) => u.includes("existential.audio/blackhole")),
        openedUrls.join(" | ") || "（未记录到调用）",
      );
      check(
        "o-macos-3. 卡片包含「音频 MIDI 设置 → 多输出设备」三步说明",
        /音频 MIDI 设置/.test(guideText) && /多输出设备/.test(guideText) && /系统输出/.test(guideText),
        guideText.slice(0, 56) + "…",
      );
    }

    await p.locator('[data-testid="loopback-guide-refresh"]').click();
    const refreshed = await waitFor(
      async () => {
        const t = await p.locator('[data-testid="toasts"]').innerText().catch(() => "");
        return /音频设备/.test(t) ? t.replace(/\s+/g, " ") : "";
      },
      { timeout: 8000, interval: 250, label: "等待重新检测结果" },
    ).catch(() => "");
    check(`o-${platform}-4. 「重新检测设备」重新调用 list_audio_sources`, Boolean(refreshed), String(refreshed).slice(0, 46));
    check(`o-${platform}-5. 引导页无 console error`, errs.length === 0, errs.slice(0, 2).join(" | ") || "无");
  } finally {
    await ctx.close().catch(() => {});
  }
}

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

    /* ============================================================
     * 首次运行配置向导（settings.general.onboardingCompleted = false）
     * ========================================================== */
    await waitFor(() => page.locator('[data-testid="onboarding-wizard"]').isVisible().catch(() => false), {
      timeout: 20000,
      interval: 200,
      label: "等待首次运行配置向导",
    });
    const obTitle = await page.locator('[data-testid="onboarding-title"]').innerText();
    check("k1. 首次进入（onboardingCompleted=false）弹出全屏配置向导", /向导/.test(obTitle), obTitle);

    const privacy = (await page.locator('[data-testid="onboarding-privacy"]').innerText()).replace(/\s+/g, " ");
    check(
      "k2. 第 1 步明确提示「音频会发送到你配置的识别服务」（隐私）",
      /音频/.test(privacy) && /识别服务/.test(privacy) && /不会出本机/.test(privacy) && /whisper\.cpp/i.test(privacy),
      privacy.slice(0, 56) + "…",
    );
    const obSteps = await page.locator('[data-testid="onboarding-step-indicator"] li').count();
    check("k2b. 向导有步骤指示器（5 步）", obSteps === 5, `${obSteps} 步`);
    await shot(page, "17-onboarding-1-welcome.png");

    /* ---- 第 2 步：语音识别服务 ---- */
    await page.locator('[data-testid="onboarding-next"]').click();
    await waitFor(() => page.locator('[data-testid="onboarding-asr-baseurl"]').isVisible().catch(() => false), {
      timeout: 8000,
      interval: 200,
      label: "等待识别服务步骤",
    });
    const obBaseUrl = await page.locator('[data-testid="onboarding-asr-baseurl"]').inputValue();
    const obTestBtn = await page.locator('[data-testid="onboarding-asr-test"]').isVisible();
    check(
      "k3. 第 2 步有 Base URL 输入框与「测试连接」按钮",
      /^https?:\/\//.test(obBaseUrl) && obTestBtn,
      `baseUrl=${obBaseUrl}，测试按钮=${obTestBtn}`,
    );

    await page.locator('[data-testid="onboarding-asr-preset"]').selectOption("local-whispercpp");
    await waitFor(() => page.locator('[data-testid="onboarding-asr-local-hint"]').isVisible().catch(() => false), {
      timeout: 5000,
      label: "等待本地 whisper.cpp 提示",
    });
    const localHint = (await page.locator('[data-testid="onboarding-asr-local-hint"]').innerText()).replace(/\s+/g, " ");
    check(
      "k4. 本地 whisper.cpp 预设给出启动命令示例与「端口要和 Base URL 一致」提示",
      /whisper-server/.test(localHint) && /8080/.test(localHint) && /端口/.test(localHint) && /ggml-base\.bin/.test(localHint),
      localHint.slice(0, 72) + "…",
    );
    await shot(page, "18-onboarding-2-asr-local.png");

    await page.locator('[data-testid="onboarding-asr-test"]').click();
    const obAsrOk = await waitFor(
      async () => {
        const el = page.locator('[data-testid="onboarding-asr-test-result"]');
        if ((await el.count()) === 0) return "";
        const t = (await el.innerText()).replace(/\s+/g, " ");
        return /连接成功/.test(t) ? t : "";
      },
      { timeout: 8000, interval: 200, label: "等待向导内识别测试成功反馈" },
    ).catch(() => "");
    check(
      "k5. 向导内「测试连接」成功时显示延迟与「空文本属正常」说明",
      /连接成功/.test(obAsrOk) && /延迟/.test(obAsrOk) && /静音/.test(obAsrOk),
      obAsrOk.slice(0, 60) + "…",
    );

    // 失败路径：清空 Base URL 后应当显示后端返回的中文错误
    await page.locator('[data-testid="onboarding-asr-baseurl"]').fill("");
    await page.locator('[data-testid="onboarding-asr-test"]').click();
    const obAsrBad = await waitFor(
      async () => {
        const el = page.locator('[data-testid="onboarding-asr-test-result"]');
        if ((await el.count()) === 0) return "";
        const t = (await el.innerText()).replace(/\s+/g, " ");
        return /连接失败/.test(t) ? t : "";
      },
      { timeout: 8000, interval: 200, label: "等待向导内识别测试失败反馈" },
    ).catch(() => "");
    check(
      "k6. 测试失败时显示后端返回的中文错误（Base URL 未配置）",
      /连接失败/.test(obAsrBad) && /Base URL/.test(obAsrBad),
      obAsrBad.slice(0, 60) + "…",
    );

    // 切回云端预设，并验证向导里可以直接打开服务商控制台
    await page.locator('[data-testid="onboarding-asr-preset"]').selectOption("groq");
    await waitFor(async () => (await page.locator('[data-testid="onboarding-asr-open-console"] button').count()) > 0, {
      timeout: 5000,
      label: "等待服务商控制台链接",
    });
    await page.locator('[data-testid="onboarding-asr-open-console"] button').first().click();
    const opened = await page.evaluate(() => window.__MEETING_HEAR_OPENED_URLS__ ?? []);
    check(
      "k7. 向导内可用 api.openUrl 打开服务商控制台（Groq key 申请页）",
      opened.some((u) => u.includes("console.groq.com")),
      opened.join(" | ") || "（未记录到调用）",
    );

    /* ---- 第 3 步：AI 接口 ---- */
    await page.locator('[data-testid="onboarding-next"]').click();
    await waitFor(() => page.locator('[data-testid="onboarding-ai-baseurl"]').isVisible().catch(() => false), {
      timeout: 8000,
      interval: 200,
      label: "等待 AI 接口步骤",
    });
    const obAiBase = await page.locator('[data-testid="onboarding-ai-baseurl"]').inputValue();
    const obAiModel = await page.locator('[data-testid="onboarding-ai-model"]').inputValue();
    check(
      "k8. 第 3 步 AI 接口有 Base URL / 模型名 / 测试连接",
      /^https?:\/\//.test(obAiBase) && obAiModel.trim().length > 0 && (await page.locator('[data-testid="onboarding-ai-test"]').isVisible()),
      `${obAiBase} · ${obAiModel}`,
    );
    const ollamaHint = (await page.locator('[data-testid="onboarding-ai-ollama-hint"]').innerText()).replace(/\s+/g, " ");
    check(
      "k8b. 说明本地 Ollama 用法（http://localhost:11434/v1 + 已 pull 的模型名）",
      /Ollama/.test(ollamaHint) && /11434/.test(ollamaHint) && /ollama pull/.test(ollamaHint),
      ollamaHint.slice(0, 64) + "…",
    );
    await page.locator('[data-testid="onboarding-ai-test"]').click();
    const obAiOk = await waitFor(
      async () => {
        const el = page.locator('[data-testid="onboarding-ai-test-result"]');
        if ((await el.count()) === 0) return "";
        const t = (await el.innerText()).replace(/\s+/g, " ");
        return /连接成功/.test(t) ? t : "";
      },
      { timeout: 8000, interval: 200, label: "等待向导内 AI 测试结果" },
    ).catch(() => "");
    check("k9. 向导内 AI 接口「测试连接」给出成功反馈与延迟", /连接成功/.test(obAiOk) && /延迟/.test(obAiOk), obAiOk.slice(0, 56));
    await shot(page, "19-onboarding-3-ai.png");

    /* ---- 第 4 步：音频设备 ---- */
    await page.locator('[data-testid="onboarding-next"]').click();
    await waitFor(() => page.locator('[data-testid="onboarding-audio-list"]').isVisible().catch(() => false), {
      timeout: 8000,
      interval: 200,
      label: "等待音频设备步骤",
    });
    const obDevices = await page.locator('[data-testid="onboarding-audio-list"] .pick-row').count();
    const obLoopRows = await page.locator('[data-testid^="onboarding-loopback-"]').count();
    check(
      "k10. 第 4 步列出麦克风与系统内录来源（可勾选默认设备）",
      obDevices >= 3 && obLoopRows >= 1,
      `设备行 ${obDevices}，内录候选 ${obLoopRows}`,
    );
    await shot(page, "20-onboarding-4-audio.png");

    /* ---- 第 5 步：完成 ---- */
    await page.locator('[data-testid="onboarding-next"]').click();
    await waitFor(() => page.locator('[data-testid="onboarding-done-summary"]').isVisible().catch(() => false), {
      timeout: 8000,
      interval: 200,
      label: "等待完成步骤",
    });
    const doneSummary = (await page.locator('[data-testid="onboarding-done-summary"]').innerText()).replace(/\s+/g, " ");
    check(
      "k11. 第 5 步汇总配置并说明「随时可以在设置里改」",
      /语音识别服务/.test(doneSummary) && /AI 接口/.test(doneSummary) && /音频来源/.test(doneSummary),
      doneSummary.slice(0, 64) + "…",
    );
    await shot(page, "21-onboarding-5-done.png");

    await page.locator('[data-testid="onboarding-finish"]').click();
    await waitFor(async () => (await page.locator('[data-testid="onboarding-wizard"]').count()) === 0, {
      timeout: 10000,
      interval: 200,
      label: "等待向导关闭",
    });
    await waitFor(() => page.locator('[data-testid="sidebar"]').isVisible().catch(() => false), {
      timeout: 8000,
      label: "等待主界面（左侧历史栏）",
    });
    check("k12. 完成向导后进入主界面，左侧出现历史 sidebar", await page.locator('[data-testid="sidebar"]').isVisible());
    check("k12b. 向导期间没有开始录音（状态仍为空闲）", /空闲/.test(await page.locator('[data-testid="status-pill"]').innerText()));

    // 刷新页面：向导不应再出现（onboardingCompleted 已持久化）
    await page.reload({ waitUntil: "domcontentloaded" });
    await waitFor(() => page.locator('[data-testid="sidebar"]').isVisible().catch(() => false), {
      timeout: 20000,
      label: "等待刷新后启动完成",
    });
    await sleep(600);
    check(
      "k13. 刷新页面后向导不再出现（onboardingCompleted 已保存）",
      (await page.locator('[data-testid="onboarding-wizard"]').count()) === 0,
    );
    const emptyGuide = (await page.locator('[data-testid="sidebar-empty"]').innerText().catch(() => "")).replace(/\s+/g, " ");
    check(
      "k14. 空历史时 sidebar 给出友好引导（点「新建会议」开始）",
      /还没有会议记录/.test(emptyGuide) && /新建会议/.test(emptyGuide),
      emptyGuide.slice(0, 48),
    );
    await shot(page, "22-sidebar-empty.png");

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

    // c1b. 可疑结果必须「照样显示」。
    // 这是踩过的真实事故：模型和语言对不上时返回英文幻觉，被过滤器直接丢掉，
    // 界面上一个字都没有，用户以为程序坏了。可疑只能标注，不能删除。
    const suspectRows = page.locator('[data-testid="transcript-list"] [data-suspect="1"]');
    const suspectRowCount = await suspectRows.count();
    let suspectText = "";
    let suspectBadge = 0;
    if (suspectRowCount > 0) {
      suspectText = (await suspectRows.first().locator(".t-text").innerText()).trim();
      suspectBadge = await suspectRows.first().locator('[data-testid="segment-suspect"]').count();
    } else {
      await waitFor(
        async () => ((await suspectRows.count()) > 0 ? 1 : 0),
        { timeout: 20000, interval: 500, label: "等待可疑段落出现" },
      ).catch(() => 0);
      if ((await suspectRows.count()) > 0) {
        suspectText = (await suspectRows.first().locator(".t-text").innerText()).trim();
        suspectBadge = await suspectRows.first().locator('[data-testid="segment-suspect"]').count();
      }
    }
    check(
      "c1b. 可疑段落照常出现在转写里（不被静默丢弃）",
      suspectText.length > 0,
      suspectText ? `内容：${suspectText.slice(0, 30)}` : "没有找到可疑段落",
    );
    check("c1c. 可疑段落带可见标记说明原因", suspectBadge === 1, `标记数=${suspectBadge}`);
    const suspectCounter = await page.locator('[data-testid="transcript-suspect-count"]').count();
    check("c1d. 工具栏汇总可疑条数", suspectCounter === 1, `计数元素=${suspectCounter}`);

    await shot(page, "03-segments.png");

    // 正在录音的会话置顶并高亮（红点）
    const liveRow = page.locator('[data-testid="sidebar-list"] .sb-row.recording');
    const liveCount = await liveRow.count();
    const liveDot = await liveRow.first().locator(".rec-dot").count().catch(() => 0);
    const liveFirst = await page
      .locator('[data-testid="sidebar-list"] .sb-row')
      .first()
      .evaluate((el) => el.className.includes("recording"))
      .catch(() => false);
    check(
      "c10. sidebar 里正在录音的会话置顶并高亮（带红点）",
      liveCount === 1 && liveDot === 1 && liveFirst,
      `录音中条目 ${liveCount}，红点 ${liveDot}，置顶 ${liveFirst}`,
    );
    await shot(page, "23-sidebar-recording.png");

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

    // 指标条做过精简：只留「识别 / 延迟 / 字数」，RTF 与句数收进延迟的悬停提示
    const metrics = await page.locator('[data-testid="metrics"]').innerText().catch(() => "");
    check(
      "c4. 指标条只显示识别 / 延迟 / 字数（已去掉 RTF、句数、音频时长）",
      /识别/.test(metrics) && /延迟/.test(metrics) && /字数/.test(metrics)
        && !/RTF/.test(metrics) && !/句数/.test(metrics) && !/音频/.test(metrics),
      metrics.replace(/\s+/g, " ").slice(0, 70),
    );
    const latencyTitle = await page.locator('[data-testid="metric-latency"]').getAttribute("title").catch(() => "");
    check("c4a. RTF 与句数仍在延迟的悬停提示里（信息没丢，只是不占位）", /RTF/.test(latencyTitle ?? "") && /句/.test(latencyTitle ?? ""), latencyTitle ?? "");
    const serviceMetric = await page.locator('[data-testid="metric-asr-service"]').innerText().catch(() => "");
    check(
      "c4b. 指标条显示识别服务（服务商 · 模型）而非本地模型名",
      /·/.test(serviceMetric) && /whisper|SenseVoice|faster-whisper/i.test(serviceMetric),
      serviceMetric,
    );

    // 分区是「有内容才渲染」的，所以这里等纪要累积出来（mock 的待办要等第 7 句）
    const summaryPanel = await waitFor(
      async () => {
        const t = await page.locator('[data-testid="summary-panel"]').innerText().catch(() => "");
        return /会议总览/.test(t) && /纪要要点/.test(t) && /待办事项/.test(t) ? t : false;
      },
      { timeout: 40000, interval: 500, label: "等待纪要三块齐全" },
    ).catch(() => "");
    check(
      "c5. 纪要面板渲染总览 / 纪要要点 / 待办三块（有内容才渲染）",
      Boolean(summaryPanel),
      String(summaryPanel).replace(/\s+/g, " ").slice(0, 80),
    );
    // 精简的核心诉求：空的区块不再渲染，也不出现「暂无」占位文字
    check(
      "c5b. 已合并的分区不再重复展示（无关键要点/已达成的决定/当前主题三块，且没有「暂无」占位）",
      !/关键要点/.test(summaryPanel) && !/已达成的决定/.test(summaryPanel)
        && !/当前主题/.test(summaryPanel) && !/暂无/.test(summaryPanel),
      summaryPanel.replace(/\s+/g, " ").slice(0, 80),
    );
    // 底部状态行只在落后时出现，平时不占一行
    const lagVisible = await page.locator('[data-testid="summary-lag"]').count();
    check("c5c. 纪要及时跟上了就不显示状态行（不再常驻「纪要已跟上转写」）", lagVisible === 0, `lag 元素 ${lagVisible} 个`);
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
    await page.locator('[data-testid="sidebar-settings"]').click();
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

    // 提示块默认只留一行结论，离线自建的命令收进可折叠详情里
    const noticeText = (await page.locator('[data-testid="asr-service-notice"]').innerText()).replace(/\s+/g, " ");
    check(
      "e3b. 提示块一行说明「本应用不含识别模型，需要外部识别服务」",
      /不含识别模型/.test(noticeText) && /外部识别服务/.test(noticeText),
      noticeText.slice(0, 48) + "…",
    );
    check(
      "e3b2. 离线自建命令已收起（默认不在提示块正文里，减少常驻文字）",
      !/whisper-server -m/.test(noticeText),
      noticeText.slice(0, 40) + "…",
    );
    // 展开详情后命令应当可见 —— 信息没丢，只是不占屏
    await page.locator('[data-testid="asr-service-notice"] details').first().evaluate((el) => {
      el.open = true;
    });
    const expanded = (await page.locator('[data-testid="asr-service-notice"]').innerText()).replace(/\s+/g, " ");
    check(
      "e3b3. 展开「详情」后能看到 whisper.cpp / faster-whisper 的启动命令",
      /whisper-server -m/.test(expanded) && /faster-whisper-server/.test(expanded),
      expanded.slice(0, 60) + "…",
    );
    await page.locator('[data-testid="asr-service-notice"] details').first().evaluate((el) => {
      el.open = false;
    });
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
    // 识别语言属于识别服务的事，应用里不应出现该设置项
    const langSelectors = await page.locator('[data-testid="asr-language"]').count();
    const translateSwitches = await page.locator('[data-testid="asr-translate-english"]').count();
    check(
      "e5g. 应用不提供「识别语言/翻译成英文」设置（语言归识别服务管）",
      langSelectors === 0 && translateSwitches === 0,
      `language=${langSelectors} translate=${translateSwitches}`,
    );

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
    await page.locator('[data-testid="sidebar-settings"]').click();
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
    await page.locator('[data-testid="sidebar-settings"]').click();
    await waitFor(() => page.locator('[data-testid="settings-dialog"]').isVisible(), { timeout: 8000 });
    const persisted = await page.locator('[data-testid="ai-final-report"]').getAttribute("aria-checked");
    check("e10. 设置保存后持久化（停止后生成完整纪要已开启）", persisted === "true", `aria-checked=${persisted}`);
    await page.keyboard.press("Escape");
    await waitFor(async () => (await page.locator('[data-testid="settings-dialog"]').count()) === 0, { timeout: 8000 });

    /* ---------------------------------------------------------- 历史（sidebar + Ctrl+K） */
    // Ctrl+K 不再是「打开历史视图」，而是聚焦左侧历史栏的搜索框
    await page.keyboard.press("Control+k");
    const focused = await waitFor(
      async () => {
        const id = await page.evaluate(() => document.activeElement?.getAttribute("data-testid") ?? "");
        return id === "sidebar-search" ? id : "";
      },
      { timeout: 5000, interval: 150, label: "等待 Ctrl+K 聚焦 sidebar 搜索框" },
    ).catch(() => "");
    check(
      "f1. Ctrl+K 聚焦 sidebar 搜索框（不再打开独立历史视图）",
      focused === "sidebar-search" && (await page.locator('[data-testid="history-view"]').count()) === 0,
      `activeElement=${focused || "未聚焦"}`,
    );

    // 搜索框可用：输入关键字过滤历史列表
    await page.locator('[data-testid="sidebar-search"]').fill("会议");
    await sleep(300);
    const searched = await page.locator('[data-testid="sidebar-list"] .sb-row').count();
    await page.locator('[data-testid="sidebar-search"]').fill("");
    await sleep(300);
    check("f1b. sidebar 搜索框可过滤历史会话", searched >= 1, `匹配 ${searched} 条`);

    // 回到录制视图，验证停止流程（详情页有「返回当前会议」，录制视图本来就在）
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

    /* ---------------------------------------------------------- 左侧历史 sidebar */
    const sbRows = page.locator('[data-testid="sidebar-list"] .sb-row');
    const rowCount = await waitFor(async () => {
      const n = await sbRows.count();
      return n >= 1 ? n : 0;
    }, { timeout: 10000, interval: 300, label: "等待 sidebar 列出历史会话" }).catch(() => 0);
    check("l1. 主界面左侧 sidebar 列出历史会话", (await page.locator('[data-testid="sidebar"]').isVisible()) && rowCount >= 1, `${rowCount} 条`);

    const rowText = (await sbRows.first().innerText()).replace(/\s+/g, " ");
    const rowStats = await sbRows.first().locator(".sb-stats").count();
    check(
      "l2. 每条显示标题 / 创建时间 / 时长 / 段数字数 / 是否有纪要",
      /段数/.test(rowText) && /字数/.test(rowText) && /纪要/.test(rowText) && /\d{4}-\d{2}-\d{2}/.test(rowText) && rowStats === 1,
      rowText.slice(0, 64),
    );

    const sbId = ((await sbRows.first().locator('[data-testid^="sidebar-open-"]').getAttribute("data-testid")) ?? "").replace("sidebar-open-", "");
    await sbRows.first().hover();
    await sleep(250);
    await shot(page, "24-sidebar-hover-actions.png");
    check(
      "l3. hover 历史条目显示重命名 / 删除入口",
      (await page.locator(`[data-testid="sidebar-rename-${sbId}"]`).count()) === 1 &&
        (await page.locator(`[data-testid="sidebar-delete-${sbId}"]`).count()) === 1,
      `条目 ${sbId}`,
    );

    // 重命名（复用原历史视图的那套逻辑与二次确认）
    await page.locator(`[data-testid="sidebar-rename-${sbId}"]`).click();
    await waitFor(() => page.locator(`[data-testid="sidebar-rename-input-${sbId}"]`).isVisible(), { timeout: 5000 });
    await page.locator(`[data-testid="sidebar-rename-input-${sbId}"]`).fill("产品周会（改名验证）");
    await page.locator(`[data-testid="sidebar-rename-input-${sbId}"]`).press("Enter");
    const renamed = await waitFor(async () => {
      const t = await sbRows.first().innerText();
      return /改名验证/.test(t) ? t.replace(/\s+/g, " ") : "";
    }, { timeout: 8000, interval: 200, label: "等待重命名生效" }).catch(() => "");
    check("l4. sidebar 重命名会话", Boolean(renamed), renamed.slice(0, 40));

    // 点击历史条目 → 打开详情 → 返回当前会议
    await page.locator(`[data-testid="sidebar-open-${sbId}"]`).click();
    await waitFor(() => page.locator('[data-testid="detail-title"]').isVisible(), { timeout: 8000, label: "等待会话详情" });
    const sidebarDetailTitle = await page.locator('[data-testid="detail-title"]').innerText();
    check("l5. 点击历史条目在主区域打开会话详情", /改名验证/.test(sidebarDetailTitle), sidebarDetailTitle);
    await page.locator('[data-testid="detail-back"]').click();
    const backHome = await waitFor(
      () => page.locator('[data-testid="start-recording"]').isVisible().catch(() => false),
      { timeout: 8000, interval: 200, label: "等待返回当前会议" },
    ).catch(() => false);
    check("l6. 详情页「返回当前会议」回到主界面", backHome);

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

    /* ---------------------------------------------------------- sidebar 删除（二次确认） */
    await page.locator('[data-testid="sidebar-list"] .sb-row').first().hover();
    await page.locator(`[data-testid="sidebar-delete-${sbId}"]`).click();
    const confirmShown = await waitFor(
      () => page.locator(`[data-testid="sidebar-confirm-${sbId}"]`).isVisible().catch(() => false),
      { timeout: 5000, interval: 200, label: "等待删除确认" },
    ).catch(() => false);
    await page.locator(`[data-testid="sidebar-delete-confirm-${sbId}"]`).click();
    const emptied = await waitFor(
      () => page.locator('[data-testid="sidebar-empty"]').isVisible().catch(() => false),
      { timeout: 8000, interval: 200, label: "等待列表清空" },
    ).catch(() => false);
    check("l7. sidebar 删除会话有二次确认，删除后回到空历史引导", confirmShown && emptied);

    /* ---------------------------------------------------------- 主题 / 字号 */
    await page.locator('[data-testid="sidebar-settings"]').click();
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
    await page.locator('[data-testid="sidebar-settings"]').click();
    await waitFor(() => page.locator('[data-testid="settings-dialog"]').isVisible(), { timeout: 8000 });
    await page.locator('[data-testid="tab-general"]').click();
    await page.locator('[data-testid="theme-select"]').selectOption("dark");
    await page.locator('[data-testid="font-scale"]').fill("1");
    await page.locator('[data-testid="settings-save"]').click();
    await waitFor(async () => (await page.locator('[data-testid="settings-dialog"]').count()) === 0, { timeout: 8000 });

    /* ---------------------------------------------------------- sidebar 折叠 */
    // 宽窗口：手动折叠 → 记忆到 localStorage
    await page.locator('[data-testid="toggle-sidebar"]').click();
    await waitFor(
      async () => (await page.locator('[data-testid="sidebar"]').getAttribute("data-collapsed")) === "true",
      { timeout: 5000, interval: 150, label: "等待 sidebar 折叠" },
    ).catch(() => {});
    const collapsedAttr = await page.locator('[data-testid="sidebar"]').getAttribute("data-collapsed");
    const collapsedStored = await page.evaluate(() => localStorage.getItem("meeting-hear:sidebar-collapsed"));
    const collapsedKeepsNew = await page.locator('[data-testid="sidebar-new-meeting"]').isVisible();
    const collapsedHidesSearch = (await page.locator('[data-testid="sidebar-search"]').count()) === 0;
    check(
      "m1. 折叠 sidebar 为窄条（保留新建按钮，隐藏搜索框）",
      collapsedAttr === "true" && collapsedKeepsNew && collapsedHidesSearch,
      `data-collapsed=${collapsedAttr}，新建按钮=${collapsedKeepsNew}，搜索框已隐藏=${collapsedHidesSearch}`,
    );
    check("m2. 折叠状态记忆在 localStorage", collapsedStored === "1", `meeting-hear:sidebar-collapsed=${collapsedStored}`);

    // 展开后 Ctrl+K 仍然能聚焦搜索框
    await page.locator('[data-testid="toggle-sidebar"]').click();
    await sleep(200);
    await page.keyboard.press("Control+k");
    const focusAgain = await page.evaluate(() => document.activeElement?.getAttribute("data-testid") ?? "");
    check("m3. 展开后 Ctrl+K 依旧聚焦搜索框", focusAgain === "sidebar-search", `activeElement=${focusAgain}`);
    await page.locator('[data-testid="sidebar-search"]').blur().catch(() => {});

    /* ---------------------------------------------------------- 最小窗口尺寸 */
    await page.setViewportSize({ width: 1024, height: 640 });
    await sleep(500);
    const minOk = await page.locator('[data-testid="summary-panel"]').isVisible();
    check("i1. 最小窗口 1024×640 布局可用", minOk);
    const narrowCollapsed = await page.locator('[data-testid="sidebar"]').getAttribute("data-collapsed");
    check("i2. 窗口 < 1100px 时 sidebar 自动折叠为窄条", narrowCollapsed === "true", `data-collapsed=${narrowCollapsed}`);
    await shot(page, "25-sidebar-collapsed.png");
    await shot(page, "14-min-window.png");
    await page.setViewportSize({ width: 1360, height: 860 });
    await sleep(300);

    /* ------------- 未配置识别服务时，开始录音要给出引导而不是静默失败 ------------- */
    // 从会话详情点「返回当前会议」回到录制视图
    await page.locator('[data-testid="detail-back"]').click();
    await waitFor(() => page.locator('[data-testid="start-recording"]').isVisible(), {
      timeout: 8000,
      label: "等待回到录制视图",
    });

    await page.locator('[data-testid="sidebar-settings"]').click();
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

    // j3. sidebar 的「新建会议」走同一条校验路径（而不是静默失败）
    await page.keyboard.press("Escape");
    await waitFor(async () => (await page.locator('[data-testid="settings-dialog"]').count()) === 0, { timeout: 8000 });
    await page.locator('[data-testid="sidebar-new-meeting"]').click();
    const guardDialog2 = await waitFor(
      () => page.locator('[data-testid="settings-panel-asr"]').isVisible().catch(() => false),
      { timeout: 8000, interval: 200, label: "等待「新建会议」触发引导" },
    ).catch(() => false);
    const guardToast2 = (await page.locator('[data-testid="toasts"]').innerText().catch(() => "")).replace(/\s+/g, " ");
    check(
      "j3. 识别服务不可用时点 sidebar「新建会议」→ 触发引导并打开设置（未静默失败）",
      guardDialog2 && /语音识别/.test(guardToast2),
      `弹窗=${guardDialog2}，提示=${guardToast2.slice(0, 46)}`,
    );

    // 还原：重新打开识别开关，保持环境干净
    await page.locator('[data-testid="tab-asr"]').click();
    const restoreSwitch = page.locator('[data-testid="asr-enabled"]');
    if ((await restoreSwitch.getAttribute("aria-checked")) !== "true") await restoreSwitch.click();
    await page.locator('[data-testid="settings-save"]').click();
    await waitFor(async () => (await page.locator('[data-testid="settings-dialog"]').count()) === 0, { timeout: 8000 });

    /* ------------- 设置 → 通用 → 重新运行配置向导 ------------- */
    await page.locator('[data-testid="sidebar-settings"]').click();
    await waitFor(() => page.locator('[data-testid="settings-dialog"]').isVisible(), { timeout: 8000 });
    await page.locator('[data-testid="tab-general"]').click();
    await waitFor(() => page.locator('[data-testid="rerun-onboarding"]').isVisible(), { timeout: 8000 });
    await page.locator('[data-testid="rerun-onboarding"]').click();
    const wizardAgain = await waitFor(
      () => page.locator('[data-testid="onboarding-wizard"]').isVisible().catch(() => false),
      { timeout: 8000, interval: 200, label: "等待重新打开向导" },
    ).catch(() => false);
    check(
      "n1. 设置 → 通用「重新运行配置向导」会重新打开向导",
      wizardAgain && (await page.locator('[data-testid="settings-dialog"]').count()) === 0,
    );
    // 走完向导，保持环境干净
    await page.locator('[data-testid="onboarding-skip-all"]').click();
    await waitFor(async () => (await page.locator('[data-testid="onboarding-wizard"]').count()) === 0, {
      timeout: 8000,
      interval: 200,
      label: "等待向导关闭",
    });
    check("n2. 向导「跳过」也会把 onboardingCompleted 置为 true（不再弹出）", true);

    /* ------------- 各平台的内录引导卡片（用 mock 参数模拟平台与「无内录源」） ------------- */
    await checkLoopbackGuide(browser, "macos", "brew install blackhole-2ch", "26-loopback-guide-macos.png");
    await checkLoopbackGuide(browser, "linux", "sudo apt install pulseaudio-utils", "27-loopback-guide-linux.png");

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
