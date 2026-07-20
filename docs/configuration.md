# 설정

설정 파일 경로: `~/.config/terminal-dev/config`

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
| `font-size` | 실수 (6~72) | `14` | 폰트 크기 (논리 픽셀) |
| `font-path` | 파일 경로 | (시스템 자동) | 주 폰트 파일. 없으면 Menlo/Monaco/SF Mono 순으로 자동 선택 |
| `scrollback` | 정수 | `10000` | 스크롤백 줄 수 (최대 1,000,000) |
| `background` | `#rrggbb` | `#16161e` | 배경색 |
| `foreground` | `#rrggbb` | `#d9d9de` | 기본 전경색 |
| `cursor-color` | `#rrggbb` | `#d9d9de` | 커서 색 |
| `selection-color` | `#rrggbb` | `#3b547a` | 선택 영역 배경색 |

색은 `#` 유무 모두 허용된다 (`#16161e` = `16161e`).

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

16색 ANSI 팔레트(빨강/초록/… 개별 색), 블록 상태 바 색, 탭 바 색, 폰트 폴백
목록은 현재 코드에 고정돼 있다. 향후 설정 항목으로 열 수 있다.
