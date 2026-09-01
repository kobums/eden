# 설계 문서

> 조사 결과는 [research.md](research.md) 참고. 이 문서는 결정 사항과 아키텍처, 로드맵을 기록한다. 구현 구조는 [architecture.md](architecture.md).

## 확정된 결정 (2026-07-20)

| 항목 | 결정 | 이유 |
|---|---|---|
| 목표 | **공개 프로덕트** (오픈소스) | 안정성·문서·기본값 완성도까지 품질 기준으로 삼는다 |
| 플랫폼 | **macOS 우선** | 개발 환경이 macOS. 네이티브 최적화로 빠르게 완성 후 확장 (Ghostty 전략) |
| 스택 | **Rust + alacritty_terminal** | PTY·VT 파서·그리드는 검증된 크레이트(Zed도 사용)로 깔고, 렌더링·UI·차별화에 시간 투자 |
| 차별화 축 | **① 블록 UI + AI ② 내장 멀티플렉서** | 조사에서 검증된 수요 1위(블록/AI) + tmux 3대 불만을 원천 해소(내장 mux) |

## 제품 원칙 (조사의 교훈)

1. **네이티브 성능이 기본기** — Electron 금지. GPU 렌더링(glyph atlas + damage tracking)은 협상 불가.
2. **로컬 우선** — 계정·클라우드 강제 금지. AI는 BYOK(자기 API 키) + 로컬 모델(Ollama) 옵션. (Warp 반발의 교훈)
3. **설치 즉시 완성** — 설정 없이 기본값으로 완결된 경험. (Ghostty 모델, tmux 반면교사)
4. **표준에 올라탄다** — 블록은 OSC 133, 키보드는 Kitty keyboard protocol, 이미지는 Kitty graphics protocol. 독자 규격 남발 금지.

## 아키텍처

```
┌─────────────────────────────────────────────┐
│ UI 레이어 (탭/분할/블록/커맨드 팔레트)         │  ← 직접 구현
├─────────────────────────────────────────────┤
│ 렌더러 (wgpu → Metal, glyph atlas, damage)   │  ← 직접 구현
├─────────────────────────────────────────────┤
│ 블록 엔진 (OSC 133 파싱 → 명령+출력 단위)      │  ← 직접 구현 (차별화 ①)
├─────────────────────────────────────────────┤
│ 세션 매니저 (페인/탭 모델, 세션 지속성)         │  ← 직접 구현 (차별화 ②)
├─────────────────────────────────────────────┤
│ 터미널 코어: PTY + VT 파서 + 그리드            │  ← alacritty_terminal
└─────────────────────────────────────────────┘
   AI 서비스 (BYOK/Ollama, 블록 컨텍스트 인식)    ← 사이드카 모듈, 코어와 분리
```

- 크레이트 구조: `core`(터미널 세션 래핑) / `renderer` / `app`(UI·이벤트 루프) / `ai` 분리를 목표로 하되, 초기에는 단일 크레이트에서 모듈로 시작해 경계가 굳으면 분리.
- 렌더링은 wgpu로 시작(macOS에선 Metal 백엔드로 동작). 병목이 확인되면 Metal 직접 구현 검토.

## 로드맵

- [x] Phase 0 — 뼈대: cargo 프로젝트, alacritty_terminal로 PTY 셸 실행 + 그리드 상태 확인 (headless)
- [x] Phase 1 — 창에 글자 그리기: winit 창 + wgpu glyph atlas 렌더링, 키 입력 → PTY
  - 확인됨: 창 오픈, 셸 프롬프트 컬러 렌더링(powerline 배경색 포함), 블록 커서, OSC 창 제목 반영
  - 알려진 한계: 한글 IME 미지원(Phase 2), powerline 전용 글리프 등 폰트 폴백 없음, `ESC k`(screen 제목 시퀀스) 잔여물 'k' 표시
- [x] Phase 2 — 쓸 수 있는 터미널: 스크롤백, 선택/클립보드, 트루컬러, 커서, 리사이즈/reflow, IME(한글 입력)
  - 스크롤백 10,000줄 + 휠 스크롤(대체 스크린에서는 화살표 변환), 입력 시 자동 하단 스크롤
  - 마우스 선택(단일/단어/줄 — 클릭 횟수), Cmd+C/V(bracketed paste), OSC 52 클립보드
  - 한글 IME: preedit 오버레이 렌더링 + 커서 위치에 후보창 배치, 폰트 폴백(Apple SD Gothic Neo)
  - PUA(powerline 아이콘)는 주 폰트에서만 찾도록 제한 (폴백 오검출 방지)
  - 남은 것: ScaleFactorChanged 대응 (마우스 리포팅은 Phase 10, 검색은 Phase 11에서 완료)
- [x] Phase 3 — 셸 통합: OSC 133 마킹(zsh/bash 스크립트 제공), 프롬프트 점프
  - 자체 PTY IO 루프로 교체 (alacritty EventLoop 제거) — 파서 앞단에서 OSC 133 가로채기
  - zsh 통합: `shell/integration.zsh`(precmd/preexec → A/C/D;exit), ZDOTDIR 부트스트랩으로 자동 주입
  - 마크는 절대 줄 번호(히스토리 포함)로 기록, Cmd+↑/↓ 프롬프트 점프
  - 검증: 실행 시 `[mark] PromptStart` 기록 확인 (EDEN_DEBUG_MARKS=1)
  - 남은 것: bash/fish 통합, 히스토리 상한(10k) 초과 시 마크 오차, B(프롬프트 끝) 마크 활용
- [x] Phase 4 — 블록 UI 1단계: 블록 도출 + 상태 시각화 + 블록 단위 조작 (차별화 ①)
  - 마크 → 블록 도출 (프롬프트 A / 출력 시작 C / 종료 D;exit)
  - 좌측 거터 상태 바: 실행 중=파랑, 성공=초록, 실패=빨강 (실패 명령 시각화 검증됨)
  - Cmd+클릭 = 블록 전체 선택, Cmd+Shift+C = 마지막 명령 출력 복사 (bounds_to_string, 비파괴)
  - 남은 것: 블록 접기, 블록 검색/북마크, 블록 호버 UI, 출력 잘라 보기
- [x] Phase 5a — 탭: 다중 세션 (차별화 ②-a 1단계)
  - 탭마다 독립 세션(PTY+그리드+마크/블록), 이벤트에 탭 ID 태깅으로 라우팅
  - 탭 바 렌더링(활성 하이라이트, OSC 제목 반영), 탭 클릭 전환
  - Cmd+T 새 탭, Cmd+W 닫기(셸 종료 시 자동 닫힘), Cmd+1..9, Cmd+Shift+[/], 마지막 탭 닫으면 종료
  - 검증: Cmd+T → 2번 탭 생성·독립 실행·제목 반영 확인
- [x] Phase 5b — 분할(페인): 이진 분할 트리 (차별화 ②-a 2단계)
  - `layout.rs`: 페인 이진 분할 트리 (split_leaf/remove/layout), 탭 = 트리
  - Cmd+D 좌우 / Cmd+Shift+D 상하 분할, Cmd+Option+화살표 포커스 이동, 클릭 포커스
  - Cmd+W = 포커스 페인 닫기 (마지막 페인이면 탭, 마지막 탭이면 앱 종료), 셸 exit도 동일
  - 렌더러 다중 페인화 (PaneView), 커서/IME/블록바는 페인별, 휠은 마우스 아래 페인
  - 검증: Cmd+D 분할 → 새 페인 포커스 → 독립 실행 확인
  - 남은 것: 선언적 레이아웃(Zellij식) (페인 줌은 Phase 12, 구분선 드래그는 Phase 13에서 완료)
- [x] Phase 6 — AI: 블록 컨텍스트 기반 자연어 → 명령 생성. BYOK + Ollama (차별화 ①)
  - `ai.rs`: 로컬 우선 라우팅 — ANTHROPIC_API_KEY 있으면 Anthropic(claude-opus-4-8), 없으면 로컬 Ollama
  - Cmd+K로 하단 AI 입력 바 → 자연어(한/영) 입력 → Enter로 생성 (별도 스레드, 취소 시퀀스)
  - 컨텍스트: 포커스 페인 최근 화면 텍스트 + 마지막 종료 코드를 함께 전송
  - **안전 원칙: 생성된 명령은 입력줄에 삽입만 하고 실행하지 않음** (실행은 항상 사용자 몫)
  - 검증: Cmd+K → "show disk usage" → `ls -la /tmp` 프롬프트 삽입(미실행) 확인
  - 남은 것: 스트리밍 응답, 에러 설명 모드, 명령 미리보기/수정 UI, ant 프로필 인증
- [x] Phase 7 — 세션 지속성: detach/attach, 재시작 후 복원 (차별화 ②-b)
  - `mux.rs`: 셸/PTY를 소유하는 별도 데몬 프로세스 (`eden --daemon`, setsid로 독립)
  - 데몬은 세션별 출력을 리플레이 버퍼(최대 2MB)에 축적 + 붙은 클라이언트에 실시간 전달
  - 클라이언트(GUI)는 Unix 소켓으로 attach → 리플레이 재생으로 화면/스크롤백/실행 상태 복원
  - **Term은 GUI에 그대로 유지** → 선택/스크롤/블록/AI/한글 등 기존 기능 전부 무수정 보존
  - GUI 닫기 = detach(데몬 생존), 재실행 = 살아있는 세션을 탭으로 자동 복원, Cmd+W = 세션 종료
  - 검증: `echo PERSIST_MARKER_42` → GUI 종료 → 데몬 생존 확인 → 재실행 시 화면+블록바 복원 + 재입력 동작 확인
  - 남은 것: detach 시 리플레이 상한 초과분 스크롤백 손실(현재 화면은 보존), 원격 mux(SSH), 분할 레이아웃 복원(현재는 세션당 탭 1개로 복원)
- [~] Phase 8 — 프로토콜: OSC 8 하이퍼링크 + synchronized output (부분 완료)
  - OSC 8 하이퍼링크: 링크 셀에 밑줄 렌더링, Cmd+클릭으로 URL 열기(`open`) — 검증됨
  - synchronized output(mode 2026): 리더 루프가 진입/종료 시퀀스를 감시해 sync 중 redraw 억제, 종료 시 원자적 프레임 — 배선됨
  - OSC 52 클립보드는 Phase 2에서 이미 완료
  - **미루는 것(반쪽 구현이 오히려 해로움)**:
    - Kitty keyboard protocol — 완전한 CSI-u 인코더 필요 (advertise만 하고 인코딩 틀리면 Neovim 등 입력 깨짐)
    - Kitty graphics protocol — APC 파싱 + GPU 이미지 서브시스템(별도 아틀라스) 필요
- [~] Phase 9 — 프로덕트화: 설정 파일 + 테마 + 커맨드 팔레트 (부분 완료)
  - `config.rs`: `~/.config/eden/config` (Ghostty식 key=value), 없으면 기본값
  - 설정 항목: font-size, font-path, scrollback, background/foreground/cursor/selection(#rrggbb)
  - 렌더러를 Theme 기반으로 리팩터 (bg/fg/선택/커서 색 + 폰트 설정 반영), 세션은 scrollback 반영
  - `config.example` 제공 (전부 주석, 복사해서 사용)
  - 커맨드 팔레트(Cmd+Shift+P): 10개 액션 목록 + 부분일치 필터 + 화살표/Enter 실행
  - 검증: 초록 테마+폰트20 반영 확인 / 팔레트에서 "split" 필터→Enter→페인 분할 확인
- [x] Phase 9b — Quake 모드 + 배포 패키징
  - Quake 드롭다운: `global-hotkey`로 Ctrl+` 전역 핫키 등록, OS 콜백을 winit으로 포워딩, 토글 시 화면 상단 배치+포커스
  - 검증: Finder에서 Ctrl+` → 터미널 숨김 → 다시 Ctrl+` → 상단 드롭다운+포커스 확인
  - 패키징: `scripts/bundle.sh`(.app 번들+Info.plist), `scripts/make-icon.sh`(아이콘), `Casks/eden.rb`(Homebrew Cask), README/LICENSE(MIT)
  - 검증: `.app` 번들 실행 → "eden" 타이틀, 데몬 스폰, 렌더링 정상
  - ~~남은 것: 코드 서명·공증~~ — Phase 14(릴리스 파이프라인)에서 완료

- [x] Phase 10 — 마우스 리포팅: TTY 앱으로 마우스 이벤트 전달 (Phase 2의 숙제)
  - `app/mouse_report.rs`: 인코딩을 순수 함수로 분리 — App·Term·락이 없어 단위 테스트 가능
  - 클릭/해제/드래그(1002)/이동(1003)/휠(64·65), SGR(1006) → UTF-8(1005) → 레거시 X10 순 폴백
  - 레거시는 좌표가 223을 넘으면 드롭한다 (쓰레기를 보내느니 버린다). urxvt 1015는 구현하지 않음
  - Shift = 로컬 선택 탈출구 (xterm·iTerm2 관례). htop에서 텍스트를 선택할 유일한 수단
  - 포커스 이동을 리포팅보다 먼저 처리 — 비포커스 페인 클릭이 엉뚱한 세션으로 가지 않게
  - 이동 리포트는 셀 단위 중복 제거. winit은 픽셀마다 이벤트를 쏘므로 1003에서 소켓이 포화된다
  - 함께 고친 것: 휠이 ALTERNATE_SCROLL을 무시하던 문제, APP_CURSOR(DECCKM)에서 CSI 대신
    SS3(`\eOA`)를 보내야 하는데 항상 CSI를 보내던 문제, 커서가 페인 밖일 때 휠이 먹통이던 문제
  - `PANE_PADDING` 중복 상수 제거 → `renderer::PADDING` 공유 ("같아야 한다"는 주석뿐이던 잠재 버그)
  - 남은 것: **실제 앱에서의 수동 검증** (vim `:set mouse=a`, htop, lazygit, less) — 아래 참고
- [x] Phase 11 — 스크롤백 검색 (Cmd+F)
  - alacritty의 `term::search`(RegexSearch/RegexIter)를 사용 — 이 리포에서 미사용이던 기능.
    줄바꿈 래핑·와이드 문자·스크롤백 경계를 이미 처리하고 smart case가 공짜
  - `RegexIter`는 명시적 end에서 멈춰 버퍼를 순환하지 않는다 → 직접 스캔 루프를 짤 때
    가장 위험했던 무한 루프 가능성이 원천 제거된다 (설계 단계의 최대 리스크였음)
  - 매치는 **절대 줄 번호**로 저장하고 렌더 시점에 그리드 좌표로 변환한다. 그리드 `Line`은
    출력이 날 때마다 밀려 캐시하면 조용히 어긋난다 (블록·마크가 쓰는 관용구와 동일).
    덕분에 새 출력이 들어와도 재스캔이 필요 없다
  - 타이핑마다 증분 검색, 매치 1000개 상한, Enter/Shift+Enter 순환, 히트를 화면 가운데로
  - 이미 화면 안에 있는 매치로는 스크롤하지 않는다 — 증분 검색 중 화면 떨림 방지
  - Esc는 스크롤 위치를 유지한 채 닫는다 (브라우저·에디터 관례)
  - `Renderer::draw`의 위치 인자 7개를 `DrawParams` 구조체로 묶었다
  - 남은 것: 페인별 독립 검색(현재는 포커스 이동 시 닫힘), 선택 영역 내 검색, 검색 히스토리
- [x] Phase T — 테스트 + CI 기반 (Phase 10·11과 병행)
  - 이 리포의 첫 테스트. 56개, 전부 GPU·PTY 없이 돈다
  - `layout.rs`를 페이로드 제네릭(`PaneNode<P = Pane>` + `PaneId`)으로 바꿔 테스트 가능하게 했다.
    `Pane`이 `Session`(→ mux 데몬)을 소유해 막혀 있었다. 기본 타입 파라미터 덕에 호출부는 무수정
  - `Config::load()`에서 파일 IO를 떼어내 `Config::parse(&str)` 노출
  - 커버: 분할 트리 15 · 설정 파서 13 · 색 변환 7 · 마우스 인코딩 15 · 검색 좌표 7
  - `.github/workflows/ci.yml` (macos-14): fmt · clippy · build · test
  - clippy 경고 19개를 정리하고 `-D warnings`로 전환

- [x] Phase 12 — 페인 줌 (Phase 5b의 숙제)
  - Cmd+Z로 포커스된 페인을 탭 전체로 확대/복원. 분할 트리는 건드리지 않고
    배치 단계에서만 무시하므로 풀면 원래 구조가 정확히 돌아온다
  - 배치를 계산하던 세 곳(히트 테스트·PTY 리사이즈·렌더)이 각자 `root.layout()`을
    부르고 있었다. `PaneNode::layout_zoomed`로 모아서 규칙을 한 곳에만 넣었다 —
    기존 중복도 함께 제거됐다
  - 분할·페인 닫기·포커스 이동 시 자동 해제. 특히 포커스 이동은 줌 중이면
    배치에 페인이 하나뿐이라 이동할 곳이 없어 조용히 먹통이 됐을 자리다
  - `focused`가 트리에 없으면 줌을 무시하고 정상 배치로 떨어진다 (빈 화면 방지)
  - 탭 제목에 `[Z]` 표시 — 줌 중에는 다른 페인이 사라진 것처럼 보인다 (tmux 관례)
  - 테스트 4개: 전체 rect 차지, 줌 해제 후 배치 완전 일치, 없는 focused 폴백, 단일 리프
  - 남은 것: 구분선 드래그 리사이즈

- [x] Phase 13 — 구분선 드래그 리사이즈 + bash 셸 통합 (Phase 5b·3의 숙제)
  - `PaneNode::Split`에 `ratio`(0.1~0.9) 추가. 기존 고정 50/50을 대체한다
  - 배치와 구분선 위치가 같은 계산을 쓰도록 `split_rects`로 추출 — 어긋나면
    구분선이 실제 경계와 다른 자리에 잡힌다
  - `dividers()`가 경로(`SplitPath`)와 함께 히트 영역을 돌려주고, `set_ratio(path, r)`로
    중첩 분할 중 하나만 지목해 바꾼다. 히트 영역은 GAP(3px)보다 넉넉하다(±4px)
  - 비율은 0.1~0.9로 clamp — 한쪽 페인이 사라질 만큼 끌 수 없다
  - 드래그는 선택·마우스 리포팅보다 우선. 줌 중에는 구분선이 없다
  - bash OSC 133 통합: **자동 주입하지 않는다.** bash에는 ZDOTDIR 같은 안전한
    주입 지점이 없다 — 로그인 셸은 `--rcfile`을 무시하고, `--rcfile`을 쓰려고
    로그인 셸을 포기하면 `/etc/profile`(path_helper)을 건너뛰어 PATH가 조용히
    달라진다(실측 확인). 스크립트만 깔고 한 줄 추가를 안내한다
  - bash 3.2(macOS 기본)로 검증: 종료 코드 전달, 한 줄 여러 명령에서 C 한 번,
    OSC 7 cwd 추적, 탭 완성 중 오탐 없음
  - fish는 **손댈 게 없었다.** fish 3.4+가 OSC 133을 자체 내보낸다(4.8.1로 실측).
    통합 스크립트를 썼다가 마크가 중복돼 되돌렸다. fish는 파라미터를 붙여
    보내지만(`A;click_events=1`) 파서가 첫 글자로 분기해 그대로 동작한다
  - 테스트 11개 추가 (비율·clamp·구분선 위치·중첩 경로·잘못된 경로 + 세 셸의
    OSC 133 형식 호환성). 총 81개
  - 남은 것: 구분선 호버 시 커서 모양 변경, 더블클릭으로 50/50 복원

- [x] Phase 14 — 프로덕트화 마무리: 이름·아이콘·폰트·릴리스 파이프라인 (v0.1.1~v0.1.4)
  - 제품 이름 확정: `terminal-dev` → **eden** (저장소 kobums/eden, 소켓·설정 경로 `~/.cache/eden`·`~/.config/eden`)
  - 앱 아이콘(◡̈): `.app` 번들은 Info.plist로, `cargo run`은 `resumed`에서
    `NSApplication.setApplicationIconImage`로 지정 — 맨 바이너리도 Dock 아이콘이 같다
  - 폰트: MesloLGS Nerd Font 우선 로드 + PUA(파워라인) 글리프는 Nerd Font
    폴백에서만 찾도록 `pua_ok` 분리. 모니터 배율 변경 시 폰트 재계산 + atlas 리셋
  - 새 셸을 홈 디렉터리에서 시작 (Dock 실행 시 데몬 cwd가 `/`인 문제)
  - 릴리스 파이프라인 `scripts/release.sh`: Developer ID 서명(hardened runtime)
    → notarytool 공증 → staple → zip+sha256 → `--publish`로 git 태그·GitHub
    릴리스·Homebrew tap(kobums/homebrew-tap) cask 갱신까지 자동화.
    설치는 `brew install --cask kobums/tap/eden`
- [x] Phase 15 — 한글 IME 조합 중 Cmd 단축키 (v0.1.5)
  - 증상: Claude Code처럼 한글로 오래 입력하는 앱에서 Cmd+T/Cmd+D가 먹통.
    원인은 macOS winit이 조합(preedit) 중 keyDown을 IME로만 보내고
    KeyboardInput을 앱에 전달하지 않는 것 (앱 코드에 도달조차 안 함)
  - 수정 ①: Cmd를 누르는 동안 `set_ime_allowed(false)` — IME를 우회해
    KeyEvent가 전달된다. 조합 중이던 글자는 폐기 (단축키를 누른 의도)
  - 수정 ②: `on_key`의 preedit 가드에서 Cmd 조합은 통과 (이중 안전장치)
  - 수정 ③: `Ime::Disabled` 수신 시 preedit 클리어 — 조합 중 한/영 전환 시
    stale preedit이 남아 모든 키가 죽던 잠재 버그
  - 수정 ④: 단축키 매칭을 `logical_key` 문자에서 물리 키(`KeyCode`)로 전환 —
    한글 입력 소스에서 자모("ㅅ")로 와 매칭이 실패할 여지 제거. Shift 조합이
    정의 안 된 키(Cmd+Shift+W 등)는 여전히 무시해 기존 시맨틱 보존

- [x] Phase 16 — 평문 URL 자동 감지 (Phase 8의 확장)
  - `app/links.rs`: 스킴 allowlist 정규식으로 **뷰포트만** 스캔. 검색(Phase 11)과
    같은 alacritty `RegexSearch`/`RegexIter`를 써서 줄바꿈 래핑·와이드 문자가 공짜
  - 매치는 절대 줄이 아니라 **그리드 좌표**다 — 스캔한 락 안에서만 소비하므로
    좌표가 밀 틈이 없다. 프레임을 넘겨 캐시하면 안 된다
  - 끝 문장부호 정리: `.,;:!?`와 **짝 없는** 닫는 괄호만 자른다. 매치 안의
    여닫이 짝을 세므로 `https://a.com/b(c)`는 살고 `(https://a.com)`은 잘린다
  - Cmd+클릭 폴백: OSC 8 → 평문 URL → 블록 선택. **OSC 8이 이긴다** — 표시
    텍스트와 실제 URI가 다른 게 OSC 8의 존재 이유라 추론이 이기면 엉뚱한 주소가 열린다
  - 렌더러는 페인당 1회 스캔한 결과를 셀 루프에서 `contains`로만 확인 (검색
    하이라이트와 동일 관용구). 셀마다 정규식을 돌리지 않는다
  - 테스트 13개. 남은 것: 뷰포트 경계에 걸쳐 래핑된 URL은 보이는 부분만 매치
  - 수동 검증: `(https://a.com)`은 괄호를 빼고, `https://x.io/b(c)`는 포함하고,
    `https://end.org.`은 마침표를 빼고 밑줄이 그어지는 것을 실제 창에서 확인
- [x] Phase 17 — 명령 완료 알림 + OSC 9/777 (Phase 3의 D 마크 활용)
  - `Mark`에 `at: Instant` 추가 → D 마크와 직전 C 마크의 차이가 소요 시간
  - **판단은 App(메인 스레드)에서** 한다. reader 스레드는 `CommandFinished`·
    `Notify` 이벤트로 사실만 보낸다 — AppKit을 메인 스레드 밖에서 부를 수 없다
  - 조건: 임계값(기본 10초) 이상 + "보고 있지 않음"(창 비포커스 또는 그 페인의
    탭이 비활성). `WindowEvent::Focused`를 이번에 처음 추적한다
  - 전달 2단계: `requestUserAttention`(Dock 바운스, 권한·번들 조건 없음) +
    `NSUserNotification` 배너. 신식 `UNUserNotificationCenter`는 번들 밖
    `cargo run`에서 크래시해 일부러 쓰지 않았다 (deprecated 경고는 함수 하나에 격리)
  - OSC 9/777은 `watch_cwd`와 같은 carry 패턴 (스트림 감시의 네 번째 복제)
  - **attach 리플레이 가드**: 재접속 시 데몬이 과거 출력을 재생하면서 옛 OSC
    9/777이 다시 울린다. attach 세션의 첫 Output 프레임은 알림 감시를 건너뛴다.
    D 마크는 리플레이 시 소요 시간 ≈ 0이라 임계값이 알아서 거른다
  - 설정 `notify`·`notify-threshold`. 테스트 15개
  - **수동 검증에서 잡은 크래시**: 번들 밖(`cargo run`, 맨 바이너리) 실행에서는
    `defaultUserNotificationCenter`가 nil을 돌려준다(알림 센터가 번들 식별자로
    앱을 구분한다). objc2 생성 바인딩이 반환값을 non-null로 선언해 nil에서
    패닉했고, 알림이 뜰 때마다 앱이 죽었다. 바인딩을 우회해 `msg_send_id!`로
    `Option`을 받고, nil이면 배너를 포기하고 Dock 바운스만 남긴다
- [x] Phase 18 — 블록 거터 색 설정 + 키바인딩 커스터마이즈 (Phase 4·9의 숙제)
  - 거터 색 3개(`pane.rs`의 하드코딩 상수)를 Theme으로 옮기고 `block-gutter`로
    끌 수 있게 했다
  - `app/action.rs`: 팔레트 전용이던 `PaletteAction`을 공용 `Action`으로 승격.
    **키바인딩과 팔레트가 같은 액션 집합을 공유한다** — 예전에는 단축키 표와
    팔레트 목록이 따로라 조용히 갈라질 수 있었다
  - `on_command_key`의 match를 `(물리 키, Shift, Option) → Action` 조회 표로 교체.
    예전 match에는 Shift를 보지 않는 팔(F·Z·숫자·화살표)이 있어서, 표에서
    "Shift 무관"을 두 항목으로 펼쳐 동작을 정확히 보존했다 (회귀 스냅샷 테스트)
  - 설정 `keybind = cmd+t = new-tab`은 **그 조합 하나만** 바꾼다 (전체 교체 아님)
  - **Cmd 없는 조합은 거부한다.** Cmd 없는 키는 셸로 가야 하므로 사용자가 그
    영역을 가로채면 터미널이 망가진다
  - `run_action`은 각 메서드를 부르기만 한다 — 레이아웃 저장(Phase 19) 같은
    부수 효과가 메서드 본문에 있어서, 로직을 끌어오면 팔레트 실행 시 훅이 빠진다
  - 테스트 15개
- [x] Phase 19 — 분할 레이아웃 복원 (Phase 7의 숙제)
  - `app/layout_persist.rs`: 탭 순서·페인 트리·분할 비율·포커스·활성 탭을
    `~/.cache/eden/layout.json`에 저장/복원. leaf 값은 mux 세션 ID다
  - `PaneNode<P>`가 이미 페이로드 제네릭(Phase T)이라 `PaneNode<u64>`로
    직렬화·가지치기를 GPU·소켓 없이 전부 테스트할 수 있었다
  - serde derive를 layout.rs에 붙이지 않았다 — `Pane`이 `Session`(소켓·스레드)을
    소유해 Serialize가 불가능하다. `serde_json::Value` 수동 변환이 오히려 짧다
  - 가지치기: 죽은 leaf 제거 → 자식 하나 남은 Split 접기 → 전멸한 탭 제거 →
    focused가 죽었으면 첫 leaf. 기존 `remove`와 같은 규칙을 공유한다
  - **세션 ID 오매칭 방지**: 데몬이 재시작하면 ID가 1부터 재발급돼 옛 파일이
    엉뚱한 세션과 붙는다. 데몬 시작 시각(boot id)을 `SESSION_LIST` 응답
    **꼬리에** 붙이고 파일과 대조한다. 꼬리라서 구버전 데몬 + 신버전 GUI 조합도
    안 깨진다(응답이 짧으면 None → 대조 포기)
  - 저장은 구조 변화 5시점에 즉시(임시 파일 + rename). 종료 훅에 걸지 않는다 —
    크래시에서도 마지막 구조가 남아야 세션 지속성의 목적에 맞다
  - 깨진 JSON·미래 version은 예전 동작(세션당 탭 1개)으로 폴백. 테스트 12개
  - **수동 검증에서 고친 것**: 페인을 콘텐츠 전체 크기로 붙였다가 relayout으로
    줄이면, 그 사이 재생된 리플레이가 넓은 폭으로 그려진 뒤 좁은 폭으로
    리플로우돼 프롬프트가 두 번 그려진 것처럼 보였다. `PaneNode`가 페이로드
    제네릭이라 세션 ID 트리로도 배치를 계산할 수 있어서, attach 전에 각 leaf의
    **최종 크기**를 구해 넘기는 것으로 해결했다
  - 남은 것: 파일 락 없음(GUI 여럿이면 마지막 저장이 이긴다), 줌 상태 미저장
- [x] Phase 20 — Kitty keyboard protocol (CSI u) — 위 "미뤄둔 프로토콜" 해소
  - **파서 작업은 0이었다.** alacritty_terminal 0.26이 프로토콜 상태 머신(모드
    스택 push/pop/query, TermMode 플래그 5종, 대체 스크린 분리)을 이미 갖고 있다.
    `Config { kitty_keyboard: true }` 한 줄 + 인코더만 만들면 됐다
  - `app/kitty_key.rs`: mouse_report.rs와 같은 순수 함수 패턴. winit `KeyEvent`는
    테스트에서 만들 수 없어 자체 `KeyInput` + `from_winit` 어댑터로 분리했다
  - 플래그 5종 전부 구현. 반쪽 인코더는 앱 입력을 깨뜨리므로 부분 구현은 안 한다
  - kitty의 실제 구현(key_encoding.c)을 대조해, alacritty가 스펙과 어긋나는
    Shift+Enter/Shift+Tab을 `CSI 13;2u`/`CSI 9;2u`로 인코딩했다 (Neovim `<S-CR>`의 관건)
  - `term.mode()`는 **매 키마다 읽는다.** 대체 스크린 진입/이탈에서 모드가 갈려
    캐시하면 즉시 어긋난다
  - IME는 건드리지 않았다 — preedit 중에는 kitty 모드라도 인코딩하지 않고,
    조합 확정은 기존 Commit 경로로 평문이 간다 (Phase 15 회귀 방지)
  - kitty 플래그가 없으면 release는 버린다 — 이 판정이 틀리면 모든 키가 두 번
    입력된다. 명시적 테스트로 고정했다
  - 설정 `kitty-keyboard`, **기본 off** (dogfooding 탈출구). 테스트 43개
  - 미구현(스펙상 생략 허용): alternate key의 3번째 필드(기본 배치 키) —
    winit이 배치 정보를 노출하지 않는다. caps/num lock 수식자 비트도 동일
  - **수동 검증 결과(실제 창에서)**: 질의 응답 `CSI ? 0 u`, 인코딩
    `CSI 97;5u`(Ctrl+A)·`CSI 13;2u`(Shift+Enter)·`CSI 98;6u`(Ctrl+Shift+B),
    numpad `CSI 57410u`(`/`)·`CSI 57409u`(`.`)를 확인했다. 모드 스택은
    `0 → push1 → 1 → push5 → 5 → pop → 1 → pop → 0`으로 정확히 풀린다 —
    앱이 종료하며 pop하면 셸이 레거시로 돌아온다
  - 남은 것: Neovim 등 실제 클라이언트에서 며칠 써 본 뒤 기본 on 전환
- [x] Phase 21 — 퀵윈 묶음 (2026-09-01): 폰트 크기 단축키 · SUN_LEN 패닉 수정 ·
  파일 드롭 경로 붙여넣기 · 비활성 페인 디밍 · 라이트/다크 자동 전환
  - **폰트 크기 런타임 조절** — Cmd+= / Cmd+- / Cmd+0(설정값 복귀). 기존
    `set_scale_factor`를 `rebuild_metrics`로 일반화해 `base_font_size × scale`
    두 입력 중 어느 쪽이 바뀌어도 같은 경로로 재계산 + atlas 리셋. 키맵·팔레트
    양쪽에 Action 3개(`font-size-up/down/reset`)로 추가 — Phase 18의 단일 Action
    집합 덕에 두 곳이 자동으로 일치한다. `=`/`-` 키 이름을 keybind 파서에 추가
  - **`SUN_LEN` 패닉 수정** — HOME이 길면 mux 소켓 경로가 `sun_path` 한계
    (macOS 104바이트)를 넘어 `Session::new`가 패닉하던 실버그 (Phase 16~20
    검증 중 발견). 경로가 한계를 넘으면 `/tmp/eden-mux-<uid>-<HOME 해시>.sock`
    으로 폴백 — 데몬·클라이언트가 같은 규칙으로 계산하므로 항상 서로를 찾고,
    HOME별 해시로 격리도 유지된다
  - **파일 드래그 앤 드롭** — `WindowEvent::DroppedFile` → 셸 인용
    (`shell_quote`, POSIX 작은따옴표 규칙) → 기존 붙여넣기 경로(`paste_text`로
    공용화, bracketed paste 지원). 여러 파일은 이벤트가 파일마다 오므로 경로
    끝에 공백 하나를 붙여 구분한다
  - **비활성 페인 디밍** — `inactive-dim = 0~0.8` (기본 0 = 끔 — 기존 사용자의
    화면을 조용히 바꾸지 않는다). 셀 루프에서 전경·셀 배경을 `theme.bg` 쪽으로
    `mix` 한 번씩 — 별도 오버레이 패스 없음. 커서·preedit은 포커스된 페인에만
    그려져 디밍과 만나지 않는다
  - **라이트/다크 자동 전환** — `theme-light`/`theme-dark` 키 + 라이트 프리셋
    `latte`(Catppuccin Latte) 내장. `Config::parse_for(text, Appearance)`가
    맞지 않는 외양의 줄을 통째로 무시한다. 창 생성 직후 `window.theme()`으로
    초기 외양을 잡고(그 전에는 다크로 해석), `WindowEvent::ThemeChanged`에서
    설정을 다시 읽어 `Renderer::set_theme`으로 갈아탄다. 배경 불투명도의
    surface alpha 모드는 생성 시점 고정이라 못 바꾸지만, 같은 파일을 다시 읽는
    구조라 실제로는 값이 같다
  - 테스트 18개 추가 (keybind 파싱·config 범위 검사·외양별 프리셋·shell_quote·
    소켓 경로 폴백)

미뤄둔 프로토콜 — 안전한 테스트 하네스가 없으면 검증 불가라 보류:
- ~~Kitty keyboard(CSI-u 인코더)~~ — **Phase 20에서 완료** (기본 off, 검증 후 on 예정)
- Kitty graphics(GPU 이미지): APC 파싱 + 이미지 디코드 + 별도 텍스처 아틀라스/배치 서브시스템 필요

각 Phase는 "직접 실행해서 확인 가능한 상태"로 끝나야 다음으로 넘어간다.

## 향후 로드맵 후보 (Phase 16~)

2026-08-30 조사 결과. ① 위 각 Phase의 "남은 것", ② research.md의 타 터미널
장점 중 미도입분, ③ 2026년 동향(AI 에이전트 오케스트레이션 런타임으로서의
터미널)을 종합했다. 착수하면 정식 Phase로 승격해 위 로드맵에 편입한다.

**우선순위 Top 5는 Phase 16~20으로 전부 구현됐다** (위 로드맵 참고).
착수 당시의 코드 레벨 조사·설계는 [plan.md](plan.md)에 남아 있다.

### 우선순위 Top 5 — 전부 완료 (2026-08-30)

1. ~~평문 URL 자동 감지~~ — **Phase 16**
2. ~~명령 완료 알림 + OSC 9/777~~ — **Phase 17**
3. ~~Kitty keyboard protocol (CSI u)~~ — **Phase 20** (기본 off, 검증 후 on)
4. ~~분할 레이아웃 복원~~ — **Phase 19**
5. ~~블록 거터 색 설정 + 키바인딩 커스터마이즈~~ — **Phase 18**

### 전체 후보 (카테고리별)

2026-09-01 추가 조사분을 각 카테고리에 편입했다. 이미 구현된 것(더블/트리플
클릭 선택, OSC 52 클립보드, OSC 0/2 창 제목)은 코드 확인 후 제외.

A. 표준 프로토콜 완성
- ~~Kitty keyboard protocol~~ — Phase 20 완료
- Kitty graphics protocol + Sixel 폴백 — `yazi`·이미지 미리보기. APC 파싱 +
  GPU 이미지 아틀라스 필요, 큰 작업 (보류 사유는 위 참고)
- undercurl(물결 밑줄) + 컬러 밑줄 — LSP/린터 에러 표시 표준. 렌더러 작업은 작은 편
- ~~OSC 9/777 알림~~ — Phase 17 완료
- grapheme clustering (mode 2027) — 이모지 ZWJ 조합·최신 유니코드 폭.
  한글 중심 터미널이라 폭 문제와 궁합이 좋은 주제
- XTGETTCAP + eden 전용 terminfo 배포 — kitty처럼 `TERM=xterm-eden` 제공.
  kitty keyboard advertise와 시너지
- OSC 4/10/11/12 색 질의·설정 응답 — vim/neovim이 배경색을 물어 라이트/다크를
  자동 판단하는 경로. alacritty_terminal이 이벤트는 주므로 응답만 붙이면 됨
- 비주얼 벨 — BEL 수신 시 화면 플래시 또는 탭 뱃지 (OSC 9 알림과 별개)

B. 블록 UI 심화 (Phase 4의 남은 것 포함)
- 블록 접기(collapse) — 긴 출력 접어서 스크롤백 탐색성 확보 (Warp 핵심 UX)
- 블록 재실행 — 블록 클릭 → 그 명령 다시 실행
- 블록 검색/북마크, 블록 호버 액션(복사·공유 버튼)
- 명령 히스토리 팔레트 — 블록 마크에서 뽑은 "이 세션에서 실행한 명령" 목록을
  팔레트 UI로 검색·재실행 (블록 재실행의 확장판)
- 트리거(iTerm2식) — 출력 정규식 매칭 → 하이라이트/알림/명령

C. AI 심화 — 에이전트 오케스트레이션 (2026 최대 흐름)
- tmux가 AI 에이전트 런타임으로 재부상했다(세션 지속성·프로세스 격리·CLI
  조작). eden은 이미 mux 데몬 + 블록 마크를 갖고 있어 구조적 강점이 있다
- `eden` CLI 서브커맨드 (`eden list` / `eden send <id> "…"` / `eden attach`)
  — tmux `send-keys`처럼 스크립트·에이전트가 세션 조작. mux 프로토콜에
  프레임 몇 개 추가로 가능
- 에이전트 세션 대시보드 — 여러 페인의 Claude Code 상태(실행 중/입력 대기)를
  탭 바·상태바에 표시. 블록 마크(실행 중 = D 마크 없음)로 감지 가능
- AI 스트리밍 응답 + 에러 설명 모드 (Phase 6의 남은 것) — 실패 블록에서
  "왜 실패했나" 한 번에 질의
- git worktree 연동 병렬 세션 생성

D. 멀티플렉서·세션 심화 (차별화 ②)
- ~~분할 레이아웃 복원~~ — Phase 19 완료
- detach 시 리플레이 2MB 초과분 스크롤백 손실 개선 (그리드 스냅샷 저장 등)
- 원격 mux(SSH 도메인) — WezTerm의 킬러 피처. mux가 이미 소켓 기반이라
  확장 여지 있음. 장기 과제
- 선언적 레이아웃(Zellij KDL식) — 프로젝트별 "탭 3개+분할" 프리셋 (Phase 5b의 남은 것)
- 세션 이름 지정 (숫자 ID 대신)

E. UX·발견 가능성·설정
- ~~평문 URL 자동 감지~~ — Phase 16 완료. ~~거터 색·키바인딩 설정~~ — Phase 18 완료
- 설정 핫 리로드 — 파일 감시로 테마 즉시 반영 (현재는 재시작 필요).
  키바인딩·거터 색은 리로드가 쉽지만 `kitty-keyboard`는 세션 생성 시점에만
  Term Config로 들어가 기존 세션에 반영되지 않는다 (Phase 20 참고)
- 키바인딩 힌트/치트시트 오버레이 (Zellij의 발견 가능성 교훈)
- 페인별 독립 검색, 검색 히스토리 (Phase 11의 남은 것)
- 구분선 호버 시 커서 변경, 더블클릭 50/50 복원 (Phase 13의 남은 것)
- ~~런타임 폰트 크기 조절 (Cmd+= / Cmd+- / Cmd+0)~~ — **Phase 21 완료**
- Opt+드래그 사각형(블록) 선택 — alacritty `SelectionType::Block`이 crate에
  이미 있어 연결만 하면 됨
- copy-on-select 옵션 — 선택 즉시 클립보드 복사 (tmux/iTerm2 사용자 관례)
- Opt+클릭으로 커서 이동 — iTerm2 관례. 화살표 시퀀스를 계산해 보내는
  순수 함수라 테스트 친화적
- ~~파일 드래그 앤 드롭 → 경로 붙여넣기~~ — **Phase 21 완료**
- 키보드 힌트 모드 (kitty hints식) — 마우스 없이 단축키로 링크/경로 열기.
  Phase 16의 `visible_urls()`가 이미 있어 절반은 완성
- 탭 이름 변경 + 드래그 재정렬 — 세션 이름 지정(D)과 자연히 묶임
- ~~비활성 페인 디밍~~ — **Phase 21 완료** (`inactive-dim`, 기본 끔)
- 페인 번호 오버레이 점프 (tmux `display-panes`식) — Cmd+숫자 탭 전환의 페인 버전
- broadcast input — 여러 페인 동시 입력. C(에이전트 오케스트레이션)와 시너지
- 다중 창 지원 — 현재 단일 창 가정, 아키텍처 확인 필요. 규모 중~대
- 스크롤 위치 인디케이터/스크롤바 — 스크롤백 10k에서 현재 위치 표시.
  검색·프롬프트 점프와 묶으면 미니맵으로 확장 가능
- ~~macOS 라이트/다크 자동 전환~~ — **Phase 21 완료** (`theme-light`/
  `theme-dark` + `latte` 프리셋). 일반 설정 핫 리로드는 여전히 후보로 남는다
- 프로파일 — 프로필별 셸/테마/작업 디렉터리 (iTerm2 핵심 기능).
  선언적 레이아웃(D)과 겹치는 부분 정리 필요
- 스크롤백을 에디터/페이저로 열기 — 화면+스크롤백을 임시 파일로 덤프해
  `$EDITOR`로 (`bounds_to_string` 관용구 재활용)
- fish식 인라인 자동제안 — 셸 히스토리 기반 회색 제안 텍스트 (Warp 스타일).
  셸 자체 기능과 경계 설계가 필요해 규모 중~대
- 폰트 ligature — fontdue 한계로 shaping 엔진 필요, 장기

F. 성능·품질 (내부)
- damage tracking — 변경된 셀만 재렌더 (research 체크리스트 1번, 현재는 매
  프레임 전체 재구성)
- glyph atlas 가득 참 시 축출/증설
- 스크롤백 상한(10k) 초과 시 블록 마크 오차 보정 (Phase 3의 남은 것)
- ~~`HOME`이 길면 mux 소켓 경로가 `SUN_LEN`을 넘어 `Session::new`가 패닉~~ —
  **Phase 21 완료** (`/tmp` 폴백 경로)
- 패닉 로그 파일 (`~/.cache/eden/panic.log` 수준) — Phase 17에서 "번들 밖
  실행에서만 죽는" 버그가 수동 검증에서만 잡혔던 만큼 값싸고 회수가 큼
- 인앱 업데이트 확인 — 실행 시 GitHub 릴리스 태그 비교 → 상태바에
  "새 버전" 표시 (brew가 있으므로 알림만, Sparkle 불필요)
- 접근성 — VoiceOver에 화면 텍스트 노출. macOS 네이티브를 표방한다면
  장기적으로 필요

남은 후보 중 다음 타로 유력한 것은 **B(블록 접기·재실행)** 와 **C(에이전트
오케스트레이션)** 다. 둘 다 eden이 이미 가진 것(블록 마크, mux 데몬) 위에
얹히고, 다른 터미널이 쉽게 따라오기 어려운 방향이다.

~~그 사이에 끼워 넣을 소형 퀵윈: ① 폰트 크기 단축키 ② `SUN_LEN` 패닉 수정
③ 드래그 앤 드롭 경로 ④ 비활성 페인 디밍 ⑤ 라이트/다크 자동 전환~~ —
**다섯 개 모두 Phase 21로 구현 완료** (2026-09-01).

## 미결 사항

- ~~제품 이름~~ — **eden으로 확정** (v0.1.4, 저장소 kobums/eden)
- ~~Phase 10·11의 수동 GUI 검증~~ — **2026-07-21 완료.** 원시 SGR 시퀀스
  (`^[[<0;12;5M`/`m`), vim 클릭·드래그·휠, Shift+드래그 로컬 선택,
  less 휠(ALTERNATE_SCROLL·APP_CURSOR 분기), 검색 하이라이트·카운터·순환·
  한글 IME 조합을 실제 창에서 확인했다.

  검색 로직 자체(스크롤백 매치, smart case, invalid 정규식, 매치 상한, 출력이
  밀려도 절대 좌표가 유지되는지)는 `app/search.rs`의 테스트가 진짜 `Term`으로
  덮으므로 회귀는 CI가 잡는다. 마우스는 인코딩까지만 자동화돼 있고 winit
  이벤트 경로는 여전히 수동 확인이 필요하다.
- ~~라이선스~~ — **MIT로 확정** (LICENSE 파일)
- ~~IME/한글 조합 입력 처리 방식~~ — Phase 2(preedit 오버레이·후보창 배치)와
  Phase 15(조합 중 Cmd 단축키)에서 완료
