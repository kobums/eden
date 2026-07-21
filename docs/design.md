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
  - 검증: 실행 시 `[mark] PromptStart` 기록 확인 (TERMDEV_DEBUG_MARKS=1)
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
  - 남은 것: 페인 줌, 구분선 드래그 리사이즈, 선언적 레이아웃(Zellij식)
- [x] Phase 6 — AI: 블록 컨텍스트 기반 자연어 → 명령 생성. BYOK + Ollama (차별화 ①)
  - `ai.rs`: 로컬 우선 라우팅 — ANTHROPIC_API_KEY 있으면 Anthropic(claude-opus-4-8), 없으면 로컬 Ollama
  - Cmd+K로 하단 AI 입력 바 → 자연어(한/영) 입력 → Enter로 생성 (별도 스레드, 취소 시퀀스)
  - 컨텍스트: 포커스 페인 최근 화면 텍스트 + 마지막 종료 코드를 함께 전송
  - **안전 원칙: 생성된 명령은 입력줄에 삽입만 하고 실행하지 않음** (실행은 항상 사용자 몫)
  - 검증: Cmd+K → "show disk usage" → `ls -la /tmp` 프롬프트 삽입(미실행) 확인
  - 남은 것: 스트리밍 응답, 에러 설명 모드, 명령 미리보기/수정 UI, ant 프로필 인증
- [x] Phase 7 — 세션 지속성: detach/attach, 재시작 후 복원 (차별화 ②-b)
  - `mux.rs`: 셸/PTY를 소유하는 별도 데몬 프로세스 (`terminal --daemon`, setsid로 독립)
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
  - `config.rs`: `~/.config/terminal-dev/config` (Ghostty식 key=value), 없으면 기본값
  - 설정 항목: font-size, font-path, scrollback, background/foreground/cursor/selection(#rrggbb)
  - 렌더러를 Theme 기반으로 리팩터 (bg/fg/선택/커서 색 + 폰트 설정 반영), 세션은 scrollback 반영
  - `config.example` 제공 (전부 주석, 복사해서 사용)
  - 커맨드 팔레트(Cmd+Shift+P): 10개 액션 목록 + 부분일치 필터 + 화살표/Enter 실행
  - 검증: 초록 테마+폰트20 반영 확인 / 팔레트에서 "split" 필터→Enter→페인 분할 확인
- [x] Phase 9b — Quake 모드 + 배포 패키징
  - Quake 드롭다운: `global-hotkey`로 Ctrl+` 전역 핫키 등록, OS 콜백을 winit으로 포워딩, 토글 시 화면 상단 배치+포커스
  - 검증: Finder에서 Ctrl+` → 터미널 숨김 → 다시 Ctrl+` → 상단 드롭다운+포커스 확인
  - 패키징: `scripts/bundle.sh`(.app 번들+Info.plist), `scripts/make-icon.sh`(아이콘), `Casks/terminal-dev.rb`(Homebrew Cask), README/LICENSE(MIT)
  - 검증: `.app` 번들 실행 → "terminal-dev" 타이틀, 데몬 스폰, 렌더링 정상
  - 남은 것: 코드 서명·공증(Apple Developer 자격증명 필요)

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

미뤄둔 프로토콜 — 안전한 테스트 하네스가 없으면 검증 불가라 보류:
- Kitty keyboard(CSI-u 인코더): 완전 구현 + Neovim 등 실제 클라이언트 테스트 필요 (반쪽 인코더는 앱 입력을 깨뜨림)
- Kitty graphics(GPU 이미지): APC 파싱 + 이미지 디코드 + 별도 텍스처 아틀라스/배치 서브시스템 필요

각 Phase는 "직접 실행해서 확인 가능한 상태"로 끝나야 다음으로 넘어간다.

## 미결 사항

- 제품 이름 (현재 리포 이름 `terminal`)
- **Phase 10·11의 수동 GUI 검증** — 자동 테스트는 순수 로직만 덮는다. 실제 창을 띄워
  아래를 확인해야 완료로 볼 수 있다:
  - vim `:set mouse=a` — 클릭 커서 이동 / 드래그 비주얼 선택 / 휠 스크롤
  - htop — 컬럼 헤더 클릭 정렬, **Shift+드래그로 로컬 텍스트 선택**
  - less — 휠 스크롤 (ALTERNATE_SCROLL·APP_CURSOR 분기)
  - `printf '\e[?1000h\e[?1006h'; cat -v` 후 클릭 → `^[[<0;12;5M` / `^[[<0;12;5m`
  - Cmd+F로 `bin` 검색 → 하이라이트·카운터, Enter 순환, `(` 입력 시 invalid, 한글 조합,
    검색 중 `ls` 실행 후에도 하이라이트가 올바른 줄에 유지되는지(절대 줄 좌표 검증)
- 라이선스 (MIT vs GPL — 공개 프로덕트 방향이므로 초기에 결정 필요)
- IME/한글 조합 입력 처리 방식 (Phase 2에서 조사)
