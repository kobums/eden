# 아키텍처

eden는 Rust + wgpu(Metal)로 만든 macOS 네이티브 터미널이다. 핵심
설계 원칙은 [design.md](design.md)에 정리돼 있고, 이 문서는 실제 구현 구조를
다룬다.

## 전체 그림

```
┌──────────────────────── GUI 프로세스 (eden) ────────────────────────────┐
│                                                                          │
│  winit 이벤트 루프 (app/, App)                                           │
│    ├─ 키/마우스/IME 입력 → 세션에 바이트 전송                            │
│    ├─ 탭/페인 레이아웃 (layout.rs, 이진 분할 트리)                        │
│    ├─ 커맨드 팔레트 · AI 바 · Quake 오버레이                             │
│    └─ 렌더러 (renderer/, wgpu glyph atlas)                               │
│                                                                          │
│  세션 (session.rs)  ──── 페인마다 하나 ────                              │
│    ├─ Term (alacritty_terminal, 그리드/스크롤백)                         │
│    ├─ OSC 133 스캐너 → 블록 마크                                         │
│    └─ mux 클라이언트  ◀── Unix 소켓 ──┐                                  │
└────────────────────────────────────────┼────────────────────────────────┘
                                          │
┌──────────────────── mux 데몬 프로세스 (eden --daemon) ──────────────────┐
│    세션 레지스트리                       │                                │
│      ├─ PTY + 셸 (창을 닫아도 생존)     ▼                                │
│      ├─ 리플레이 버퍼 (최대 2MB)                                         │
│      └─ 구독자(클라이언트)에게 출력 전달                                 │
└──────────────────────────────────────────────────────────────────────────┘
```

핵심은 **셸/PTY를 GUI가 아니라 별도 데몬이 소유**한다는 점이다. GUI를 닫아도
데몬이 셸을 살려두므로 세션이 지속되고, 다시 붙으면 리플레이로 복원된다.
Term은 GUI 쪽에 남기 때문에 선택·스크롤·블록·AI 등 모든 기능이 바이트 소스만
소켓으로 바뀔 뿐 그대로 동작한다.

## 모듈

| 모듈 | 역할 |
|---|---|
| `main.rs` | 진입점 — 데몬 분기, 이벤트 루프 생성, Dock 아이콘 지정(`cargo run`도 번들과 같은 아이콘) |
| `app/` | winit 앱 — 창/탭/페인 상태, 이벤트 루프 배선, 입력 라우팅 |
| `renderer/` | wgpu 렌더러 — glyph atlas, 셀 배경/글리프 2패스, 크롬, 오버레이 |
| `session.rs` | 페인 세션 — Term + OSC 133 스캐너 + 블록 도출 + mux 클라이언트 |
| `mux.rs` | mux 데몬 + 클라이언트 + 프레임 프로토콜 (세션 지속성) |
| `layout.rs` | 페인 이진 분할 트리 (분할/제거/배치 계산·줌) |
| `config.rs` | 설정 파일 파서 (`key = value`) |
| `ai.rs` | 자연어 → 셸 명령 생성 (Anthropic BYOK / 로컬 Ollama) |

`app/`과 `renderer/`는 관심사별 하위 모듈로 나뉜다. 각 하위 모듈은 같은 타입
(`App` / `Renderer`)에 대한 `impl` 블록을 하나씩 갖는다 — 상태는 한 곳에
모아두고 동작만 파일별로 가른 구조다.

| `app/` | 역할 |
|---|---|
| `mod.rs` | `App`·`State`·`Tab` 정의, 탭/페인 생성·분할·닫기, 재그리기, 이벤트 디스패치 |
| `input.rs` | 키보드·IME → 앱 단축키 또는 PTY 바이트 (`key_to_bytes`) |
| `mouse.rs` | 클릭·드래그 선택·더블/트리플 클릭·휠·Cmd+클릭·마우스 리포팅 배선 |
| `mouse_report.rs` | 마우스 리포트 인코딩 (X10/UTF-8/SGR) — 순수 함수, 단위 테스트 대상 |
| `clipboard.rs` | 선택 복사, 붙여넣기, 마지막 출력 복사 |
| `action.rs` | 앱 액션 enum + 키맵 — 키바인딩과 팔레트가 공유하는 단일 소스 |
| `kitty_key.rs` | Kitty keyboard protocol (CSI u) 인코딩 — 순수 함수, 단위 테스트 대상 |
| `palette.rs` | 커맨드 팔레트 필터·표시 목록 + 액션 실행부(`run_action`) |
| `search.rs` | 스크롤백 검색 (Cmd+F) — 절대 줄 좌표 매치, alacritty RegexIter 사용 |
| `links.rs` | 평문 URL 감지 (뷰포트 한정 정규식 스캔) — 밑줄·Cmd+클릭 |
| `notify.rs` | 명령 완료·OSC 9/777 알림 — 조건 판단(순수 함수) + AppKit 전달 |
| `layout_persist.rs` | 탭·페인 구조 ↔ `layout.json` 직렬화와 가지치기 |
| `ai_bar.rs` | AI 바 상태, 컨텍스트 수집, 생성 결과 삽입 |
| `quake.rs` | Ctrl+` 전역 핫키 등록과 드롭다운 토글 |
| `status.rs` | 하단 상태바 문자열 (cwd·git 브랜치·CPU·메모리·시계) |

| `renderer/` | 역할 |
|---|---|
| `mod.rs` | `Renderer` 정의, wgpu 초기화(파이프라인·버퍼), 프레임 조립과 제출 |
| `text.rs` | glyph atlas(래스터라이즈·shelf packing·캐싱), 텍스트 한 줄 그리기·폭 계산·말줄임 |
| `pane.rs` | 터미널 그리드 — 셀 배경/글리프, 커서, 블록 거터, IME preedit |
| `chrome.rs` | 탭 바, 하단 상태바, AI 바, 팔레트 오버레이 |
| `color.rs` | `Theme`, ANSI/256색 → RGB, 크롬 색 파생 |

## 데이터 흐름: 키 입력 → 화면

```
키 입력 (winit)
  → app/input.rs: key_to_bytes()  (KeyEvent → PTY 바이트)
  → session.write()               → mux 클라이언트 → Unix 소켓
  → mux 데몬: PTY master에 write → 셸이 처리
  → 셸 출력 → 데몬이 리플레이 버퍼에 축적 + 구독자에 전달
  → GUI mux-reader 스레드: OSC 133 스캐너 → alacritty 파서 → Term 그리드
  → Event::Wakeup → winit 재그리기 요청
  → renderer.draw(): Term 그리드 → glyph atlas → GPU
```

## 키 입력 라우팅과 한글 IME

`app/input.rs`의 처리 우선순위는 **팔레트 → AI 바 → 검색 바 → Cmd 단축키 →
PTY 전달**이다. 오버레이가 열려 있으면 키를 가로채고, 검색 바는 Cmd 조합을
삼키지 않는다(바가 열려 있어도 Cmd+C 복사가 되어야 하므로).

Cmd 단축키는 `logical_key`(문자)가 아니라 **물리 키 위치(`KeyCode`)로 매칭**한다.
한글 등 비라틴 입력 소스에서는 `logical_key`가 자모("ㅅ")로 와서 문자 매칭이
실패하기 때문이다.

한글 IME(조합 입력)와 단축키는 다음처럼 얽혀 있다:

- winit(macOS)은 조합(preedit) 중 keyDown을 IME(`interpretKeyEvents`)로만 보내고
  `KeyboardInput`을 앱에 전달하지 않는다. 그대로 두면 **조합 중 Cmd 단축키가
  통째로 삼켜진다.** 그래서 `ModifiersChanged`에서 **Cmd를 누르는 동안
  `set_ime_allowed(false)`**로 IME를 잠시 끈다 — 조합 중이던 글자는 폐기되고
  단축키가 정상 전달된다 (Cmd를 떼면 다시 켠다).
- `on_key`의 preedit 가드(조합 중 키 무시)도 Cmd 조합은 통과시킨다 (이중 안전장치).
- `Ime::Disabled` 수신 시 `preedit`을 클리어한다 — winit은 입력 소스 전환 시
  빈 Preedit 이벤트 없이 Disabled만 보내므로, 지우지 않으면 stale preedit이
  남아 모든 키 입력이 무시된다.
- 조합 확정 텍스트는 `Ime::Commit`으로 도착해 PTY로 바로 쓰인다. 조합 중
  문자열은 커서 위치에 오버레이로 렌더링되고, IME 후보창은
  `set_ime_cursor_area`로 커서 바로 아래에 배치된다.

Cmd 조합이 아닌 키는 `key_to_bytes`가 PTY 바이트로 변환한다 — 화살표·Home/End·
PageUp/Down·Delete·Tab·Enter·Backspace·Esc, Ctrl+A~Z(C0 제어 문자),
Ctrl+Space(NUL).

## 렌더링 파이프라인

- **glyph atlas**: 글리프를 한 번만 래스터라이즈해 R8 텍스처 아틀라스에 캐싱.
  비용이 "화면의 문자 수"가 아니라 "고유 글리프 수"에 비례한다 (조사에서 확인한
  Alacritty 방식).
- **2패스 인스턴스 드로우**: ① 셀 배경(단색 사각형) ② 글리프(아틀라스 샘플링).
  각 패스는 인스턴스 draw call 하나.
- **폰트 로드·폴백**: 주 폰트는 설정의 `font-path`가 우선이고, 없으면
  MesloLGS Nerd Font(`~/Library/Fonts` · `/Library/Fonts`) → Menlo → Monaco →
  SF Mono 순으로 자동 선택한다. 폴백 체인은 주 폰트 → Nerd Font(PUA 전용) →
  한글(Apple SD Gothic Neo) → 기호(Apple Symbols) 순. PUA(powerline·Nerd Font
  아이콘)는 일반 폴백 폰트가 엉뚱한 글리프를 돌려줄 수 있어 `pua_ok` 표시가
  된 폰트(주 폰트 + Nerd Font 폴백)에서만 찾는다. 주 폰트가 Nerd Font가 아니어도
  (예: `font-path`로 Menlo 지정) 파워라인 글리프는 MesloLGS NF에서 폴백된다.
- **배율 대응**: 모니터 배율(scale factor)이 바뀌면 폰트 픽셀 크기·셀 메트릭을
  재계산하고 glyph atlas를 리셋한다 — 배율이 다른 모니터로 창을 옮겨도 글자
  크기가 유지된다.
- **테마**: 배경/전경/커서/선택 색과 폰트 크기는 설정에서 온다
  ([configuration.md](configuration.md)).
- **synchronized output (mode 2026)**: 리더 루프가 진입/종료 시퀀스를 감시해
  sync 중에는 재그리기를 억제하고, 종료 시 한 번에 그려 티어링을 없앤다.

## 세션과 블록 (OSC 133)

셸에 주입된 통합 스크립트(`shell/integration.zsh`)가 프롬프트 시작(A) /
명령 출력 시작(C) / 명령 종료+종료코드(D)를 OSC 133으로 발신한다. GUI의
바이트 스캐너가 이를 가로채 절대 줄 번호로 마크를 기록하고, 마크로부터
"명령+출력" 블록을 도출한다. 블록은 좌측 거터의 상태 바(성공/실패/실행중),
Cmd+↑/↓ 프롬프트 점프, Cmd+Shift+C 마지막 출력 복사, AI 컨텍스트에 쓰인다.

셸 통합은 zsh에 한해 ZDOTDIR 부트스트랩으로 자동 주입되며(사용자 .zshrc는
그대로 이어짐), 설정 없이 동작한다. 스크립트는 데몬이 세션 생성 시
`~/.cache/eden/shell/`에 설치한다 — bash용(`integration.bash`)도 함께 깔지만
자동 주입하지는 않고(안전한 주입 지점이 없다, [features.md](features.md) 참고)
사용자가 한 줄 source하게 안내한다. fish는 3.4+가 OSC 133을 자체 발신하므로
아무것도 필요 없다. 같은 통합 스크립트가 **OSC 7**로 현재 작업 디렉터리도
보고하며, 리더 스레드가 이를 파싱해 세션 cwd로 저장한다(하단 상태바에서 사용).

새 셸은 항상 **홈 디렉터리에서 시작**한다 — 데몬이 Dock에서 실행된 앱에서
스폰되면 cwd가 `/`인데, 그걸 상속시키지 않도록 PTY 생성 시 working directory를
명시한다.

## 하단 상태바

창 하단에 항상 표시되는 바. 왼쪽은 cwd(OSC 7) + git 브랜치(cwd에서 `.git/HEAD`
탐색), 오른쪽은 CPU·메모리(`sysinfo`)·시계(libc `localtime`). 1초 틱 스레드가
`AppEvent::Tick`을 보내 지표를 새로고침하고 재그린다. 탭 바와 상태바가 차지하는
높이만큼 콘텐츠 영역(페인 그리드)이 줄어든다.

## mux 데몬 프로토콜

Unix 소켓(`~/.cache/eden/mux/control.sock`) 위의 길이 프리픽스 프레임:

```
[u32 len][u8 tag][payload...]
```

| 방향 | 태그 | 의미 |
|---|---|---|
| 클라 → 데몬 | Create / Attach / Input / Resize / Kill / List | 세션 생성·부착·입력·크기·종료·목록 |
| 데몬 → 클라 | Attached / Output / SessionList / Exit | 부착 완료·출력 바이트·세션 목록·셸 종료 |

부착 시 데몬은 락을 잡은 채 리플레이 버퍼를 먼저 보내고 구독자로 등록해 라이브
바이트 유실을 막는다. 창을 닫으면(소켓 종료) detach로 처리하고 세션은 살아있다.
Cmd+W(Kill)는 셸에 SIGHUP을 보내 세션을 정리하며, 모든 세션이 끝나면 데몬도
종료한다.

`SessionList` 응답은 `[u32 count][u64 id]…[u64 boot]` 형식이다. 꼬리의 `boot`는
데몬 시작 시각으로, 레이아웃 복원(`layout.json`)이 "이 세션 ID들이 어느 데몬
세대의 것인지" 대조하는 데 쓴다 — 세션 ID는 데몬이 재시작하면 1부터 재발급되기
때문이다. **꼬리에 붙인 것이 하위호환의 핵심이다.** 구버전 데몬은 이 필드를 보내지
않고, 신버전 클라이언트는 페이로드 길이를 검사해 없으면 대조를 포기한다. 그래서
바이너리만 교체하고 데몬을 재시작하지 않은 상태에서도 깨지지 않는다.

리플레이는 `Output` 프레임 하나로 한 번에 온다. 알림(OSC 9/777)이 재접속마다
다시 울리는 것을 막으려고, 부착된 세션의 **첫 `Output` 프레임은 알림 감시를
건너뛴다** — 이 동작은 "리플레이 = 첫 프레임 하나"라는 데몬 쪽 성질에 의존한다.

## AI 명령 생성

Cmd+K로 연 입력 바에 자연어를 넣으면 별도 스레드가 요청한다. 라우팅은
로컬 우선: `ANTHROPIC_API_KEY`가 있으면 Anthropic Messages API(BYOK,
`claude-opus-4-8`), 없으면 로컬 Ollama(`EDEN_OLLAMA_URL` /
`EDEN_OLLAMA_MODEL`, 기본 `llama3.2`). 포커스된 페인의 최근 화면 텍스트
(최근 30줄, 최대 4000바이트) + 마지막 종료 코드를 컨텍스트로 함께 보낸다.
응답은 코드 펜스·`$` 프리픽스를 정리한 뒤 삽입하고, Esc로 취소하면 시퀀스
번호가 올라가 늦게 도착한 응답은 무시된다. **생성된 명령은 실행하지 않고
프롬프트에 삽입만** 한다 — 실행 여부는 항상 사용자 몫(조사에서 얻은 안전 원칙).
여러 줄 응답은 개행을 공백으로 바꿔 삽입 즉시 실행되는 사고를 막는다.
