# 개발

## 빌드 · 실행

```sh
cargo build            # 디버그
cargo build --release  # 릴리스
./target/release/eden
```

macOS 전용(Metal 렌더러, Cocoa 창, macOS 시스템 폰트/`open` 사용). Rust
stable로 빌드된다.

## 프로젝트 구조

```
src/
  main.rs       진입점 (데몬 분기 + 이벤트 루프 생성 + Dock 아이콘)
  app/          winit 앱
    mod.rs           App/State/Tab, 탭·페인 관리, 재그리기, 이벤트 디스패치
    input.rs         키보드·IME → 단축키(물리 키 매칭) 또는 PTY 바이트
    mouse.rs         클릭·선택·휠·Cmd+클릭·구분선 드래그·마우스 리포팅 배선
    mouse_report.rs  마우스 리포트 인코딩 (X10/UTF-8/SGR, 순수 함수)
    clipboard.rs     복사/붙여넣기 (bracketed paste)
    palette.rs       커맨드 팔레트
    search.rs        스크롤백 검색 (Cmd+F)
    ai_bar.rs        AI 명령 생성 바
    quake.rs         Ctrl+` 전역 드롭다운
    status.rs        하단 상태바 문자열
  renderer/     wgpu glyph atlas 렌더러
    mod.rs        Renderer, wgpu 초기화, 폰트 로드·폴백, 프레임 조립·제출
    text.rs       glyph atlas + 텍스트 한 줄 그리기·폭 계산·말줄임
    pane.rs       터미널 그리드 (셀·커서·블록 거터·preedit)
    chrome.rs     탭 바·상태바·AI 바·검색 바·팔레트
    color.rs      테마와 ANSI 색 변환
    shader.wgsl   배경/글리프 셰이더
  session.rs    페인 세션 (Term + OSC 133/OSC 7 스캐너 + 블록 + mux 클라이언트)
  mux.rs        mux 데몬 + 클라이언트 (세션 지속성, 셸 통합 설치)
  layout.rs     페인 이진 분할 트리 (분할 비율·구분선·줌)
  config.rs     설정 파서
  ai.rs         자연어 → 셸 명령 (Anthropic BYOK / Ollama)
shell/
  integration.zsh   OSC 133 + OSC 7 셸 통합 (zsh, 자동 주입)
  integration.bash  bash용 통합 (설치만 하고 자동 주입은 안 함)
  zshenv            ZDOTDIR 부트스트랩 주입
scripts/
  bundle.sh         .app 번들 생성 (Info.plist 포함)
  make-icon.sh      dist/AppIcon.icns 생성
  release.sh        서명 → 공증 → zip → (--publish) 태그·GitHub 릴리스·cask 갱신
  update-cask.sh    Homebrew cask 파일 재생성 (release.sh가 호출)
Casks/
  eden.rb   Homebrew Cask 템플릿 (실제 배포본은 kobums/homebrew-tap에 있음)
assets/             앱 아이콘 원본 (icon.png — Dock 아이콘으로도 사용)
docs/               이 문서들
config.example      설정 예시
```

모듈별 책임은 [architecture.md](architecture.md) 참고.

## mux 데몬 디버깅

`eden --daemon`은 세션/PTY를 소유하는 데몬으로 실행된다(보통 GUI가 자동
스폰). 상태 확인/정리:

```sh
pgrep -fl "eden --daemon"              # 데몬 실행 여부
ls ~/.cache/eden/mux/*.sock            # 제어 소켓
rm -f ~/.cache/eden/mux/control.sock   # 죽은 소켓 정리
```

OSC 133 마크 로그: `EDEN_DEBUG_MARKS=1 ./target/debug/eden`

## 패키징 · 릴리스

로컬 번들:

```sh
./scripts/make-icon.sh    # dist/AppIcon.icns (최초 1회)
./scripts/bundle.sh       # dist/eden.app (릴리스 빌드 포함, Info.plist·아이콘)
open dist/eden.app
```

정식 릴리스는 `scripts/release.sh`가 전체 파이프라인을 자동화한다:

```sh
./scripts/release.sh              # 서명 → 공증 → staple → dist/eden-<version>.zip + sha256
TAP_DIR=~/develop/homebrew-tap \
  ./scripts/release.sh --publish  # + git 태그(v<version>) + GitHub 릴리스 + cask 갱신·푸시
```

단계별로: ① keychain의 Developer ID Application 인증서로 hardened runtime
서명 → ② `notarytool submit --wait`로 Apple 공증 → ③ `stapler staple` 후
재압축 → ④ sha256 계산. `--publish`를 붙이면 ⑤ `Cargo.toml`의 version으로
git 태그를 만들어 푸시하고 ⑥ `gh release create`(또는 upload)로 zip을 올린 뒤
⑦ `TAP_DIR`이 지정돼 있으면 `update-cask.sh`로 tap 저장소의
`Casks/eden.rb`를 재생성해 커밋·푸시한다.

사전 준비:

1. **Developer ID Application 인증서** (keychain)
2. **공증 자격증명** — 둘 중 하나:
   - keychain 프로파일 (기본 `spot-notary`, `NOTARY_PROFILE`로 변경)
   - 환경 변수 `NOTARY_KEY`(.p8 경로) + `NOTARY_KEY_ID` + `NOTARY_ISSUER` (CI용)
3. `--publish`에는 `gh` CLI 로그인과 tap 저장소
   (kobums/homebrew-tap) 로컬 체크아웃(`TAP_DIR`)

릴리스 절차 관례: 버전을 `Cargo.toml`에서 올리고 `release: v<version>`
커밋을 만든 뒤 `release.sh --publish`를 돌린다. 새 버전이 태그·릴리스·cask에
일관되게 반영된다. 설치는 `brew install --cask kobums/tap/eden`.

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
| `layout.rs` | 분할 트리: 배치·GAP 정합성·split/remove·경계·줌·비율/구분선 |
| `config.rs` | `key = value` 파서, 범위 검사, 프리셋 순서 의존성 |
| `renderer/color.rs` | mix, 256색 인덱스, 파생 크롬 색의 명암 방향 |
| `app/mouse_report.rs` | 버튼 코드·수정자 비트·SGR/X10/UTF-8 인코딩·셀 환산 |
| `app/search.rs` | 절대↔그리드 좌표 변환, 매치 포함 판정, 순환 이동 |
| `session.rs` | OSC 133 마크 파싱 (zsh·bash·fish 형식 호환) |

`layout.rs`는 `PaneNode<P = Pane>`로 페이로드가 제네릭이다. `Pane`이 `Session`을
소유해(→ mux 데몬 스폰) 테스트에서 만들 수 없기 때문이고, 테스트는
`PaneNode<usize>`를 쓴다. 기본 타입 파라미터라 호출부는 영향이 없다.

CI는 macOS 러너에서 위 셋과 빌드를 돌린다 ([.github/workflows/ci.yml](../.github/workflows/ci.yml)).

## 구현 이력 (Phase 0~13)

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
| 13 | 구분선 드래그 리사이즈 + bash 셸 통합 (fish는 자체 지원 확인) |

Phase 13 이후의 후속 작업 (릴리스 단위 — design.md의 Phase 14·15에 해당):

| 버전 | 내용 |
|---|---|
| v0.1.1 | 모니터 배율 변경 시 폰트 재계산, MesloLGS Nerd Font 우선 로드 + PUA 폴백 |
| v0.1.2 | 앱 아이콘(◡̈), 새 셸 홈 디렉터리 시작 |
| v0.1.3 | `cargo run`에서도 Dock 아이콘 표시 |
| v0.1.4 | 앱 이름 `terminal-dev` → `eden` 확정, 릴리스 파이프라인(서명·공증·cask 자동화) |
| v0.1.5 | 한글 IME 조합 중 Cmd 단축키 수정 + 물리 키(KeyCode) 매칭 전환 |
| v0.2.0 | Phase 16~20: 평문 URL 감지, 명령 완료 알림 + OSC 9/777, 거터 색·키바인딩 설정, 분할 레이아웃 복원, Kitty keyboard protocol(기본 off) |
| v0.2.1 | Phase 21: 폰트 크기 단축키, `SUN_LEN` 패닉 수정, 파일 드롭 경로 붙여넣기, 비활성 페인 디밍, 라이트/다크 자동 전환 |

## 남은 작업

전체 후보 목록과 우선순위는 [design.md](design.md)의 "향후 로드맵 후보"가
단일 출처다. 여기에는 이전 Phase가 명시적으로 남긴 것만 적는다.

- **Kitty keyboard protocol 기본 on 전환** — Phase 20에서 인코더·모드 스택은
  완성했고 기본 off로 두었다. Neovim 등 실제 클라이언트에서 며칠 써 본 뒤 켠다.
- **Kitty graphics protocol** — APC 파싱 + 이미지 디코드 + 별도 GPU 텍스처
  아틀라스/배치 서브시스템 필요.
- 블록 접기·재실행 (Phase 4), 페인별 검색·검색 히스토리 (Phase 11),
  구분선 호버 커서·더블클릭 50/50 복원 (Phase 13).
- glyph atlas가 가득 찼을 때의 축출/증설 (현재는 이후 글리프를 그리지 않는다).

## 코드 스타일

- 주석은 "왜"를 설명하고, "무엇"은 코드가 말하게 한다.
- 각 기능은 직접 실행해 확인 가능한 상태로 만든 뒤 다음으로 넘어간다.
- 표준(OSC 133 / OSC 8 / mode 2026)에 올라타고, 독자 규격은 남발하지 않는다.
- 반쪽 구현(검증 불가)은 넣지 않는다.
