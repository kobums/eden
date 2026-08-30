# Phase 16~20 구현 계획

2026-08-30 작성. [design.md](design.md)의 "향후 로드맵 후보 (Phase 16~)" 중
우선순위 Top 5를 코드 레벨로 조사해 세운 구현 계획이다.

> **상태: 다섯 Phase 모두 구현·수동 검증 완료 (2026-08-30).** 실제 결과와
> 드러난 사실은 [design.md](design.md)의 로드맵에 기록했다. 이 문서는 착수
> 전에 세운 계획 원본으로 남겨 둔다 — 계획과 결과가 어디서 갈렸는지 볼 수
> 있어야 다음 계획이 나아진다.
>
> 계획과 달랐던 큰 것 셋:
> - **Phase 20이 예상보다 작았다.** alacritty_terminal 0.26이 프로토콜 상태
>   머신을 이미 갖고 있어 파서 작업이 0이었다. 인코더만 만들면 됐다.
> - **Phase 17에 계획에 없던 구멍이 있었다.** detach 후 재접속 시 리플레이에
>   섞인 옛 OSC 9/777 알림이 다시 울린다. 첫 Output 프레임을 건너뛰어 막았다.
> - **Phase 18의 키맵 전환은 조용한 회귀를 품고 있었다.** 예전 match에는
>   Shift를 보지 않는 팔이 있어서, 표로 옮기며 그대로 두면 Cmd+Shift+F 같은
>   조합이 죽는다. "Shift 무관"을 두 항목으로 펼쳐 동작을 보존했다.
>
> **수동 검증에서만 잡힌 버그 둘** — 테스트가 못 잡는 자리였다:
> - Phase 17의 알림 배너가 **번들 밖 실행에서 앱을 죽였다.** 조건 판단은
>   순수 함수로 덮여 있었지만, 죽는 곳은 그 뒤의 AppKit 호출이었다.
> - Phase 19 복원 시 페인을 전체 크기로 붙였다가 줄여 **리플레이가 리플로우**
>   됐다. 직렬화 왕복 테스트로는 보이지 않는, 화면에서만 보이는 문제였다.
>
> 검증은 격리된 `HOME`으로 실제 창을 띄우고 화면을 캡처해서 했다. 실제 설정과
> 살아있는 세션을 건드리지 않으려는 것이었는데, 부수적으로 `HOME`이 길면
> mux 소켓이 `SUN_LEN`(~104자)을 넘어 `Session::new`가 패닉한다는 것도 드러났다.

착수 순서와 예상 규모:

| Phase | 항목 | 규모 | 신규 의존성 |
|---|---|---|---|
| 16 | 평문 URL 자동 감지 | 소 (~200줄 + 테스트) | 없음 |
| 17 | 명령 완료 알림 + OSC 9/777 | 소~중 (~250줄) | objc2-foundation feature 1개 |
| 18 | 블록 거터 색 설정 + 키바인딩 커스터마이즈 | 중 (~300줄) | 없음 |
| 19 | 분할 레이아웃 복원 | 중 (~300줄) | 없음 (serde_json 기보유) |
| 20 | Kitty keyboard protocol | 대 (~500줄 + 검증) | 없음 |

공통 원칙 (기존 Phase들과 동일):

- 반쪽 구현 금지 — 특히 Phase 20은 완전한 인코더 없이는 advertise하지 않는다
- 순수 함수로 분리해 GPU·PTY 없이 테스트 (mouse_report.rs·search.rs 패턴)
- 각 Phase는 "직접 실행해서 확인 가능한 상태"로 끝낸다

---

## Phase 16 — 평문 URL 자동 감지

### 목표

화면에 그냥 찍힌 `https://…` 텍스트를 감지해 밑줄을 그리고 Cmd+클릭으로
연다. 지금은 OSC 8 시퀀스로 선언된 링크만 된다.

### 현재 코드

- Cmd+클릭 경로: `src/app/mouse.rs:237-244` — `open_hyperlink_at()`이 OSC 8
  하이퍼링크를 확인하고(`mouse.rs:396-414`, `grid()[point].hyperlink()`),
  없으면 블록 전체 선택으로 폴백
- 밑줄 렌더링: `src/renderer/pane.rs:135-146` — `indexed.hyperlink()`가
  있는 셀 아래에 1.5px 사각형
- 정규식 인프라: `src/app/search.rs`(Phase 11)가 이미
  `alacritty_terminal::term::search::{RegexSearch, RegexIter}`를 쓴다.
  **줄바꿈 래핑·와이드 문자·버퍼 경계를 다 처리해 주므로 그대로 재활용한다**

### 설계

새 모듈 `src/app/links.rs`:

```rust
/// 뷰포트(화면에 보이는 영역)만 스캔해 평문 URL 매치를 돌려준다.
pub fn visible_urls(term: &Term<EventProxy>) -> Vec<Match>   // Match = alacritty의 RangeInclusive<Point>
pub fn url_at(term: &Term<EventProxy>, point: Point) -> Option<String>
```

- 정규식은 Alacritty hints 기본값을 차용한다 (스킴 allowlist 방식):
  `(https?://|file://|git://|ssh:|ftp://|mailto:)[^\x00-\x1f\x7f-\x9f<>"\s{}\^⟨⟩`]+`
- `RegexSearch`는 컴파일 비용이 있으므로 `OnceLock`으로 1회 컴파일
- 스캔 범위는 **뷰포트로 한정** (`display_offset` 기준 화면 줄 수만큼).
  50행 × 200열 수준이라 매 redraw마다 돌려도 부담 없다. 스크롤백 전체를
  스캔하지 않는다 — 검색(Phase 11)과 달리 보이는 것만 클릭 가능하면 된다
- 끝 문장부호 정리: 매치 끝의 `.,;:!?` 와 짝 없는 `)` `]` 는 잘라낸다
  (마크다운·산문 속 URL에서 흔한 오탐). 괄호는 매치 안의 여닫이 짝을 세서
  판단한다

연결 지점 (둘 다 한 줄 수준):

1. `renderer/pane.rs`의 셀 루프: OSC 8 밑줄 조건에
   "이 셀이 평문 URL 매치 안에 있는가"를 추가. 매치 목록은 draw 진입 시
   `visible_urls()` 1회 호출로 만들어 `Vec<Match>`로 넘긴다
   (셀마다 `contains` 확인 — 검색 하이라이트 `pane.rs:100-102`와 동일 관용구)
2. `mouse.rs open_hyperlink_at()`: OSC 8 → **평문 URL** → 블록 선택 순으로
   폴백 한 단계 추가. `url_at()`이 Some이면 `open`으로 실행

### 구현 단계

1. `links.rs` 순수 로직 + 테스트 (Term에 문자열 먹여서 — search.rs 테스트 패턴)
2. 렌더러 밑줄 연결
3. 마우스 클릭 연결
4. 수동 검증: `echo`로 찍은 URL, 줄바꿈으로 래핑된 긴 URL, `(https://…)`
   괄호 안 URL, 한글 문장 속 URL

### 테스트 항목

- 기본 매치·다중 매치·매치 없음
- 래핑된 URL이 한 매치로 잡히는가 (RegexIter가 처리하지만 회귀 방지)
- 끝 문장부호 제거: `https://a.com.` / `(https://a.com)` / `https://a.com/b(c)`
  (마지막 것은 괄호 짝이 맞아 자르면 안 된다)
- 뷰포트 밖(스크롤백)은 매치에 없음

### 리스크·주의

- 매 프레임 스캔이므로 정규식이 뷰포트 밖으로 새지 않게 `RegexIter`의
  start/end를 정확히 준다 (Phase 11에서 익힌 API)
- OSC 8 링크와 겹치면 OSC 8이 이긴다 (명시가 추론보다 우선)

---

## Phase 17 — 명령 완료 알림 + OSC 9/777

### 목표

① 비포커스 상태에서 N초 이상 걸린 명령이 끝나면 macOS 알림.
② 앱이 보내는 OSC 9(iTerm2)·OSC 777(rxvt) 알림 시퀀스 지원.

### 현재 코드

- OSC 133 D 마크(종료코드 포함)가 이미 기록된다:
  `src/session.rs:553-591` (`parse_mark_kind`, `record_mark`)
- `Mark { kind, abs_line }`에 **시각 정보가 없다** — 소요 시간 계산 불가
- **창 포커스를 추적하지 않는다** — `src/app/mod.rs:598-642`의 WindowEvent
  분기에 `Focused`가 없다
- 바이트 스트림 감시 패턴이 이미 둘 있다: `watch_cwd`(OSC 7,
  `session.rs:365-407`)와 `watch_sync`(mode 2026). OSC 9/777도 같은 자리에
  같은 패턴으로 추가하면 된다
- 의존성: `objc2-foundation`(NSData feature만)·`objc2-app-kit` 기보유

### 설계

**시간 추적** — `Mark`에 `at: std::time::Instant` 추가. `record_mark`는
어차피 단조 증가 시각만 필요하므로 Instant로 충분하다(직렬화 안 함).
소요 시간 = D 마크의 `at` − 직전 C 마크의 `at`.

**포커스 추적** — `WindowEvent::Focused(bool)` 분기 추가 →
`self.window_focused: bool`. 알림 조건:

```
notify = 설정 on
      && 소요 시간 ≥ notify-threshold (기본 10초)
      && (창 비포커스 || 해당 페인의 탭이 비활성)
```

**알림 판단 위치** — 마크는 reader 스레드(`session.rs:reader_loop`)에서
기록되지만 포커스 상태는 App(메인 스레드)에 있다. reader에서 판단하지 않고,
D 마크 기록 시 `Event::CommandFinished { duration, exit }`를 EventProxy로
보내고 **App에서 조건 판단**한다. 기존 `Event::Wakeup`·`Event::Exit`와 같은
경로라 스레드 문제가 없다.

**알림 전달 (macOS)** — 두 단계:

1. **Dock 바운스** — `NSApplication::requestUserAttention`
   (objc2-app-kit 기보유, 코드 3줄). 권한·번들 조건 없음. 1차 구현
2. **배너 알림** — `NSUserNotification`(objc2-foundation feature 추가).
   10.14부터 deprecated지만 여전히 동작하고, 서명된 .app 번들(Phase 14)에서
   앱 이름·아이콘이 제대로 붙는다. `UNUserNotificationCenter`(신식)는
   번들 밖 `cargo run`에서 크래시하므로 쓰지 않는다 — 개발 편의가 우선
   - 알림 본문: 명령 텍스트가 저장돼 있지 않으므로 블록의 프롬프트 줄을
     `bounds_to_string`으로 뽑는다 (copy_last_output과 동일 관용구).
     실패 시 "명령 완료 (exit 0, 32초)" 형태로 폴백

**OSC 9/777** — `reader_loop`에 `watch_notify` 추가 (watch_cwd 복제 패턴):

- OSC 9: `ESC ] 9 ; <본문> BEL|ST`
- OSC 777: `ESC ] 777 ; notify ; <제목> ; <본문> BEL|ST`
- 파싱되면 같은 `Event::Notify { title, body }` 경로로 App에 전달.
  포커스 조건은 동일하게 적용 (포커스 중이면 무시 — 스팸 방지)

**설정 키** (`src/config.rs`):

```
notify = on | off            # 기본 on
notify-threshold = 10        # 초, 명령 완료 알림 최소 소요 시간
```

### 구현 단계

1. `Mark.at` + 소요 시간 계산 (순수 함수 + 테스트)
2. `WindowEvent::Focused` 추적
3. Dock 바운스로 엔드투엔드 연결 (`sleep 15` → 다른 앱 갔다가 → 바운스 확인)
4. NSUserNotification 배너 + 본문(프롬프트 줄 추출)
5. `watch_notify` (OSC 9/777) + 파서 테스트
6. 설정 키 2개 + config.example 갱신

### 테스트 항목

- 소요 시간: C 없이 D만 온 경우(None), C→D 정상, 연속 명령
- OSC 9/777 파싱: BEL/ST 종료, 청크 경계에 걸친 시퀀스(carry), 비 UTF-8
- 조건 함수: 임계값 미만/포커스 중/비활성 탭 각각

### 리스크·주의

- NSUserNotification 권한 프롬프트는 최초 1회 뜬다 — features.md에 기록
- reader 스레드에서 AppKit 호출 금지 (알림은 반드시 메인 스레드 이벤트로)

---

## Phase 18 — 블록 거터 색 설정 + 키바인딩 커스터마이즈

### 목표

① 하드코딩된 블록 거터 색 3개를 설정으로 개방하고 거터를 끌 수 있게 한다.
② Cmd 단축키를 `keybind` 설정으로 재정의할 수 있게 한다.

### 현재 코드

- 거터 색 하드코딩: `src/renderer/pane.rs:11-13`
  (`BLOCK_RUNNING`/`BLOCK_OK`/`BLOCK_FAIL`) — 사용처는 `pane.rs:208-213` 한 곳
- Theme 파이프라인: `config.rs → renderer::Theme`(color.rs)로 색이 흐른다.
  기존 키(`background` 등)와 동일하게 태우면 된다
- 단축키 표: `src/app/input.rs:117-153`의 `on_command_key` match — 물리 키
  (`KeyCode`) + Shift 여부로 분기 (Phase 15에서 물리 키 매칭으로 전환됨)
- 액션 목록이 이미 있다: `src/app/palette.rs:11-40`의 `PaletteAction` enum
  (NewTab·SplitRight·ToggleZoom 등 10개) — **키바인딩과 팔레트가 같은 액션
  집합을 공유해야 한다**

### 설계

**색 설정** — 기계적 작업:

```
block-gutter = on | off        # 기본 on
block-running-color = #5a8cf2
block-ok-color = #59b875
block-fail-color = #eb6b75
```

`Config`에 필드 4개 → `Theme`으로 전달 → `pane.rs`의 상수 3개를
`theme.*`로 교체, `draw_block_gutter` 진입에 `block_gutter` 체크.

**키바인딩** — `PaletteAction`을 `src/app/action.rs`의 `Action`으로 승격하고
팔레트·키맵 둘 다 이걸 쓴다. `on_command_key`의 match 팔을 전부 Action 실행
함수 `run_action(&mut self, Action)`으로 옮긴다 (팔레트 실행부와 통합 —
현재 중복돼 있는 로직이 한 곳으로 모인다).

설정 문법 (Ghostty 차용):

```
keybind = cmd+t = new-tab
keybind = cmd+shift+d = split-down
keybind = cmd+opt+left = focus-left
```

- 파싱: `<mods>+<key>` — mods는 `cmd`(필수)·`shift`·`opt`, key는 물리 키
  이름 소문자(`a`~`z`, `0`~`9`, `left`·`right`·`up`·`down`, `[`·`]`)
- 액션 이름은 Action enum의 kebab-case (`new-tab`, `close-pane`,
  `split-right`, `split-down`, `toggle-zoom`, `copy`, `paste`,
  `copy-last-output`, `search`, `ai-generate`, `palette`,
  `jump-prev-prompt`, `jump-next-prompt`, `focus-left/right/up/down`,
  `next-tab`, `prev-tab`, `tab-1`~`tab-9`, `quake-toggle`은 제외 — 전역
  핫키는 별도 경로)
- 기본 맵은 현재 표와 동일하게 코드로 구성하고, `keybind` 줄이 **같은 키를
  덮어쓰는** 방식 (전체 교체가 아니라 오버라이드 — 설정 한 줄로 한 키만
  바꿀 수 있어야 한다)
- 해석: `on_command_key`가 `HashMap<(KeyCode, Mods), Action>` 조회로 바뀐다.
  Cmd 없는 조합은 1차 범위에서 제외 (현재 단축키가 전부 Cmd 기반이고,
  Cmd 없는 키는 PTY로 가야 해서 충돌 위험이 크다)

### 구현 단계

1. 색 4키 (Config→Theme→pane.rs) + config.example — 독립적으로 먼저 출하 가능
2. `Action` enum 추출 + `run_action` 통합 (동작 변화 없음 — 리팩터)
3. `keybind` 파서 + 기본 맵 + 오버라이드 (테스트 먼저)
4. `on_command_key`를 맵 조회로 전환
5. keybindings.md·configuration.md 갱신

### 테스트 항목

- 파서: 정상 줄, 알 수 없는 키/액션(무시), mods 순서 무관, 대소문자
- 기본 맵이 현재 표와 1:1 일치 (회귀 스냅샷)
- 오버라이드: `cmd+t = split-right` 후 new-tab 바인딩이 사라지는가 —
  아니다, cmd+t만 바뀐다 (키 단위 교체 확인)

### 리스크·주의

- Cmd+C/V 같은 시스템 관례 키를 사용자가 깨는 것은 허용한다 (자기 책임) —
  단 문서에 경고
- 팔레트 액션과 키맵 액션이 갈라지지 않게 Action enum을 단일 소스로 유지

---

## Phase 19 — 분할 레이아웃 복원

### 목표

detach/attach(GUI 재시작) 시 페인 트리(분할 구조·비율·포커스·탭 순서)까지
복원한다. 지금은 세션당 탭 1개로만 복원된다.

### 현재 코드

- 복원 진입점: `src/app/mod.rs:648-664` `restore_or_create_tabs` —
  `Session::list()`(mux LIST 프레임)로 살아있는 세션 ID를 받아
  `attach_tab(id)`로 탭 1개씩 생성
- 트리: `src/layout.rs:109` `PaneNode<P = Pane>` — **페이로드 제네릭**이라
  (Phase T에서 테스트용으로 만들어 둔 구조) `PaneNode<u64>`(세션 ID 트리)로
  직렬화 로직을 순수하게 다룰 수 있다
- `Tab { root, focused, zoomed }`, Split에 `ratio` 있음 (`layout.rs:113`)
- 세션 ID는 attach 시 확보된다 (`app/mod.rs:242`)
- serde_json 기보유 (`Cargo.toml`) — 단 layout.rs에 serde derive를 붙이지
  않고 **수동 JSON 변환**으로 간다 (아래 이유)

### 설계

**저장 위치** — 클라이언트 파일 `~/.cache/eden/layout.json`. 데몬은 손대지
않는다 — 데몬 프로토콜 확장 없이 끝나고, 데몬이 죽어도(세션 소멸) 파일은
그냥 무시되므로 안전하다.

**스키마** (세션 ID 기반 — 페인 ID는 런타임 값이라 쓰지 않는다):

```json
{
  "version": 1,
  "active": 0,
  "tabs": [
    {
      "focused": 3,
      "root": { "dir": "row", "ratio": 0.6,
                "first": { "leaf": 3 },
                "second": { "leaf": 5 } }
    }
  ]
}
```

`leaf`의 값 = mux 세션 ID(u64). zoomed는 저장하지 않는다 (일시 상태).

**변환** — `src/app/layout_persist.rs`:

```rust
fn to_json(tabs: &[Tab], active: usize) -> serde_json::Value      // Pane → session_id만 추출
fn from_json(v: &Value, alive: &HashSet<u64>) -> Vec<TabPlan>     // TabPlan = PaneNode<u64> + focused
```

serde derive를 layout.rs에 붙이지 않는 이유: `Pane`이 `Session`(소켓·스레드)
을 소유해 Serialize 불가능하고, 트리 구조만 옮기면 되므로 `serde_json::Value`
수동 변환이 오히려 짧다.

**가지치기** — 복원 시 살아있는 세션 목록(`Session::list()`)과 대조:

- 죽은 leaf는 트리에서 제거하고, 자식이 하나 남은 Split은 그 자식으로
  접는다 (기존 `remove` 로직과 같은 규칙 — `layout.rs:259` 참고)
- 탭의 모든 leaf가 죽었으면 탭 자체를 버린다
- 파일에 없는 살아있는 세션(파일 유실·구버전)은 지금처럼 단독 탭으로 붙인다
- `focused`가 죽은 세션이면 트리의 첫 leaf로 폴백 (`first_id` 관용구)

**저장 시점** — 구조가 변할 때마다 즉시 저장 (split/close_pane/new_tab/
탭 전환/드래그 리사이즈 종료). 전부 이미 App의 메서드 한 곳씩을 지나므로
`save_layout()` 호출 한 줄씩이다. 종료 훅에 걸지 않는 이유: 크래시·강제
종료에서도 마지막 구조가 남는 쪽이 세션 지속성의 목적에 맞다.

**복원 흐름** (`restore_or_create_tabs` 교체):

1. `Session::list()` → alive 집합
2. layout.json 읽기·파싱 실패 시 → 현재 동작(세션당 탭 1개)으로 폴백
3. `from_json`으로 TabPlan 목록 → 탭마다 트리를 걸으며 leaf 순서대로
   `Session::attach` → `PaneNode<Pane>` 재구성 (ratio·dir 유지)
4. attach 실패한 leaf는 죽은 것으로 간주하고 같은 접기 규칙 적용

### 구현 단계

1. `to_json`/`from_json` + 가지치기 순수 로직 (PaneNode<u64>로 테스트 —
   GPU·소켓 불필요)
2. 저장 훅 5곳
3. 복원 흐름 교체
4. 수동 검증: 2분할+탭 2개 → GUI 종료 → 재실행 → 구조·비율·포커스 복원,
   한 세션만 죽인 뒤 재실행 → 남은 페인으로 접힘

### 테스트 항목

- 라운드트립: 트리 → JSON → 트리 (dir·ratio·focused 보존)
- 가지치기: 3-leaf 트리에서 1개 사망 → Split 접힘·비율 유지,
  전원 사망 → 탭 제거, focused 사망 → 첫 leaf 폴백
- 파일에 없는 신규 세션 → 단독 탭 추가
- 깨진 JSON·미래 version → 폴백 (패닉 금지)

### 리스크·주의

- GUI 2개가 동시에 뜨는 경우 마지막 저장이 이긴다 — 현재 사용 모델(단일
  GUI)에서 허용. 파일 락은 넣지 않는다
- 세션 ID는 데몬 재시작 시 1부터 재발급된다 — 죽은 데몬의 layout.json이
  새 데몬의 다른 세션과 우연히 매칭될 수 있다. 데몬 시작 시각을 파일과
  세션 목록 양쪽에 넣어 대조하는 것으로 방지 (LIST 응답에 boot id 1프레임
  추가 — 유일한 프로토콜 변경, 하위호환: 짧은 응답이면 무시)

---

## Phase 20 — Kitty keyboard protocol (CSI u)

### 목표

Neovim·최신 TUI가 요구하는 키 구분(Shift+Enter, Ctrl+I vs Tab 등)을
지원한다. "완전 구현 전엔 advertise하지 않는다"는 원칙(design.md) 그대로,
프로토콜 5개 플래그를 전부 구현한 뒤에만 켠다.

### 현재 코드 — 조사 결과 절반이 이미 있다

**alacritty_terminal 0.26이 프로토콜 상태 머신을 내장한다** (이번 조사의
핵심 발견 — 파서 쪽 작업이 0이다):

- `Config { kitty_keyboard: bool }` (crate `term/mod.rs:349`) — 켜면 파서가
  query(`CSI ? u`)·push(`CSI > flags u`)·pop(`CSI < u`)·set을 전부 처리하고
  모드 스택·대체 스크린 분리까지 관리한다
- 현재 상태는 `term.mode()`의 플래그로 노출된다: `DISAMBIGUATE_ESC_CODES`
  `REPORT_EVENT_TYPES` `REPORT_ALTERNATE_KEYS` `REPORT_ALL_KEYS_AS_ESC`
  `REPORT_ASSOCIATED_TEXT` (합집합 `KITTY_KEYBOARD_PROTOCOL`)
- eden은 `session.rs:159`의 `Config`에 `kitty_keyboard: true` 한 줄 +
  **키 인코더**만 만들면 된다. `mode()` 조회는 mouse_report.rs가 이미 하는
  패턴 그대로다 (`clipboard.rs:46`도 동일)

인코더가 없는 곳: `src/app/input.rs:211-255` `key_to_bytes` — 레거시
시퀀스만 안다. release 이벤트는 `input.rs:17`에서 버려진다.

### 설계

**새 모듈 `src/app/kitty_key.rs`** — mouse_report.rs와 같은 원칙: winit
`KeyEvent` + `ModifiersState` + `TermMode` → `Option<Vec<u8>>` 순수 함수.
App·락 의존 없음, 전체를 단위 테스트로 덮는다.

인코딩 규칙 요약 (kitty 스펙 기준):

- 형식: `CSI unicode-key-code[:shifted[:base]] ; mods[:event] [;text] u`
  또는 기능 키는 기존 `CSI number ~` 계열 유지 + mods 삽입
- `DISAMBIGUATE`: Esc→`CSI 27;mods u`, 수식키 조합만 CSI u로. 수식 없는
  일반 문자·Enter·Tab·Backspace는 레거시 유지
- `REPORT_EVENT_TYPES`: mods 뒤 `:1/:2/:3`(press/repeat/release).
  winit의 `event.repeat`와 `ElementState::Released` 매핑.
  `on_key`의 조기 리턴(`input.rs:17`)을 이 모드일 때만 통과시킨다
- `REPORT_ALTERNATE_KEYS`: shifted 코드포인트 병기 (winit
  `key_without_modifiers`·`text_with_all_modifiers` 활용)
- `REPORT_ALL_KEYS_AS_ESC`: 일반 문자·수식키 단독 press까지 전부 CSI u.
  수식키 자체(Shift 단독 등)는 winit `ModifiersChanged`가 아니라
  `KeyboardInput`의 Named(Shift…) 이벤트로 온다 — 코드포인트 57441~ 대역
- `REPORT_ASSOCIATED_TEXT`: 세 번째 파라미터에 텍스트 코드포인트

분기 순서 (on_key 개편):

```
Cmd 단축키 (앱 우선 — 프로토콜에 안 넘김)
→ term.mode()에 kitty 플래그 있으면 kitty_key::encode()
→ 없으면 기존 key_to_bytes()
```

**IME 상호작용** (Phase 15의 교훈이 있는 영역): preedit 중에는 kitty 모드라도
키를 인코딩하지 않는다 (`input.rs:23` 가드 유지). 조합 확정은 Commit 텍스트로
가는 기존 경로 그대로 — kitty 모드에서 Commit 텍스트는
`REPORT_ALL_KEYS_AS_ESC`여도 평문으로 보낸다 (kitty 자신의 IME 동작과 동일).

### 구현 단계

1. `kitty_key.rs` 인코더 + 스펙 표 기반 테스트 (아래). 이 단계에서는
   아무 데도 연결하지 않는다
2. `session.rs` Config에 `kitty_keyboard: true` — 단, eden 설정
   `kitty-keyboard = on|off`(기본 off)로 감싼다. 완주 전까지 기본 off
3. `on_key` 분기 연결 + release 이벤트 통과
4. 실전 검증 (아래) 통과 후 기본값 on 전환 + 설정 키는 탈출구로 유지
5. features.md·design.md("미뤄둔 프로토콜" 해소) 갱신

### 테스트 항목 (블로커였던 "검증 하네스" 해법)

- **단위**: kitty 스펙 문서의 인코딩 표를 그대로 테스트 벡터로 옮긴다
  (`Ctrl+Shift+A`, `Esc`, `Shift+Enter`, `F1~F12`, 화살표, 수식키 단독,
  release 이벤트, 각 플래그 조합별 동일 키의 출력 차이 — 최소 40케이스)
- **프로토콜 왕복**: 진짜 `Term`에 `CSI > 1 u` push를 먹인 뒤 `mode()`에
  플래그가 서는지, pop 후 사라지는지 (파서는 crate 것이지만 우리 설정
  경로의 회귀 방지)
- **수동 체크리스트**:
  - `printf '\x1b[?u'` 응답으로 advertise 확인, push→`cat -v`로 시퀀스
    육안 확인→pop
  - Neovim: Shift+Enter와 Enter 구분, Ctrl+I가 Tab과 구분되는지 (`:map`으로
    확인), 종료 후 모드 스택이 풀려 셸이 정상인지
  - fzf·zsh 등 프로토콜 미사용 앱이 레거시 그대로인지
  - Claude Code TUI + 한글 IME 조합 (Phase 15 회귀)

### 리스크·주의

- **최대 리스크는 여전히 반쪽 인코딩** — 단계 2~3를 기본 off로 깔아 두고
  일상 사용(dogfooding) 기간을 거친 뒤 기본 on. advertise는 crate가 모드
  스택 응답으로 하므로 "켰는데 인코딩이 틀린" 상태를 만들지 않는 것이 전부다
- winit이 macOS에서 `key_without_modifiers`를 제공하는지 버전 확인 필요
  (0.30에서 `KeyEventExtModifierSupplement` — macOS 지원됨). 안 되는 케이스는
  alternate key 생략이 스펙상 허용이다
- 대체 스크린 진입/이탈 시 모드 분리는 crate가 처리 — 우리는 매 키마다
  `mode()`를 읽기만 한다 (캐시 금지)

---

## 부록 — 조사에서 확인한 사실 (착수 시 재확인 불필요)

- alacritty_terminal 0.26의 kitty keyboard 지원 범위: Config 플래그, 모드
  스택 push/pop/query, TermMode 플래그 5종, 대체 스크린 분리
  (`~/.cargo/registry/.../alacritty_terminal-0.26.0/src/term/mod.rs:75-104,
  349, 1030, 1276-1324`)
- `PaneNode<P>` 제네릭·`first_id`·접기 규칙은 layout.rs에 이미 있어
  Phase 19의 순수 테스트가 바로 가능
- 바이트 스트림 감시(carry 포함) 패턴 3벌 존재: OSC 133 스캐너·watch_cwd·
  watch_sync — Phase 17의 OSC 9/777은 네 번째 복제
- `RegexSearch`/`RegexIter`는 Phase 11에서 검증된 API — Phase 16은 뷰포트
  한정이라 무한 루프 리스크도 없음
- 알림용 의존성: objc2-app-kit(NSApplication) 기보유,
  NSUserNotification은 objc2-foundation feature 추가만 필요
