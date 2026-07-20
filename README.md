# terminal-dev

여러 터미널(iTerm2 · tmux · Alacritty · Kitty · WezTerm · Ghostty · Warp 등)을 조사해
각각의 장점을 하나로 모은 macOS 네이티브 터미널. Rust + wgpu(Metal)로 구현.

조사 보고서는 [RESEARCH.md](RESEARCH.md), 설계 결정과 단계별 로드맵은
[DESIGN.md](DESIGN.md) 참고.

## 특징

- **GPU 렌더링** — glyph atlas 기반 wgpu(Metal) 렌더러
- **한글 IME** — 조합 중 preedit 오버레이 + 후보창 배치
- **블록 UI** — OSC 133 셸 통합으로 명령/출력을 블록 단위로 인식, 성공·실패 상태 바
- **로컬 우선 AI** — Cmd+K로 자연어 → 셸 명령 생성 (BYOK 또는 로컬 Ollama, 계정 강제 없음, 생성 명령은 실행하지 않고 입력줄에 삽입만)
- **내장 멀티플렉서** — 탭 + 페인 분할 (tmux 없이)
- **세션 지속성** — 창을 닫아도 셸이 데몬에 살아남고, 다시 열면 복원 (detach/attach)
- **하이퍼링크** — OSC 8 링크에 밑줄 + Cmd+클릭으로 열기
- **커맨드 팔레트** — Cmd+Shift+P
- **설정 파일** — `~/.config/terminal-dev/config` (Ghostty식 `key = value`)

## 빌드

```sh
cargo build --release
./target/release/terminal
```

## 설정

`~/.config/terminal-dev/config`에 작성 (없으면 기본값). 예시는
[`config.example`](config.example) 참고.

```
font-size = 14
background = #16161e
foreground = #d9d9de
scrollback = 10000
```

## 단축키

| 키 | 동작 |
|---|---|
| Cmd+T / Cmd+W | 새 탭 / 탭(세션) 닫기 |
| Cmd+1..9 | 탭 이동 |
| Cmd+Shift+[ / ] | 이전 / 다음 탭 |
| Cmd+D / Cmd+Shift+D | 좌우 / 상하 분할 |
| Cmd+Option+화살표 | 페인 포커스 이동 |
| Cmd+↑ / Cmd+↓ | 이전 / 다음 프롬프트로 점프 |
| Cmd+C / Cmd+V | 복사 / 붙여넣기 |
| Cmd+Shift+C | 마지막 명령 출력 복사 |
| Cmd+클릭 | 하이퍼링크 열기 / 블록 선택 |
| Cmd+K | AI 명령 생성 |
| Cmd+Shift+P | 커맨드 팔레트 |
| Ctrl+` | Quake 드롭다운 토글 (전역 핫키, 어느 앱에서든) |

## AI 명령 생성

Cmd+K 후 자연어(한/영)를 입력하고 Enter. 우선순위:

1. `ANTHROPIC_API_KEY`가 있으면 Anthropic API (BYOK)
2. 없으면 로컬 [Ollama](https://ollama.com) (`TERMDEV_OLLAMA_URL`, 기본 `http://localhost:11434`)

생성된 명령은 **실행되지 않고** 프롬프트에 삽입만 됩니다. 실행 여부는 사용자가 결정.

## 아키텍처

```
GUI(클라이언트)          mux 데몬(별도 프로세스)
  Term + 렌더러   ◀──소켓──▶  PTY + 셸 (창을 닫아도 생존)
  선택/스크롤/블록/AI        리플레이 버퍼
```

셸/PTY는 데몬이 소유하고 GUI는 출력 바이트를 로컬 Term에 먹인다. 덕분에
GUI를 닫아도 세션이 유지되고, 다시 붙으면 리플레이로 복원된다.

## 배포 (패키징)

```sh
./scripts/make-icon.sh          # dist/AppIcon.icns 생성
./scripts/bundle.sh             # dist/terminal-dev.app 번들 생성
open dist/terminal-dev.app
```

정식 배포(Homebrew Cask)는 코드 서명 + 공증(notarization)이 필요합니다.
Cask 템플릿은 [`Casks/terminal-dev.rb`](Casks/terminal-dev.rb) 참고 —
GitHub 릴리스에 `.app.zip`을 올린 뒤 `version`/`sha256`/`url`을 채웁니다.

## 라이선스

MIT
