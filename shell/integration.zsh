# OSC 133 셸 통합: 프롬프트/명령의 경계를 터미널에 알린다.
#   A = 프롬프트 시작, C;cmdline_url=<명령줄> = 명령 출력 시작, D;<exit> = 명령 종료
# 명령줄은 fish와 같은 파라미터 이름으로, 퍼센트 인코딩해 보낸다 —
# `eden list`가 "어느 세션이 무엇을 돌리고 있나"를 보여주는 데 쓴다.
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

# 바이트 단위 퍼센트 인코딩. LC_ALL=C로 멀티바이트(한글)를 바이트로 쪼갠다.
# 512바이트에서 자른다 — 목록 표시용이라 그 이상은 의미가 없다.
_eden_urlencode() {
  local LC_ALL=C s="${1[1,512]}" out="" c
  local -i i
  for (( i = 1; i <= ${#s}; i++ )); do
    c="${s[i]}"
    case "$c" in
      [a-zA-Z0-9._~/-]) out+="$c" ;;
      *) printf -v c '%%%02X' "$(( $(printf '%d' "'$c") & 255 ))"; out+="$c" ;;
    esac
  done
  print -rn -- "$out"
}

_eden_preexec() {
  printf '\e]133;C;cmdline_url=%s\a' "$(_eden_urlencode "$1")"
  _eden_executing=1
}

add-zsh-hook precmd _eden_precmd
add-zsh-hook preexec _eden_preexec
