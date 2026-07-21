# 개발

## 빌드 · 실행

```sh
cargo build            # 디버그
cargo build --release  # 릴리스
./target/release/terminal
```

macOS 전용(Metal 렌더러, Cocoa 창, macOS 시스템 폰트/`open` 사용). Rust
stable로 빌드된다.

## 프로젝트 구조

```
src/
  main.rs       진입점 (데몬 분기 + 이벤트 루프 생성)
  app/          winit 앱
    mod.rs        App/State/Tab, 탭·페인 관리, 재그리기, 이벤트 디스패치
    input.rs      키보드·IME → 단축키 또는 PTY 바이트
    mouse.rs      클릭·선택·휠·Cmd+클릭
    clipboard.rs  복사/붙여넣기
    palette.rs    커맨드 팔레트
    ai_bar.rs     AI 명령 생성 바
    quake.rs      Ctrl+` 전역 드롭다운
    status.rs     하단 상태바 문자열
  renderer/     wgpu glyph atlas 렌더러
    mod.rs        Renderer, wgpu 초기화, 프레임 조립·제출
    text.rs       glyph atlas + 텍스트 한 줄 그리기·폭 계산
    pane.rs       터미널 그리드 (셀·커서·블록 거터·preedit)
    chrome.rs     탭 바·상태바·AI 바·팔레트
    color.rs      테마와 ANSI 색 변환
    shader.wgsl   배경/글리프 셰이더
  session.rs    페인 세션 (Term + OSC 133 + 블록 + mux 클라이언트)
  mux.rs        mux 데몬 + 클라이언트 (세션 지속성)
  layout.rs     페인 이진 분할 트리
  config.rs     설정 파서
  ai.rs         자연어 → 셸 명령
shell/
  integration.zsh   OSC 133 셸 통합
  zshenv            ZDOTDIR 부트스트랩 주입
scripts/
  bundle.sh         .app 번들 생성
  make-icon.sh      아이콘 생성
Casks/
  terminal-dev.rb   Homebrew Cask 템플릿
docs/               이 문서들
config.example      설정 예시
```

모듈별 책임은 [architecture.md](architecture.md) 참고.

## mux 데몬 디버깅

`terminal --daemon`은 세션/PTY를 소유하는 데몬으로 실행된다(보통 GUI가 자동
스폰). 상태 확인/정리:

```sh
pgrep -fl "terminal --daemon"          # 데몬 실행 여부
ls ~/.cache/terminal-dev/mux/*.sock    # 제어 소켓
rm -f ~/.cache/terminal-dev/mux/control.sock   # 죽은 소켓 정리
```

OSC 133 마크 로그: `TERMDEV_DEBUG_MARKS=1 ./target/debug/terminal`

## 패키징

```sh
./scripts/make-icon.sh    # dist/AppIcon.icns
./scripts/bundle.sh       # dist/terminal-dev.app (릴리스 빌드 포함)
open dist/terminal-dev.app
```

정식 배포에는 코드 서명 + 공증(notarization)이 필요하다(Apple Developer
자격증명). Homebrew Cask는 GitHub 릴리스에 `.app.zip`을 올린 뒤
[`Casks/terminal-dev.rb`](../Casks/terminal-dev.rb)의 `version`/`sha256`/`url`을
채운다.

## 테스트

```sh
cargo test                                   # 단위 테스트
cargo clippy --all-targets -- -D warnings    # 린트 (CI에서 차단)
cargo fmt --all -- --check                   # 포맷
```

테스트는 GPU도 PTY도 필요 없는 순수 로직만 덮는다 — 창을 띄우거나 mux 데몬을
스폰하는 코드는 대상이 아니다. 각 모듈 안의 `#[cfg(test)] mod tests`에 두어
private 함수까지 검증한다 (`tests/` 디렉터리는 쓰지 않는다).

| 대상 | 내용 |
|---|---|
| `layout.rs` | 분할 트리: 배치 계산·GAP 정합성·split/remove·경계·줌 |
| `config.rs` | `key = value` 파서, 범위 검사, 프리셋 순서 의존성 |
| `renderer/color.rs` | mix, 256색 인덱스, 파생 크롬 색의 명암 방향 |
| `app/mouse_report.rs` | 버튼 코드·수정자 비트·SGR/X10/UTF-8 인코딩·셀 환산 |
| `app/search.rs` | 절대↔그리드 좌표 변환, 매치 포함 판정, 순환 이동 |

`layout.rs`는 `PaneNode<P = Pane>`로 페이로드가 제네릭이다. `Pane`이 `Session`을
소유해(→ mux 데몬 스폰) 테스트에서 만들 수 없기 때문이고, 테스트는
`PaneNode<usize>`를 쓴다. 기본 타입 파라미터라 호출부는 영향이 없다.

CI는 macOS 러너에서 위 셋과 빌드를 돌린다 ([.github/workflows/ci.yml](../.github/workflows/ci.yml)).

## 구현 이력 (Phase 0~11)

단계별로 만들고 매번 실제 실행/스크린샷으로 검증했다. 전체 로드맵과 각 단계의
완료 내용·남은 한계는 [design.md](design.md)에 있다.

| Phase | 내용 |
|---|---|
| 0 | PTY 셸 실행 + 그리드 검증 (headless) |
| 1 | winit 창 + wgpu glyph atlas 렌더 + 키 입력 |
| 2 | 스크롤백, 선택/클립보드, 한글 IME, 폰트 폴백 |
| 3 | OSC 133 셸 통합 + 프롬프트 점프 |
| 4 | 블록 UI (상태 바, 블록 복사) |
| 5 | 탭 + 페인 분할 |
| 6 | 로컬 우선 AI 명령 생성 |
| 7 | 세션 지속성 (mux 데몬, detach/attach) |
| 8 | OSC 8 하이퍼링크 + synchronized output |
| 9 | 설정 파일 + 테마 + 커맨드 팔레트 + Quake + 배포 패키징 |
| 10 | 마우스 리포팅 (X10/UTF-8/SGR) + 휠 경로 수정 |
| 11 | 스크롤백 검색 (Cmd+F) |
| T | 첫 단위 테스트 + GitHub Actions CI |
| 12 | 페인 줌 (Cmd+Z) |

## 남은 작업

- **Kitty keyboard protocol** — 완전한 CSI-u 인코더 필요. 반쪽 구현은 프로토콜을
  켜는 앱(Neovim 등)의 키 입력을 깨뜨리므로, 실제 클라이언트로 검증할 수 있을 때
  구현.
- **Kitty graphics protocol** — APC 파싱 + 이미지 디코드 + 별도 GPU 텍스처
  아틀라스/배치 서브시스템 필요.
- 코드 서명 · 공증.
- 설정 확장 (키바인딩 커스터마이즈, 블록/오버레이 색 등 —
  현재 설정 가능한 범위는 [configuration.md](configuration.md) 참고).
- glyph atlas가 가득 찼을 때의 축출/증설 (현재는 이후 글리프를 그리지 않는다).

## 코드 스타일

- 주석은 "왜"를 설명하고, "무엇"은 코드가 말하게 한다.
- 각 기능은 직접 실행해 확인 가능한 상태로 만든 뒤 다음으로 넘어간다.
- 표준(OSC 133 / OSC 8 / mode 2026)에 올라타고, 독자 규격은 남발하지 않는다.
- 반쪽 구현(검증 불가)은 넣지 않는다.
