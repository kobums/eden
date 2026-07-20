# 터미널 에뮬레이터 조사 보고서

> 목적: 기존 터미널들의 장점을 모아 새 터미널을 만들기 위한 사전 조사
> 조사일: 2026-07-18 (웹 검색 기반, 병렬 리서치 4트랙 종합)

---

## 1. 한눈에 보는 지형도

터미널 생태계는 크게 4개 진영으로 나뉜다.

| 진영 | 대표 주자 | 무기 | 약점 |
|---|---|---|---|
| 클래식 강자 | iTerm2, Windows Terminal, Konsole | 기능 깊이, OS 통합 | 성능·메모리 |
| 멀티플렉서 | tmux, Zellij | 세션 지속성, 분할, 스크립팅 | 러닝커브, 렌더링 열화 |
| GPU 가속 모던 | Ghostty, Kitty, Alacritty, WezTerm | 속도, 새 프로토콜 | 기능 편차 큼 |
| 차세대/AI | Warp, Wave, Tabby | 블록 UI, AI, 협업 | Electron 무게, 클라우드 종속 |

**핵심 트렌드 3가지:**
1. **멀티플렉서 기능의 내장화** — 로컬 분할/탭은 터미널이 직접 흡수(WezTerm, Kitty, Ghostty)하고, 원격 세션 지속성만 tmux/mux 서버로 해결하는 분업이 진행 중.
2. **프로토콜 표준의 수렴** — OSC 133 셸 통합, Kitty graphics/keyboard protocol, synchronized output(mode 2026), OSC 8 하이퍼링크가 2026년 현재 "현대 터미널 체크리스트"로 굳어짐.
3. **사용자는 본질을 지키는 혁신만 수용** — Warp의 로그인 강제·텔레메트리는 거센 반발 → 정책 철회 → 오픈소스화로 이어졌고, Electron 기반 터미널(Hyper)은 성능 비용을 정당화하지 못해 쇠퇴.

---

## 2. 클래식 터미널

### iTerm2 (macOS) — 기능의 왕
- **킬러 피처**: Split Pane, **tmux 통합(`tmux -CC`)** — tmux 페인/윈도우를 네이티브 패널/탭으로 렌더링해 프리픽스 키 없이 GUI로 tmux 조작 + 세션 지속성. 독보적 기능.
- **Shell Integration**: 프롬프트 인식 → 프롬프트 간 점프, 명령 히스토리, 원격 파일 클릭 다운로드/드래그 업로드.
- **Hotkey Window**(전역 단축키 드롭다운), **트리거**(출력 정규식 매칭 → 하이라이트/알림/스크립트), **Python API**(protobuf+websocket 스크립팅), Instant Replay(화면 되감기), 최대 4M 라인 히스토리.
- **약점**: 성능이 최대 불만 — 메모리 0.5~3GB, TUI 실행 시 CPU 15~20%, 입력 랙. Ghostty 등으로 이탈 증가.
- 스택: Objective-C + Metal 렌더러(있지만 최적화 부족). GPL-2.0 오픈소스, 사실상 1인 메인테이너.

### Terminal.app (macOS) — 빠르지만 미니멀
- 기본 탑재, 낮은 지연시간(실측 상위권), 가벼움. macOS Tahoe(2025)에서 24년 만에 트루컬러·Powerline 지원 추가.
- 약점: split pane·트리거·스크립팅 등 고급 기능 부재, OS 릴리스에 묶인 느린 업데이트. 비공개 소스.

### GNOME Terminal / Konsole (Linux)
- GNOME Terminal: VTE 라이브러리 기반(Tilix 등 수많은 터미널의 공통 엔진). GNOME 46(VTE 0.76)에서 Alacritty급으로 지연시간 개선. 약점: GPU 가속·이미지 프로토콜 부재, GTK4 포팅 지연.
- Konsole: "단점 없는 올라운더". SSH Manager(접속 프로필 원클릭), 이미지/컬러 마우스오버 프리뷰, KPart 아키텍처(Dolphin/Kate/Yakuake에 터미널 컴포넌트로 내장). 약점: KDE 외부 의존성 폭탄.

### Windows Terminal — MS의 재발견
- **WSL 통합**이 최대 강점: PowerShell/CMD/WSL을 탭 하나로. Quake Mode, Command Palette, **AtlasEngine**(DirectX GPU 렌더러, 일반 케이스 4배 고속화), **ConPTY**(Windows에 PTY 개념 도입 — VS Code, Warp 등 모든 서드파티 터미널의 기반 인프라).
- 약점: 느린 시작 속도, 간헐적 심각한 입력 랙. MIT 오픈소스.

---

## 3. 멀티플렉서

### tmux — 사실상의 표준
- **사랑받는 것**: ① 세션 지속성(SSH 끊겨도 detach/attach로 복원, 여러 기기 동시 attach) ② 가벼움(바이너리 ~900KiB, 세션 ~6MB) ③ copy mode(키보드만으로 검색·복사) ④ 완전한 CLI 스크립팅(`send-keys` 등 — 최근엔 AI 에이전트가 세션 조작하는 용도로 각광) ⑤ TPM 플러그인 200+.
- **불만 3대장**: ① 러닝커브(프리픽스 키, 불친절한 기본값) ② 스크롤백 어색함(터미널 스크롤을 가로챔 → copy mode 진입 필요) ③ 클립보드 통합 설정 지옥. 추가로 **중간 계층에서 이스케이프 시퀀스를 재해석해 GPU 렌더링·이미지·최신 기능이 깎이는 구조적 문제**.

### Zellij — "tmux 파워의 80%를 러닝커브 20%로"
- tmux 대비 개선: **항상 보이는 키바인딩 힌트 바**(발견 가능성), **floating/stacked panes**(핀 고정 가능), **KDL 선언적 레이아웃**(프로젝트별 환경 원샷 구성 — tmuxinator 내장판), **WASM 플러그인**(언어 무관, 샌드박스), **세션 부활 내장**(재부팅 후에도 페인 구조·명령 복원, 명령은 Enter 확인 후 실행), 웰컴 스크린.
- 약점: 메모리 ~80MB(tmux의 13배), 기본 Ctrl 키바인딩이 에디터와 충돌.

### 기타
- **screen**: 어디에나 있음, 시리얼 콘솔(`screen /dev/ttyUSB0`), 멀티유저 터미널 공유. 발전은 정체.
- **mosh**: 연결(connectivity) 지속 계층 — UDP 로밍(Wi-Fi→LTE 전환에도 유지), 로컬 에코 예측으로 고지연 회선 대응. tmux와 상호보완(정석 조합: mosh + tmux).

### 왜 최신 터미널이 멀티플렉서를 내장하나
1. tmux 중간 계층의 기능·성능 열화 제거
2. 네이티브 스크롤백·클립보드·마우스로 tmux 3대 불만 원천 해소
3. tmux 가치의 분해: 로컬 분할(터미널이 흡수) + 원격 지속성(WezTerm mux 서버 / iTerm2 tmux -CC)
4. 단, 세션 공유·보편성(서버 어디에나 있음)은 여전히 tmux의 영역

---

## 4. GPU 가속 모던 터미널

| | Alacritty | Kitty | WezTerm | Ghostty |
|---|---|---|---|---|
| 언어 | Rust | C+Python | Rust | Zig(+Swift) |
| 렌더링 | OpenGL | OpenGL | OpenGL/WebGPU | Metal/OpenGL |
| 탭/분할 | 없음(의도적) | 있음 | 있음+내장 mux | 있음(네이티브) |
| 설정 | TOML | 자체 conf | **Lua 스크립트** | key=value |
| 이미지 | ✗ | ◎(원조) | ◎(3종 지원) | ○(kitty) |
| 강점 | 최저 레이턴시·리소스 | 프로토콜 혁신, 273fps | 기능 최강, 원격 mux | 속도+기능+네이티브 |
| 약점 | 기능 없음 | 기본 설정 지연, 메인테이너 논란 | **4종 중 최저 성능** | 신생, 성숙도 |

- **Alacritty**: "빠른 것 하나만". glyph atlas + draw call 2회로 화면 전체 렌더. 레이턴시 6.9ms(Typometer)로 GPU 터미널 중 최저. 탭·분할·ligature·이미지 전부 의도적 거부 — tmux 병용 전제.
- **Kitty**: **프로토콜 혁신의 진원지** — graphics protocol(픽셀 그래픽+애니메이션, 업계 표준화)과 keyboard protocol(모디파이어/press·release 구분)은 모두 Kitty발. kittens(Python 플러그인), 데몬 아키텍처로 2번째 창부터 즉시 오픈. 기본 설정 레이턴시 23.8ms → 튜닝 시 10.7ms.
- **WezTerm**: "터미널+tmux+프로그래밍 언어". **내장 멀티플렉서 + SSH/TLS 도메인 원격 세션**(네트워크 끊겨도 생존), **Lua 설정**(조건 분기·이벤트 핸들러·핫 리로드). 성능은 4종 중 최하위(레이턴시 26.1ms).
- **Ghostty**: Mitchell Hashimoto 작. "빠름·기능·네이티브 중 2개만 고르라는 통념 격파". 코어는 Zig **libghostty**(임베더블 C 라이브러리 — 터미널 인프라화 전략), GUI는 플랫폼 네이티브(Swift+AppKit / GTK4). 콜드 스타트 68ms, 처리량 iTerm2 대비 4배. 설정 없이 기본값으로 완성된 경험.

**성능 요약**: 레이턴시 Alacritty·Ghostty > Kitty(튜닝 시) > WezTerm. 실사용 체감 차이는 대부분 미미하며, 환경(컴포지터·vsync)에 따라 순위가 뒤집힘.

---

## 5. 차세대 / AI 터미널

### Warp — 블록 모델의 원조
- **블록(Blocks)**: 명령+출력을 하나의 단위로 묶어 복사·검색·북마크·공유·점프. 이 개념이 OSC 133 형태로 iTerm2·Kitty·Ghostty·VS Code까지 확산됨.
- **AI**: 컨텍스트(cwd, 히스토리, 종료 코드, 브랜치) 인식 자연어 → 명령 생성, 에러 수정 diff 제안, Agent Mode. BYOK 지원.
- **팀 기능**: Warp Drive(워크플로우 공유), 세션 URL 공유.
- **역사적 교훈**: 로그인 강제+텔레메트리+클로즈드소스 → 대반발 → 단계적 후퇴(2022 텔레메트리 옵트아웃 → 2024 로그인 철폐 → 2026 클라이언트 오픈소스화, AGPL-3.0). 단 AI·클라우드 기능은 여전히 프로프라이어터리 백엔드 의존.

### Wave Terminal — "터미널을 떠나지 않고 본다"
- **인라인 파일 프리뷰**(CSV→테이블, 이미지/PDF/MD — 원격 SSH 파일도 동일), **인라인 웹뷰**(Grafana, localhost 앱을 터미널 옆 블록으로), 위젯 단위 타일링, **모델 중립 AI**(OpenAI/Claude/Ollama 자유 선택, 인접 터미널 블록을 읽음). Apache-2.0, 계정 불필요.
- 약점: Electron — 메모리 400~800MB, 리사이즈 랙.

### Tabby / Hyper / Rio
- **Tabby**: SSH/Telnet/시리얼 연결 관리자 내장 + SFTP + Zmodem. AI가 아니라 **SSH GUI 관리**로 생존(특히 Windows 운영자층). Electron, RAM 300MB+.
- **Hyper**: React+Redux+xterm.js 확장 모델의 선구자였으나 성능 한계와 유지보수 부재로 **사실상 사망** (2년+ 무릴리스, "Is Hyper dead?" 이슈). 교훈: 웹 확장성만으로는 성능 비용을 정당화 못 함.
- **Rio**: Rust + 자체 렌더러 Sugarloaf(wgpu) — 데스크톱과 **브라우저(WASM)에서 동시에 도는** 유일한 시도.

---

## 6. 구현 아키텍처 (우리가 만들 때)

### 기본 구조
```
셸(자식 프로세스) ↔ PTY(master/slave) ↔ VT 파서(상태 기계) ↔ 그리드(셀 버퍼) ↔ 렌더러
```
- **PTY**: OS가 제공. 리사이즈는 `ioctl(TIOCSWINSZ)` + `SIGWINCH`. Windows는 ConPTY.
- **VT 파싱**: 사실상 표준은 "xterm + 현대 확장". Paul Flo Williams의 VT500 상태도가 고전적 기반(vte crate가 구현). CSI(커서/색상)·OSC(제목/링크/셸 통합)·DCS/APC(그래픽) 처리.
- **그리드**: 셀 2차원 배열(코드포인트+스타일+하이퍼링크 ID), 대체 스크린, 스크롤백 링버퍼, 와이드 문자(2셀), reflow. **파싱과 렌더링의 분리가 핵심 설계 원칙.**

### 렌더링
- GPU 가속 + **glyph atlas**: 글리프를 한 번만 래스터라이즈해 텍스처 아틀라스에 캐싱 → 비용이 "화면 문자 수"가 아니라 "고유 글리프 수"에 비례.
- **Damage tracking**: 변경된 셀/라인만 재렌더.
- 폰트 래스터화는 여전히 CPU(FreeType/CoreText/DirectWrite), 결과만 GPU 캐싱.
- 입력 레이턴시는 GPU보다 vsync·컴포지터 파이프라인 설계가 지배적.

### 활용 가능한 라이브러리
| 라이브러리 | 범위 | 비고 |
|---|---|---|
| **alacritty_terminal** (Rust) | 파서+그리드+PTY 상태 엔진 | Zed 에디터가 사용, 실전 검증 |
| **vte** (Rust) | 파서만 | 그리드는 직접 구현 |
| **wezterm-term** (Rust) | 터미널 모델 | 확장 시퀀스 지원 폭 최대 |
| **libghostty-vt** (Zig, C ABI) | 파서+상태 | 제로 의존성, 유망하나 아직 초기 |
| **xterm.js** (TS) | 완결형(파서+그리드+WebGL 렌더) | 웹/Electron이면 사실상 유일한 정답 (VS Code, Hyper, Tabby, Wave) |

### 2026년 "현대 터미널" 필수 체크리스트
1. GPU 렌더링 + glyph atlas + damage tracking
2. **OSC 133** 셸 통합 (블록/프롬프트 점프의 표준 기반 — 1순위)
3. **Kitty keyboard protocol** (CSI u)
4. **Kitty graphics protocol** (+ Sixel 폴백)
5. **Synchronized output** (mode 2026)
6. OSC 8 하이퍼링크, 24bit 트루컬러, bracketed paste(2004), OSC 52 클립보드, undercurl

---

## 7. 종합: 우리 터미널에 가져올 장점 후보

### 각 터미널에서 훔칠 것
| 출처 | 가져올 장점 |
|---|---|
| iTerm2 | tmux -CC식 네이티브 통합 발상, Shell Integration, 트리거, Hotkey Window, Instant Replay |
| tmux | 세션 지속성(detach/attach), CLI 스크립팅 가능성(AI 에이전트 시대에 재부상) |
| Zellij | 키바인딩 힌트 바(발견 가능성), 선언적 레이아웃, floating panes, 안전한 세션 부활 |
| Alacritty | glyph atlas 렌더링, 최저 레이턴시에 대한 집착 |
| Kitty | graphics/keyboard protocol 채택, 데몬 아키텍처(2번째 창 즉시 오픈) |
| WezTerm | 내장 멀티플렉서 + 원격 mux(SSH 도메인), 설정의 프로그래머빌리티 |
| Ghostty | "설정 없이 기본값으로 완성", 네이티브 UI, 코어 라이브러리 분리(libghostty 모델), 68ms 콜드 스타트 |
| Warp | 블록 UI(단, OSC 133 표준 기반으로), 컨텍스트 인식 AI, BYOK |
| Wave | 인라인 파일 프리뷰(원격 포함), 모델 중립 AI |
| Tabby/Konsole | SSH 연결 관리자 GUI |
| Windows Terminal | Quake mode, Command Palette |
| mosh | 로컬 에코 예측, 연결 로밍 (원격 기능 넣을 경우) |

### 피해야 할 함정 (실패 사례에서)
1. **Electron 금지** — Hyper의 사망, Wave/Tabby의 만성 감점 요인. 네이티브(Rust/Zig) + GPU가 기본기.
2. **계정/클라우드 강제 금지** — Warp 반발의 교훈. 로컬 우선, AI는 BYOK/로컬 모델 옵션.
3. **기능을 위해 성능을 팔지 말 것** — WezTerm이 기능 최강이지만 성능 최하위라는 평가에 발목.
4. **기본값 불친절 금지** — tmux의 "입문 의식", Kitty의 튜닝 필요 기본 설정 대신 Ghostty식 "설치 즉시 완성".
5. **독자 규격 남발 금지** — 기존 표준(OSC 133, Kitty protocols)에 올라타는 것이 생태계 편입의 지름길.

### 기술 스택 방향 (검토용 초안)
- **네이티브 코어**: Rust(alacritty_terminal/wezterm-term 활용 가능) 또는 Zig(libghostty 대기). 가장 빠른 출발은 Rust + alacritty_terminal.
- **렌더링**: wgpu(Metal/Vulkan/DX12 커버) 또는 플랫폼 네이티브(Metal 직접).
- **차별화 축 후보**: ① 표준(OSC 133) 기반 블록 UI + 로컬 우선 AI ② 내장 멀티플렉서/원격 세션 ③ 인라인 프리뷰. — 어느 축에 집중할지가 다음 결정 사항.
