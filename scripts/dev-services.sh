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
OLLAMA_MODEL="${OLLAMA_MODEL:-qwen3.5:2b}"

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

# 已在监听的 whisper-server 是否配了语言自动检测。
# 它默认按英文识别，中文语音会被转成英文乱码、应用里却什么都不显示。
whisper_lang_ok() {
  local pid cmd
  pid=$(pgrep -f "whisper-server.*--port[= ]*$WHISPER_PORT" 2>/dev/null | head -1)
  [ -z "$pid" ] && pid=$(pgrep -x whisper-server 2>/dev/null | head -1)
  [ -z "$pid" ] && return 2   # 找不到进程，无法判断
  cmd=$(tr '\0' ' ' < "/proc/$pid/cmdline" 2>/dev/null)
  case "$cmd" in
    *"--language auto"*|*"-l auto"*) return 0 ;;
    *) return 1 ;;
  esac
}

warn_missing_language() {
  c_warn "这个 whisper-server 没有带 --language auto"
  cat <<'TIP'
    它的默认语言是英文：中文语音会被按英文硬识别，结果是一段英文乱码，
    在你看来就像「识别不出任何东西」。识别语言归服务端管，应用不参与。
    修法（重启服务）：
      ./scripts/dev-services.sh restart            # 推荐
      # 或手动：先 kill 掉 whisper-server，再用 --language auto 重启：
      #   ./build/bin/whisper-server -m models/ggml-base.bin --port 8090 --language auto
TIP
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
    if whisper_lang_ok; then
      c_ok "端口 $WHISPER_PORT 已在监听，复用现有 whisper 服务（已配 --language auto）"
    else
      c_warn "端口 $WHISPER_PORT 已在监听，复用现有 whisper 服务"
      warn_missing_language
    fi
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
    c_ok "whisper 服务就绪：http://127.0.0.1:$WHISPER_PORT/inference（语言 $WHISPER_LANG）"
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
    if ollama_is_systemd && [ "${MEETINGHEAR_OLLAMA_OWN:-0}" != "1" ]; then
      # 交给 systemd，别自己再起一个；远程访问靠 override.conf 配 OLLAMA_HOST
      c_ok "通过 systemd 启动 ollama（监听地址由 /etc/systemd/system/ollama.service.d/override.conf 决定）"
      if ! systemctl start ollama 2>/dev/null; then
        c_warn "systemctl start ollama 失败（可能需要 sudo），尝试直接启动"
        OLLAMA_HOST="$BIND_HOST:$OLLAMA_PORT" nohup ollama serve > "$RUN_DIR/ollama.log" 2>&1 &
        echo $! > "$RUN_DIR/ollama.pid"
      fi
    else
      c_ok "启动 ollama serve（监听 $BIND_HOST:$OLLAMA_PORT）"
      OLLAMA_HOST="$BIND_HOST:$OLLAMA_PORT" nohup ollama serve > "$RUN_DIR/ollama.log" 2>&1 &
      echo $! > "$RUN_DIR/ollama.pid"
    fi
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
  echo "== 停止 whisper / ollama =="
  local stopped_ports=""
  for name in whisper ollama; do
    local f="$RUN_DIR/$name.pid"
    if [ -f "$f" ]; then
      local pid; pid=$(cat "$f")
      if kill -0 "$pid" 2>/dev/null; then
        kill "$pid" && c_ok "已停止 $name（pid $pid）"
        stopped_ports="$stopped_ports $(port_of "$name")"
      fi
      rm -f "$f"
      continue
    fi
    # 没记 pid：如果占用端口的正是同名进程，一并收掉。
    # 否则 restart 会「复用」一个配置不对的旧服务，用户以为改了其实没改。
    local port pid
    case "$name" in
      whisper) port="$WHISPER_PORT" ;;
      ollama)
        port="$OLLAMA_PORT"
        if ollama_is_systemd && port_listening "$port"; then
          if systemctl stop ollama 2>/dev/null; then
            c_ok "已停止 systemd 的 ollama"
            stopped_ports="$stopped_ports $port"
          else
            c_warn "ollama 由 systemd 管理，停止需要 sudo：sudo systemctl stop ollama"
          fi
          continue
        fi
        ;;
    esac
    pid=$(pgrep -x "${name}-server" 2>/dev/null | head -1)
    [ -z "$pid" ] && [ "$name" = "ollama" ] && pid=$(pgrep -x ollama 2>/dev/null | head -1)
    if [ -n "$pid" ] && port_listening "$port"; then
      kill "$pid" && c_ok "已停止占用 $port 的 $name（pid $pid，非本脚本启动）"
      stopped_ports="$stopped_ports $port"
    else
      c_warn "$name 不是由本脚本启动的，未处理"
    fi
  done
  # 等「刚刚被停掉的」端口真正释放再返回：否则紧接着 start 会「复用」一个正在
  # 退出的旧进程，两个实例同时绑定同一端口，请求会被劈成两半，表现为推理卡住不动。
  local waited=0 p
  for p in $stopped_ports; do
    while [ "$waited" -lt 15 ] && port_listening "$p"; do
      sleep 1
      waited=$((waited + 1))
    done
    port_listening "$p" && c_warn "端口 $p 还没释放干净，稍等一下再 start"
  done
}

# ollama 是否由 systemd 管理（官方安装脚本默认会装成服务）。
# 这种情况必须走 systemctl：直接 kill 掉它会被 systemd 自动拉起来，
# 和我们自己起的 ollama serve 抢同一个端口，两边都不可用。
ollama_is_systemd() {
  command -v systemctl >/dev/null 2>&1 &&
    systemctl list-unit-files ollama.service >/dev/null 2>&1 &&
    systemctl cat ollama.service >/dev/null 2>&1
}

# 服务名 → 端口
port_of() {
  case "$1" in
    whisper) echo "$WHISPER_PORT" ;;
    ollama)  echo "$OLLAMA_PORT" ;;
  esac
}

cmd_status() {
  echo "== 服务状态 =="
  if port_listening "$WHISPER_PORT"; then
    c_ok "whisper  : http://127.0.0.1:$WHISPER_PORT/inference"
    if ! whisper_lang_ok; then
      c_warn "whisper  : 未配置 --language auto，中文会被按英文识别（见下方说明）"
    fi
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
