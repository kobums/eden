# 설정

설정 파일 경로: `~/.config/eden/config`

파일이 없으면 모든 값이 기본값이다("설치 즉시 완결된 경험" 원칙). 복사해서
시작할 수 있는 예시는 [`config.example`](../config.example).

## 형식

Ghostty식 `key = value`. `#`로 시작하는 줄은 주석. 알 수 없는 키나 잘못된
값은 조용히 무시하고 기본값을 유지한다.

```
# 주석
font-size = 14
background = #16161e
```

## 항목

| 키 | 값 | 기본값 | 설명 |
|---|---|---|---|
| `theme` | 프리셋 이름 | `catppuccin` | 컬러 프리셋. `catppuccin` \| `guezwhoz`(iTerm2 다크) |
| `font-size` | 실수 (6~72) | `14` | 폰트 크기 (논리 픽셀) |
| `font-path` | 파일 경로 | (시스템 자동) | 주 폰트 파일 (`~/` 시작 경로 지원). 없으면 MesloLGS Nerd Font → Menlo → Monaco → SF Mono 순으로 자동 선택 |
| `scrollback` | 정수 | `10000` | 스크롤백 줄 수 (최대 1,000,000) |
| `background` | `#rrggbb` | `#16161e` | 배경색 |
| `foreground` | `#rrggbb` | `#d9d9de` | 기본 전경색 |
| `cursor-color` | `#rrggbb` | `#d9d9de` | 커서 색 |
| `selection-color` | `#rrggbb` | `#3b547a` | 선택 영역 배경색 |
| `palette-0` … `palette-15` | `#rrggbb` | (프리셋) | 16색 ANSI 팔레트 (0~7 표준, 8~15 밝은색) |
| `cursor-style` | `block` \| `bar` \| `underline` | `block` | 커서 모양 (iTerm2 Cursor Type 대응) |
| `background-opacity` | 실수 (0.2~1.0) | `1.0` | 배경 불투명도. iTerm2처럼 기본 배경에만 적용 — 셀 배경색·텍스트는 불투명 유지 |
| `block-gutter` | `on` \| `off` | `on` | 명령 블록 왼쪽의 상태 거터(세로 줄) 표시 |
| `block-running-color` | `#rrggbb` | `#598cf2` | 실행 중인 블록의 거터 색 |
| `block-ok-color` | `#rrggbb` | `#59b875` | 성공(exit 0)한 블록의 거터 색 |
| `block-fail-color` | `#rrggbb` | `#eb6b75` | 실패한 블록의 거터 색 |
| `notify` | `on` \| `off` | `on` | 명령 완료 알림 + OSC 9/777 알림 |
| `notify-threshold` | 정수 (초) | `10` | 이보다 오래 걸린 명령만 알린다 |
| `kitty-keyboard` | `on` \| `off` | `off` | Kitty keyboard protocol (CSI u) |
| `keybind` | `<조합> = <액션>` | (기본표) | 키바인딩 재정의. 여러 줄 가능 — 아래 참고 |

색은 `#` 유무 모두 허용된다 (`#16161e` = `16161e`).

## 키바인딩 재정의

```
keybind = cmd+shift+t = new-tab
keybind = cmd+e = split-right
```

한 줄은 그 조합 하나만 바꾼다. 나머지 기본 단축키는 그대로 남으므로, 하나를
바꾸려고 전체를 다시 적을 필요가 없다.

조합에는 **`cmd`가 반드시 있어야 한다.** Cmd 없는 키는 셸로 가야 하는데,
사용자가 그 영역을 가로채면 터미널이 망가지기 때문이다. `shift`·`opt`를
덧붙일 수 있고, 수식자 순서와 대소문자는 상관없다. 키 이름은 `a`~`z`, `0`~`9`,
`left`/`right`/`up`/`down`, `[`, `]`, `enter`, `space`, `tab`.

액션 이름:

| 분류 | 액션 |
|---|---|
| 탭 | `new-tab` `close-pane` `next-tab` `prev-tab` `select-tab-1` … `select-tab-9` |
| 분할 | `split-right` `split-down` `toggle-zoom` `focus-left` `focus-right` `focus-up` `focus-down` |
| 블록 | `jump-prev-prompt` `jump-next-prompt` `copy-last-output` |
| 기타 | `copy` `paste` `search` `palette` `ai-generate` |

해석할 수 없는 줄(모르는 키·액션, `cmd` 없음)은 조용히 무시되고 기본값이
유지된다. Cmd+C·Cmd+V 같은 시스템 관례 키도 재정의할 수 있지만, 복사·붙여넣기가
사라지는 것은 사용자 책임이다.

## 폰트 폴백

주 폰트에 없는 글리프는 자동 폴백된다: 파워라인·Nerd Font 아이콘(PUA)은
설치된 MesloLGS NF에서, 한글은 Apple SD Gothic Neo에서, 기호는 Apple
Symbols에서 찾는다. 그래서 `font-path`로 Nerd Font가 아닌 폰트(예: Menlo)를
지정해도 파워라인 프롬프트 글리프는 깨지지 않는다 — MesloLGS NF가 설치돼
있기만 하면 된다.

## 컬러 프리셋

`theme = <이름>`으로 프리셋을 고른다. 프리셋은 배경/전경/커서/선택 + 16색
팔레트를 한 번에 설정한다. `theme`을 **먼저** 두고 그 아래에서 개별 색 키로
일부만 덮어쓸 수 있다.

```
theme = guezwhoz          # iTerm2 다크 프리셋 (bg #1d1d1d, 시안 계열)
font-path = /Users/me/Library/Fonts/MesloLGS NF Regular.ttf   # powerline 글리프
cursor-color = #ffffff    # 프리셋 커서색만 덮어쓰기
```

| 프리셋 | 성격 |
|---|---|
| `catppuccin` | 기본 (다크, 파스텔) |
| `guezwhoz` | iTerm2 Guezwhoz 다크 프리셋 |

## 예시

```
font-size = 16
font-path = /System/Library/Fonts/Menlo.ttc
scrollback = 50000

# 다크 그린 테마
background = #0f1a12
foreground = #cfe8cf
cursor-color = #6ee7b7
selection-color = #244a33
```

## 적용 시점

설정은 앱 시작 시 한 번 읽는다. 바꾼 뒤에는 앱을 다시 실행해야 반영된다.
세션 지속성 때문에 재실행해도 셸은 유지되므로, 테마만 바뀌고 작업은 그대로다.

## 아직 설정으로 못 바꾸는 것

AI 바·커맨드 팔레트 오버레이 색, 검색 하이라이트 색, 폰트 폴백 목록은 현재
코드에 고정돼 있다. Cmd 없는 키바인딩(Ctrl 조합 등)도 재정의할 수 없다 —
셸로 가야 할 키를 앱이 가로채는 사고를 막기 위한 의도적 제한이다.

탭 바·상태바 색은 직접 지정하는 대신 `background`/`foreground`에서 파생된다
(크롬은 배경보다 어둡게, 활성 탭은 살짝 밝게). 테마 하나만 바꿔도 크롬이 함께
따라오게 하려는 의도다.
