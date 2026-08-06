# OSC 133 셸 통합 (bash): 프롬프트/명령의 경계를 터미널에 알린다.
#   A = 프롬프트 시작, C = 명령 출력 시작, D;<exit> = 명령 종료
#
# zsh와 달리 bash에는 ZDOTDIR 같은 안전한 주입 지점이 없다. 로그인 셸은
# --rcfile을 무시하고, --rcfile을 쓰려고 로그인 셸을 포기하면 /etc/profile
# (path_helper)을 건너뛰어 PATH가 조용히 달라진다. 그래서 이 파일은
# 자동 주입하지 않고 사용자가 직접 부른다:
#
#   echo '[ -f ~/.cache/eden/shell/integration.bash ] && \
#     . ~/.cache/eden/shell/integration.bash' >> ~/.bash_profile
#
# bash 3.2(macOS 기본)에서도 동작한다.

# 대화형 셸에서만, 그리고 한 번만.
case $- in
  *i*) ;;
  *) return 0 ;;
esac
[ -n "$_EDEN_INTEGRATED" ] && return 0
_EDEN_INTEGRATED=1

_eden_executing=""

_eden_precmd() {
  # 종료 코드를 가장 먼저 잡아야 한다 — 아래 어떤 명령이든 $?를 덮어쓴다.
  local ret=$?
  if [ -n "$_eden_executing" ]; then
    printf '\033]133;D;%s\007' "$ret"
    _eden_executing=""
  fi
  printf '\033]133;A\007'
  # OSC 7: 현재 작업 디렉터리 보고 (상태바에서 사용)
  printf '\033]7;file://%s%s\007' "${HOSTNAME}" "${PWD}"
}

# bash에는 preexec 훅이 없어 DEBUG 트랩으로 흉내낸다.
# 트랩은 PROMPT_COMMAND 안의 명령에도 걸리므로 걸러내야 한다.
_eden_preexec() {
  # 탭 완성 중에는 무시 (완성 함수가 명령을 실행한다)
  [ -n "$COMP_LINE" ] && return
  # PROMPT_COMMAND 자신은 사용자 명령이 아니다
  case "$BASH_COMMAND" in
    _eden_precmd*) return ;;
  esac
  # 한 줄에 여러 명령이 있어도 C는 처음 한 번만
  [ -n "$_eden_executing" ] && return
  printf '\033]133;C\007'
  _eden_executing=1
}

trap '_eden_preexec' DEBUG

# 기존 PROMPT_COMMAND보다 먼저 실행되어야 $?가 사용자 명령의 것이다.
if [ -n "$PROMPT_COMMAND" ]; then
  PROMPT_COMMAND="_eden_precmd; $PROMPT_COMMAND"
else
  PROMPT_COMMAND="_eden_precmd"
fi
