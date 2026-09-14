#!/usr/bin/env bash
# 一键启停本地测试用的两个服务：whisper.cpp server（识别）+ Ollama（纪要）
#
#   ./scripts/dev-services.sh start     启动两个服务并做连通性检查
#   ./scripts/dev-services.sh status    查看状态与当前配置
#   ./scripts/dev-services.sh stop      停止由本脚本启动的服务
#
# 应用里的配置（与脚本默认值一致）：
#   识别：Base URL http://127.0.0.1:8090  路径 /inference        模型 whisper-1（随便填非空）
#   纪要：Base URL http://127.0.0.1:11434/v1                     模型 见 ollama list

set -uo pipefail

WHISPER_DIR="${WHISPER_DIR:-$HOME/whisper-server}"
WHISPER_BIN="${WHISPER_BIN:-$WHISPER_DIR/build/bin/whisper-server}"
# 识别语言由**服务端**决定，应用不参与。
# whisper-server 的 language 默认值是 en，不显式设成 auto 的话，
# 中文语音会被按英文硬识别，吐出一段英文幻觉。
WHISPER_LANG="${WHISPER_LANG:-auto}"
WHISPER_MODEL="${WHISPER_MODEL:-$WHISPER_DIR/models/ggml-base.bin}"
WHISPER_PORT="${WHISPER_PORT:-8090}"
OLLAMA_PORT="${OLLAMA_PORT:-11434}"
# 默认监听 0.0.0.0，方便从别的机器（例如 Mac）连过来测试；
# 只想本机用就设 BIND_HOST=127.0.0.1
BIND_HOST="${BIND_HOST:-0.0.0.0}"
OLLAMA_MODEL="${OLLAMA_MODEL:-qwen2.5:1.5b}"

RUN_DIR="${TMPDIR:-/tmp}/meeting-hear-services"
mkdir -p "$RUN_DIR"

c_ok()   { printf '\033[32m✓\033[0m %s\n' "$*"; }
c_warn() { printf '\033[33m!\033[0m %s\n' "$*"; }
c_err()  { printf '\033[31m✗\033[0m %s\n' "$*"; }

port_listening() { # $1=port
  if command -v ss >/dev/null 2>&1; then
    ss -ltn 2>/dev/null | grep -q ":$1 "
  else
    (exec 3<>"/dev/tcp/127.0.0.1/$1") 2>/dev/null
  fi
}

wait_http() { # $1=url $2=秒数
  local url="$1" deadline=$(( $(date +%s) + ${2:-30} ))
  while [ "$(date +%s)" -lt "$deadline" ]; do
    if curl -sS -o /dev/null --max-time 3 "$url" 2>/dev/null; then return 0; fi
    sleep 1
  done
  return 1
}

start_whisper() {
  if port_listening "$WHISPER_PORT"; then
    c_ok "端口 $WHISPER_PORT 已在监听，复用现有 whisper 服务"
    return 0
  fi
  if [ ! -x "$WHISPER_BIN" ]; then
    c_err "找不到 whisper-server：$WHISPER_BIN"
    cat <<'TIP'
    编译方式：
      git clone --depth 1 https://github.com/ggml-org/whisper.cpp
      cd whisper.cpp && cmake -B build -DWHISPER_BUILD_SERVER=ON && cmake --build build -j
      ./models/download-ggml-model.sh base
    然后用 WHISPER_DIR=/path/to/whisper.cpp 重新运行本脚本
TIP
    return 1
  fi
  if [ ! -f "$WHISPER_MODEL" ]; then
    c_err "找不到模型：$WHISPER_MODEL"
    c_warn "先下载：$WHISPER_DIR/models/download-ggml-model.sh base"
    return 1
  fi

  c_ok "启动 whisper-server（$(basename "$WHISPER_MODEL")，语言 $WHISPER_LANG，监听 $BIND_HOST:$WHISPER_PORT）"
  # --language auto 很关键：whisper-server 默认 en，中文会被按英文识别
  nohup "$WHISPER_BIN" -m "$WHISPER_MODEL" --host "$BIND_HOST" --port "$WHISPER_PORT" \
    --language "$WHISPER_LANG" \
    > "$RUN_DIR/whisper.log" 2>&1 &
  echo $! > "$RUN_DIR/whisper.pid"
  if wait_http "http://127.0.0.1:$WHISPER_PORT/" 40; then
    c_ok "whisper 服务就绪：http://127.0.0.1:$WHISPER_PORT/inference"
  else
    c_err "whisper 服务启动超时，日志：$RUN_DIR/whisper.log"
    return 1
  fi
}

start_ollama() {
  if port_listening "$OLLAMA_PORT"; then
    c_ok "端口 $OLLAMA_PORT 已在监听，复用现有 Ollama"
  else
    if ! command -v ollama >/dev/null 2>&1; then
      c_err "未安装 ollama，见 https://ollama.com/download"
      return 1
    fi
    c_ok "启动 ollama serve（监听 $BIND_HOST:$OLLAMA_PORT）"
    OLLAMA_HOST="$BIND_HOST:$OLLAMA_PORT" nohup ollama serve > "$RUN_DIR/ollama.log" 2>&1 &
    echo $! > "$RUN_DIR/ollama.pid"
    wait_http "http://127.0.0.1:$OLLAMA_PORT/api/tags" 40 || c_warn "ollama 启动较慢，继续检查模型"
  fi

  if command -v ollama >/dev/null 2>&1; then
    if ollama list 2>/dev/null | grep -q "${OLLAMA_MODEL%%:*}"; then
      c_ok "Ollama 模型已就绪：$OLLAMA_MODEL"
    else
      c_warn "还没有模型 $OLLAMA_MODEL，正在拉取（首次较慢）…"
      ollama pull "$OLLAMA_MODEL" || c_err "拉取失败，请手动 ollama pull $OLLAMA_MODEL"
    fi
  fi
}

cmd_start() {
  echo "== 启动本地服务 =="
  start_whisper || true
  start_ollama  || true
  echo
  cmd_status
}

cmd_stop() {
  echo "== 停止由本脚本启动的服务 =="
  for name in whisper ollama; do
    local f="$RUN_DIR/$name.pid"
    if [ -f "$f" ]; then
      local pid; pid=$(cat "$f")
      if kill -0 "$pid" 2>/dev/null; then
        kill "$pid" && c_ok "已停止 $name（pid $pid）"
      fi
      rm -f "$f"
    else
      c_warn "$name 不是由本脚本启动的，未处理"
    fi
  done
}

cmd_status() {
  echo "== 服务状态 =="
  if port_listening "$WHISPER_PORT"; then
    c_ok "whisper  : http://127.0.0.1:$WHISPER_PORT/inference"
  else
    c_err "whisper  : 未运行（端口 $WHISPER_PORT）"
  fi
  if port_listening "$OLLAMA_PORT"; then
    c_ok "ollama   : http://127.0.0.1:$OLLAMA_PORT/v1"
    if command -v ollama >/dev/null 2>&1; then
      ollama list 2>/dev/null | tail -n +2 | awk '{printf "            模型 %s\n", $1}'
    fi
  else
    c_err "ollama   : 未运行（端口 $OLLAMA_PORT）"
  fi
  echo
  echo "== 从其它机器连（Mac / 手机）=="
  echo "  本机地址（WSL2 内网，仅本机可用）：http://127.0.0.1:$WHISPER_PORT"
  echo "  如要从别的机器连：本机 IP 通常需要宿主机做端口转发（见 README「远程接入」）"
  echo
  echo "== 应用里的填法 =="
  cat <<TIP
  语音识别 → 服务商：本地 whisper.cpp server
    Base URL   http://127.0.0.1:$WHISPER_PORT
    接口路径    /inference
    模型名      whisper-1        （whisper.cpp server 不校验名字，非空即可）
    识别语言    由服务端决定：启动 whisper-server 时带 --language auto
                （默认是 en，不加这个参数中文会被按英文识别）
    API Key     留空
  AI 接口 → 服务商：本地 Ollama
    Base URL   http://127.0.0.1:$OLLAMA_PORT/v1
    模型名      $OLLAMA_MODEL
    API Key     留空
TIP
}

case "${1:-start}" in
  start)  cmd_start  ;;
  stop)   cmd_stop   ;;
  status) cmd_status ;;
  restart) cmd_stop; sleep 1; cmd_start ;;
  *) echo "用法: $0 {start|stop|status|restart}"; exit 2 ;;
esac
