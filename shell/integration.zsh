# OSC 133 셸 통합: 프롬프트/명령의 경계를 터미널에 알린다.
#   A = 프롬프트 시작, C = 명령 출력 시작, D;<exit> = 명령 종료
[[ -o interactive ]] || return 0
[[ -n "$_EDEN_INTEGRATED" ]] && return 0
_EDEN_INTEGRATED=1

autoload -Uz add-zsh-hook

_eden_precmd() {
  local ret=$?
  if [[ -n "$_eden_executing" ]]; then
    printf '\e]133;D;%s\a' "$ret"
    unset _eden_executing
  fi
  printf '\e]133;A\a'
  # OSC 7: 현재 작업 디렉터리 보고 (상태바에서 사용)
  printf '\e]7;file://%s%s\a' "${HOST}" "${PWD}"
}

_eden_preexec() {
  printf '\e]133;C\a'
  _eden_executing=1
}

add-zsh-hook precmd _eden_precmd
add-zsh-hook preexec _eden_preexec
