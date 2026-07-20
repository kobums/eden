# OSC 133 셸 통합: 프롬프트/명령의 경계를 터미널에 알린다.
#   A = 프롬프트 시작, C = 명령 출력 시작, D;<exit> = 명령 종료
[[ -o interactive ]] || return 0
[[ -n "$_TERMDEV_INTEGRATED" ]] && return 0
_TERMDEV_INTEGRATED=1

autoload -Uz add-zsh-hook

_termdev_precmd() {
  local ret=$?
  if [[ -n "$_termdev_executing" ]]; then
    printf '\e]133;D;%s\a' "$ret"
    unset _termdev_executing
  fi
  printf '\e]133;A\a'
}

_termdev_preexec() {
  printf '\e]133;C\a'
  _termdev_executing=1
}

add-zsh-hook precmd _termdev_precmd
add-zsh-hook preexec _termdev_preexec
