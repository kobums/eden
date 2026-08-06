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
| `font-path` | 파일 경로 | (시스템 자동) | 주 폰트 파일. 없으면 Menlo/Monaco/SF Mono 순으로 자동 선택 |
| `scrollback` | 정수 | `10000` | 스크롤백 줄 수 (최대 1,000,000) |
| `background` | `#rrggbb` | `#16161e` | 배경색 |
| `foreground` | `#rrggbb` | `#d9d9de` | 기본 전경색 |
| `cursor-color` | `#rrggbb` | `#d9d9de` | 커서 색 |
| `selection-color` | `#rrggbb` | `#3b547a` | 선택 영역 배경색 |
| `palette-0` … `palette-15` | `#rrggbb` | (프리셋) | 16색 ANSI 팔레트 (0~7 표준, 8~15 밝은색) |
| `cursor-style` | `block` \| `bar` \| `underline` | `block` | 커서 모양 (iTerm2 Cursor Type 대응) |
| `background-opacity` | 실수 (0.2~1.0) | `1.0` | 배경 불투명도. iTerm2처럼 기본 배경에만 적용 — 셀 배경색·텍스트는 불투명 유지 |

색은 `#` 유무 모두 허용된다 (`#16161e` = `16161e`).

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

블록 상태 바 색(성공/실패/실행 중), AI 바·커맨드 팔레트 오버레이 색, 폰트 폴백
목록, 키바인딩은 현재 코드에 고정돼 있다. 향후 설정 항목으로 열 수 있다.

탭 바·상태바 색은 직접 지정하는 대신 `background`/`foreground`에서 파생된다
(크롬은 배경보다 어둡게, 활성 탭은 살짝 밝게). 테마 하나만 바꿔도 크롬이 함께
따라오게 하려는 의도다.
