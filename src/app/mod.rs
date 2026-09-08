//! winit 애플리케이션: 창/탭/페인 상태와 이벤트 루프 배선.
//!
//! 입력 처리와 각 오버레이는 하위 모듈로 나뉜다.
//! - [`input`] 키보드·IME → PTY 바이트
//! - [`mouse`] 클릭·드래그 선택·휠·하이퍼링크
//! - [`links`] 평문 URL 자동 감지 (밑줄·Cmd+클릭)
//! - [`clipboard`] 복사/붙여넣기
//! - [`palette`] 커맨드 팔레트 (Cmd+Shift+P)
//! - [`search`] 스크롤백 검색 (Cmd+F)
//! - [`ai_bar`] AI 명령 생성 바 (Cmd+K)
//! - [`quake`] Ctrl+` 전역 드롭다운
//! - [`status`] 하단 상태바 문자열
//! - [`layout_persist`] 탭·페인 구조를 layout.json에 저장/복원
//! - [`action`] 앱 액션 + 키맵 (키바인딩과 팔레트가 공유)

mod action;
mod ai_bar;
mod clipboard;
mod input;
mod kitty_key;
mod layout_persist;
// 렌더러가 밑줄을 그릴 때도 쓰므로 crate에 공개한다 (search와 같은 이유).
pub(crate) mod links;
mod mouse;
mod mouse_report;
mod notify;
mod palette;
mod quake;
pub(crate) mod search;
mod status;

use std::sync::Arc;
use std::time::Instant;

use alacritty_terminal::event::{Event as TermEvent, WindowSize};
use alacritty_terminal::index::Point;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{Modifiers, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

use crate::layout::{Pane, PaneNode, Rect, SplitDir};
use crate::session::AppEvent;
use crate::{config, mux, renderer, session};

use ai_bar::AiState;
use palette::Palette;

struct Tab {
    root: PaneNode,
    /// 포커스된 페인 ID
    focused: usize,
    /// 줌 상태 — 포커스된 페인이 탭 전체를 차지한다 (tmux의 `prefix+z`).
    ///
    /// 분할 트리는 건드리지 않고 배치 단계에서만 무시한다. 줌을 풀면
    /// 원래 구조가 그대로 돌아온다.
    zoomed: bool,
}

impl Tab {
    /// 이 탭의 페인 배치.
    ///
    /// 배치를 계산하는 곳이 셋(히트 테스트·PTY 리사이즈·렌더)이라 여기로 모았다.
    /// 줌 같은 규칙을 한 곳에만 넣으면 되고, 세 곳이 어긋날 일도 없다.
    fn layout(&self, content: Rect) -> Vec<(usize, Rect)> {
        self.root.layout_zoomed(content, self.focused, self.zoomed)
    }

    /// 페인이 둘 이상일 때만 줌이 의미가 있다.
    fn can_zoom(&self) -> bool {
        self.root.panes().len() > 1
    }
}

struct State {
    window: Arc<Window>,
    renderer: renderer::Renderer,
    tabs: Vec<Tab>,
    active: usize,
}

/// 탭 바와 상태바 사이의 콘텐츠 영역.
///
/// `State`를 통째로 빌리지 않고 필드만 받는다 — `redraw`처럼 renderer를 가변으로
/// 빌린 채 배치를 계산해야 하는 곳에서 쓰기 위해서다.
fn content_rect(window: &Window, renderer: &renderer::Renderer) -> Rect {
    let size = window.inner_size();
    let bar_h = renderer.tab_bar_height();
    let status_h = renderer.status_bar_height();
    Rect {
        x: 0.0,
        y: bar_h,
        w: size.width as f32,
        h: (size.height as f32 - bar_h - status_h).max(1.0),
    }
}

impl State {
    fn active_tab(&self) -> &Tab {
        &self.tabs[self.active]
    }

    fn focused_pane(&self) -> &Pane {
        let tab = self.active_tab();
        tab.root.pane(tab.focused).expect("포커스된 페인 없음")
    }

    fn content_rect(&self) -> Rect {
        content_rect(&self.window, &self.renderer)
    }

    /// 활성 탭의 페인 배치를 계산한다.
    fn pane_rects(&self) -> Vec<(usize, Rect)> {
        self.active_tab().layout(self.content_rect())
    }

    /// 페인 사각형에 대응하는 PTY 크기.
    fn window_size(&self, rect: Rect) -> WindowSize {
        let (cols, lines) = self.renderer.pane_grid_size(rect);
        WindowSize {
            num_cols: cols as u16,
            num_lines: lines as u16,
            cell_width: self.renderer.cell_width as u16,
            cell_height: self.renderer.cell_height as u16,
        }
    }

    /// 탭의 보이는 페인 세션 크기를 현재 배치에 맞춘다.
    ///
    /// 줌 중에는 가려진 페인의 PTY 크기가 낡은 채로 남지만, 줌을 풀 때
    /// 다시 호출하므로 화면에 나올 시점에는 항상 맞는다.
    fn relayout_tab(&self, tab_index: usize) {
        let rects = self.tabs[tab_index].layout(self.content_rect());
        for (id, rect) in rects {
            if let Some(pane) = self.tabs[tab_index].root.pane(id) {
                pane.session.resize(self.window_size(rect));
            }
        }
    }
}

pub struct App {
    proxy: EventLoopProxy<AppEvent>,
    state: Option<State>,
    modifiers: Modifiers,
    clipboard: Option<arboard::Clipboard>,
    next_pane_id: usize,
    config: config::Config,
    ai: AiState,
    /// 취소된 요청의 늦은 응답을 무시하기 위한 시퀀스 번호
    ai_seq: u64,
    palette: Option<Palette>,
    /// 스크롤백 검색 상태 (Cmd+F). None이면 닫힌 것.
    search: Option<search::Search>,
    /// Cmd 조합 → 액션 조회 표 (기본표 + 설정의 `keybind` 재정의).
    keymap: action::Keymap,
    /// Quake 전역 핫키 매니저 (살아있어야 핫키가 유지됨)
    _hotkey: Option<global_hotkey::GlobalHotKeyManager>,
    /// Quake 드롭다운으로 숨겨진 상태인지
    quake_hidden: bool,
    /// 창 포커스 여부 (WindowEvent::Focused). 명령 완료·OSC 9/777 알림의
    /// "보고 있지 않을 때만 알림" 조건에 쓰인다.
    window_focused: bool,
    /// 상태바용 시스템 지표 (틱마다 갱신)
    sys: sysinfo::System,
    /// 데몬 boot id (시작 시각). layout.json에 함께 저장해, 데몬이 재시작해
    /// 세션 ID가 재발급됐을 때 옛 파일이 오매칭되는 것을 막는다.
    /// 구버전 데몬(boot id 미지원)이면 None.
    daemon_boot: Option<u64>,

    // 마우스 상태
    mouse_pos: PhysicalPosition<f64>,
    left_button_down: bool,
    last_click_at: Option<Instant>,
    last_click_point: Option<Point>,
    click_count: u32,
    scroll_accum: f32,
    /// 마우스 리포팅 중 눌려 있는 버튼 (드래그 리포트 1002의 조건).
    held_button: Option<winit::event::MouseButton>,
    /// 마지막으로 리포트한 셀. 같은 셀 안의 픽셀 이동은 보내지 않는다 —
    /// 1003 모드에서 중복 제거 없이 보내면 mux 소켓이 포화된다.
    last_report_cell: Option<(usize, usize)>,
    /// 드래그 중인 구분선의 경로. None이면 드래그 중이 아니다.
    dragging_divider: Option<crate::layout::SplitPath>,

    // IME 조합 중 문자열 (포커스된 페인에 적용)
    preedit: Option<String>,
}

impl App {
    pub fn new(proxy: EventLoopProxy<AppEvent>) -> Self {
        // 키맵이 설정의 `keybind` 줄을 봐야 하므로 먼저 로드한다.
        let config = config::Config::load();
        let keymap = action::Keymap::from_config(&config.keybinds);
        Self {
            proxy,
            state: None,
            modifiers: Modifiers::default(),
            clipboard: None,
            next_pane_id: 0,
            config,
            ai: AiState::Idle,
            ai_seq: 0,
            palette: None,
            search: None,
            keymap,
            _hotkey: None,
            quake_hidden: false,
            // 창 생성 직후 Focused(true)가 오지만, 그 전에 attach 리플레이
            // 이벤트가 먼저 도착해도 알림이 새지 않도록 포커스로 시작한다.
            window_focused: true,
            sys: sysinfo::System::new(),
            daemon_boot: None,
            mouse_pos: PhysicalPosition::new(0.0, 0.0),
            left_button_down: false,
            last_click_at: None,
            last_click_point: None,
            click_count: 0,
            scroll_accum: 0.0,
            held_button: None,
            last_report_cell: None,
            dragging_divider: None,
            preedit: None,
        }
    }

    fn make_pane(&mut self, rect: Rect) -> Pane {
        let state = self.state.as_ref().unwrap();
        let id = self.next_pane_id;
        self.next_pane_id += 1;
        let ws = state.window_size(rect);
        let session = session::Session::new(
            session::EventProxy::new(self.proxy.clone(), id),
            ws,
            self.config.scrollback,
            self.config.kitty_keyboard,
        );
        Pane {
            id,
            session,
            title: "zsh".to_string(),
            unseen_exit: None,
        }
    }

    fn new_tab(&mut self) {
        let Some(state) = &self.state else { return };
        let rect = state.content_rect();
        let pane = self.make_pane(rect);
        let id = pane.id;
        let state = self.state.as_mut().unwrap();
        state.tabs.push(Tab {
            root: PaneNode::Leaf(pane),
            focused: id,
            zoomed: false,
        });
        state.active = state.tabs.len() - 1;
        self.preedit = None;
        state.window.request_redraw();
        self.save_layout();
    }

    /// 데몬의 기존 세션 ID에 붙어 새 탭으로 복원한다.
    fn attach_tab(&mut self, session_id: u64) {
        let Some(state) = &self.state else { return };
        let id = self.next_pane_id;
        let ws = state.window_size(state.content_rect());
        let Some(session) = session::Session::attach(
            session::EventProxy::new(self.proxy.clone(), id),
            session_id,
            ws,
            self.config.scrollback,
            self.config.kitty_keyboard,
        ) else {
            return; // 세션이 이미 사라졌으면 건너뛴다
        };
        self.next_pane_id += 1;
        let pane = Pane {
            id,
            session,
            title: "zsh".to_string(),
            unseen_exit: None,
        };
        let state = self.state.as_mut().unwrap();
        state.tabs.push(Tab {
            root: PaneNode::Leaf(pane),
            focused: id,
            zoomed: false,
        });
        state.window.request_redraw();
    }

    /// 포커스된 페인을 분할한다.
    fn split_pane(&mut self, dir: SplitDir) {
        let Some(state) = &self.state else { return };
        let target = state.active_tab().focused;
        let rect = state.content_rect();
        let pane = self.make_pane(rect); // 크기는 relayout에서 재조정
        let new_id = pane.id;

        let state = self.state.as_mut().unwrap();
        let active = state.active;
        if state.tabs[active]
            .root
            .split_leaf(target, dir, pane)
            .is_none()
        {
            // 분할했는데 줌이 걸려 있으면 새 페인이 보이지 않는다.
            state.tabs[active].zoomed = false;
            state.tabs[active].focused = new_id;
            state.relayout_tab(active);
            self.preedit = None;
            state.window.request_redraw();
            self.save_layout();
        }
    }

    /// 페인을 닫는다. 탭의 마지막 페인이면 탭을 닫고, 마지막 탭이면 종료한다.
    /// `kill`이 true면 데몬 세션(셸)도 종료한다 (Cmd+W). false면 로컬 정리만
    /// (셸이 이미 exit한 경우 — TermEvent::Exit).
    fn close_pane(&mut self, pane_id: usize, kill: bool, event_loop: &ActiveEventLoop) {
        let Some(state) = &mut self.state else { return };
        let Some(tab_index) = state
            .tabs
            .iter()
            .position(|t| t.root.pane(pane_id).is_some())
        else {
            return;
        };

        // 명시적 닫기: 데몬 세션 종료 (다음 실행에서 복원되지 않도록)
        if kill && let Some(pane) = state.tabs[tab_index].root.pane(pane_id) {
            pane.session.kill();
        }

        let is_last_pane = state.tabs[tab_index].root.panes().len() == 1;
        if is_last_pane {
            state.tabs.remove(tab_index);
            if state.tabs.is_empty() {
                // 마지막 탭까지 닫고 종료하는 경우에도 빈 구조를 저장한다 —
                // 다음 실행이 죽은 세션이 든 옛 파일을 읽지 않게.
                self.save_layout();
                event_loop.exit();
                return;
            }
            if state.active >= state.tabs.len() {
                state.active = state.tabs.len() - 1;
            }
        } else {
            state.tabs[tab_index].root.remove(pane_id);
            // 줌을 유지하면 포커스가 옮겨간 다른 페인이 말없이 확대된 상태가 된다.
            state.tabs[tab_index].zoomed = false;
            if state.tabs[tab_index].focused == pane_id {
                state.tabs[tab_index].focused = state.tabs[tab_index].root.first_id().unwrap_or(0);
            }
            state.relayout_tab(tab_index);
        }
        self.preedit = None;
        state.window.request_redraw();
        self.save_layout();
    }

    fn switch_tab(&mut self, index: usize) {
        let Some(state) = &mut self.state else { return };
        if index < state.tabs.len() && index != state.active {
            state.active = index;
            self.preedit = None;
            let title = state.focused_pane().title.clone();
            state.window.set_title(&title);
            state.window.request_redraw();
            // 활성 탭도 복원 대상이다 (재시작 시 같은 탭이 앞에 오도록).
            self.save_layout();
        }
    }

    /// 포커스된 페인을 탭 전체로 확대/복원한다 (tmux의 `prefix+z`).
    ///
    /// 분할 트리는 그대로 두고 배치에서만 무시하므로, 풀면 원래 구조가
    /// 정확히 돌아온다. 페인이 하나뿐이면 할 일이 없다.
    fn toggle_zoom(&mut self) {
        let Some(state) = &mut self.state else { return };
        let active = state.active;
        if !state.tabs[active].zoomed && !state.tabs[active].can_zoom() {
            return;
        }
        state.tabs[active].zoomed = !state.tabs[active].zoomed;
        // 보이게 된 페인들의 PTY 크기를 새 배치에 맞춘다.
        state.relayout_tab(active);
        self.preedit = None;
        state.window.request_redraw();
    }

    /// 방향키로 포커스를 인접 페인으로 옮긴다.
    fn move_focus(&mut self, dx: f32, dy: f32) {
        let Some(state) = &mut self.state else { return };
        // 줌 중에는 배치에 페인이 하나뿐이라 옮겨갈 곳이 없다. 먼저 푼다.
        let active = state.active;
        if state.tabs[active].zoomed {
            state.tabs[active].zoomed = false;
            state.relayout_tab(active);
        }
        let rects = state.pane_rects();
        let focused = state.active_tab().focused;
        let Some(&(_, from)) = rects.iter().find(|(id, _)| *id == focused) else {
            return;
        };
        let (fx, fy) = from.center();

        // 이동 방향의 반평면에 있는 페인 중 가장 가까운 것.
        let distance = |r: &Rect| {
            let (cx, cy) = r.center();
            (cx - fx).powi(2) + (cy - fy).powi(2)
        };
        let target = rects
            .iter()
            .filter(|(id, _)| *id != focused)
            .filter(|(_, r)| {
                let (cx, cy) = r.center();
                (cx - fx) * dx + (cy - fy) * dy > 0.0
            })
            .min_by(|(_, a), (_, b)| distance(a).total_cmp(&distance(b)))
            .map(|(id, _)| *id);

        if let Some(id) = target {
            let active = state.active;
            state.tabs[active].focused = id;
            self.preedit = None;
            state.window.request_redraw();
        }
    }

    /// macOS 라이트/다크 전환. 설정 파일을 새 외양 기준으로 다시 읽어
    /// (`theme-light`/`theme-dark` 줄이 갈린다) 테마를 즉시 갈아탄다.
    /// 두 키가 없는 설정이면 같은 값이 다시 읽힐 뿐이라 무해하다.
    fn on_theme_changed(&mut self, theme: winit::window::Theme) {
        let appearance = match theme {
            winit::window::Theme::Light => config::Appearance::Light,
            _ => config::Appearance::Dark,
        };
        self.config = config::Config::load_for(appearance);
        self.keymap = action::Keymap::from_config(&self.config.keybinds);
        let state = self.state.as_mut().unwrap();
        state.renderer.set_theme(&self.config);
        state.window.request_redraw();
    }

    /// 폰트 크기를 `delta`(pt)만큼 조절한다 (Cmd+= / Cmd+-).
    fn adjust_font_size(&mut self, delta: f32) {
        let Some(state) = &self.state else { return };
        let pt = state.renderer.font_size() + delta;
        self.set_font_size(pt);
    }

    /// 폰트 크기를 설정 파일 값으로 되돌린다 (Cmd+0).
    fn reset_font_size(&mut self) {
        self.set_font_size(self.config.font_size);
    }

    /// 논리 폰트 크기를 설정한다. 설정 파일과 같은 범위(6~72pt)로 제한하고,
    /// 셀 크기가 바뀌므로 모든 탭의 PTY 크기를 새 배치에 맞춘다.
    fn set_font_size(&mut self, pt: f32) {
        let pt = pt.clamp(6.0, 72.0);
        let Some(state) = self.state.as_mut() else {
            return;
        };
        if (state.renderer.font_size() - pt).abs() < f32::EPSILON {
            return;
        }
        state.renderer.set_font_size(pt);
        for i in 0..state.tabs.len() {
            state.relayout_tab(i);
        }
        state.window.request_redraw();
    }

    fn redraw(&mut self) {
        if self.state.is_none() {
            return;
        }
        // 포커스가 검색 대상 페인을 떠났으면 검색을 닫는다.
        self.close_search_if_pane_changed();
        // self.state를 가변 차용하기 전에 상태바 문자열을 먼저 만든다.
        let status = self.status_strings();
        // AI 바 / 팔레트 오버레이 문자열도 마찬가지.
        let ai_line = self.ai_bar_line();
        // AI 바나 검색 바가 열려 있으면 preedit은 그 바에서 렌더링하므로
        // 페인에는 넘기지 않는다.
        let pane_preedit = if matches!(self.ai, AiState::Idle) && self.search.is_none() {
            self.preedit.clone()
        } else {
            None
        };
        let search_bar = self.search.as_ref().map(|s| {
            let preedit = self.preedit.as_deref().unwrap_or("");
            (format!("Find> {}{}_", s.query, preedit), s.status())
        });
        // 매치는 복제한다 — self.search와 self.state를 동시에 가변 차용할 수 없다.
        // 상한이 1000이라 프레임당 복제 비용은 무시할 만하다.
        let search_matches: Vec<search::AbsMatch> = self
            .search
            .as_ref()
            .map(|s| s.matches().to_vec())
            .unwrap_or_default();
        let search_pane = self.search.as_ref().map(|s| s.pane_id);
        let search_current = self.search.as_ref().and_then(|s| s.current_index());
        let palette_items: Option<(String, Vec<String>, usize)> = self.palette.as_ref().map(|p| {
            let names = Self::palette_matches(&p.query)
                .iter()
                .map(|(name, _)| name.to_string())
                .collect();
            (p.query.clone(), names, p.selected)
        });

        let state = self.state.as_mut().unwrap();
        // renderer(가변)와 tabs(불변)를 동시에 빌리기 위해 필드를 분리 차용한다.
        let State {
            window,
            renderer,
            tabs,
            active,
        } = state;
        let content = content_rect(window, renderer);
        // 활성 탭은 지금 보고 있는 것이다 — 미확인 완료 표시를 지운다.
        // "그려졌다 = 봤다"로 두면 탭을 바꾸는 경로가 몇 개든 한 곳에서 끝난다.
        {
            let ids: Vec<usize> = tabs[*active].root.panes().iter().map(|p| p.id).collect();
            for id in ids {
                if let Some(pane) = tabs[*active].root.pane_mut(id) {
                    pane.unseen_exit = None;
                }
            }
        }
        let tab = &tabs[*active];
        let focused = tab.focused;
        let rects = tab.layout(content);

        // 페인별 블록을 미리 수집 (draw 중 marks 잠금을 피하기 위해)
        let block_lists: Vec<_> = rects
            .iter()
            .map(|(id, _)| {
                tab.root
                    .pane(*id)
                    .map(|p| p.session.blocks())
                    .unwrap_or_default()
            })
            .collect();

        let views: Vec<renderer::PaneView> = rects
            .iter()
            .zip(block_lists.iter())
            .filter_map(|((id, rect), blocks)| {
                tab.root.pane(*id).map(|pane| renderer::PaneView {
                    term: &pane.session.term,
                    blocks,
                    rect: *rect,
                    focused: *id == focused,
                    // 하이라이트는 검색 대상 페인에만 그린다.
                    matches: if search_pane == Some(*id) {
                        &search_matches
                    } else {
                        &[]
                    },
                    current_match: if search_pane == Some(*id) {
                        search_current
                    } else {
                        None
                    },
                })
            })
            .collect();

        let labels: Vec<renderer::TabLabel> = tabs
            .iter()
            .map(|t| {
                let title = t
                    .root
                    .pane(t.focused)
                    .map(|p| p.title.clone())
                    .unwrap_or_else(|| "zsh".to_string());
                // 줌 중에는 다른 페인이 사라진 것처럼 보이므로 표시가 필요하다.
                // tmux가 윈도우 플래그에 Z를 붙이는 것과 같은 관례.
                let title = if t.zoomed {
                    format!("{title} [Z]")
                } else {
                    title
                };
                // 실행 중이 미확인 완료보다 우선한다 — "아직 돌고 있다"가 더 급한 정보.
                let panes = t.root.panes();
                let status = if panes.iter().any(|p| p.session.is_running()) {
                    renderer::TabStatus::Running
                } else if let Some(exit) = panes.iter().find_map(|p| p.unseen_exit) {
                    renderer::TabStatus::Done(exit)
                } else {
                    renderer::TabStatus::Idle
                };
                renderer::TabLabel { title, status }
            })
            .collect();

        let palette_arg = palette_items
            .as_ref()
            .map(|(query, names, selected)| (query.as_str(), names.as_slice(), *selected));

        let ime_pos = renderer.draw(renderer::DrawParams {
            panes: &views,
            preedit: pane_preedit.as_deref(),
            tabs: &labels,
            active_tab: *active,
            ai_bar: ai_line.as_deref(),
            search: search_bar
                .as_ref()
                .map(|(line, status)| (line.as_str(), status.as_str())),
            palette: palette_arg,
            status: Some((status.0.as_str(), status.1.as_str())),
        });
        drop(views);

        // IME 후보창을 커서 바로 아래에 배치
        if let Some((x, y)) = ime_pos {
            window.set_ime_cursor_area(
                PhysicalPosition::new(x, y),
                PhysicalSize::new(renderer.cell_width as f64, renderer.cell_height as f64),
            );
        }
    }
}

impl ApplicationHandler<AppEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("eden")
            .with_inner_size(LogicalSize::new(960.0, 640.0))
            // 배경 불투명도 < 1.0이면 창을 투명 모드로 (iTerm2 Transparency)
            .with_transparent(self.config.background_opacity < 1.0);
        let window = Arc::new(event_loop.create_window(attrs).expect("창 생성 실패"));
        window.set_ime_allowed(true);
        crate::set_dock_icon();

        // 시스템이 라이트 외양이면 설정을 라이트 기준으로 다시 읽는다 —
        // App::new 시점에는 창이 없어 외양을 알 수 없었다 (기본은 다크 해석).
        if window.theme() == Some(winit::window::Theme::Light) {
            self.config = config::Config::load_for(config::Appearance::Light);
            self.keymap = action::Keymap::from_config(&self.config.keybinds);
        }

        let renderer = renderer::Renderer::new(Arc::clone(&window), &self.config);
        self.clipboard = arboard::Clipboard::new().ok();
        self.state = Some(State {
            window,
            renderer,
            tabs: Vec::new(),
            active: 0,
        });

        self.restore_or_create_tabs();

        // Quake 드롭다운: Ctrl+` 전역 핫키를 등록한다.
        self.register_quake_hotkey();

        // 상태바(시계·CPU·메모리) 갱신용 1초 틱.
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
                if proxy.send_event(AppEvent::Tick).is_err() {
                    break; // 앱 종료
                }
            }
        });
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, app_event: AppEvent) {
        if self.state.is_none() {
            return;
        }
        match app_event {
            // Quake 전역 핫키: 드롭다운 토글
            AppEvent::QuakeToggle => self.toggle_quake(),
            // 상태바 갱신 틱: 시스템 지표 새로고침 + 재그리기
            AppEvent::Tick => {
                self.sys.refresh_cpu_usage();
                self.sys.refresh_memory();
                self.state.as_ref().unwrap().window.request_redraw();
            }
            AppEvent::AiResult {
                pane_id,
                seq,
                result,
            } => self.on_ai_result(pane_id, seq, result),
            AppEvent::Term(pane_id, event) => self.on_term_event(pane_id, event, event_loop),
            // 명령 완료 / OSC 9·777 알림 — 조건 판단은 여기(메인 스레드)서 한다.
            AppEvent::CommandFinished {
                pane_id,
                duration,
                exit,
            } => self.on_command_finished(pane_id, duration, exit),
            AppEvent::Notify {
                pane_id,
                title,
                body,
            } => self.on_osc_notify(pane_id, title, body),
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        if self.state.is_none() {
            return;
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            // 알림 조건("보고 있지 않을 때만")에 쓰인다. 포커스를 되찾았을 때
            // Dock 바운스를 멈추는 것은 시스템이 알아서 한다.
            WindowEvent::Focused(focused) => {
                self.window_focused = focused;
                // `eden new`로 밖에서 만든 세션은 창을 다시 볼 때 탭으로 붙인다.
                if focused {
                    self.attach_orphans();
                }
            }
            // macOS 라이트/다크 전환 — 설정을 새 외양 기준으로 다시 읽는다.
            WindowEvent::ThemeChanged(theme) => self.on_theme_changed(theme),
            WindowEvent::ModifiersChanged(modifiers) => {
                // Cmd를 누르고 있는 동안 IME를 끈다. 한글 조합(preedit) 중에는
                // winit(macOS)이 keyDown을 IME(interpretKeyEvents)로만 보내고
                // KeyboardInput을 앱에 전달하지 않아 Cmd 단축키가 통째로
                // 삼켜지기 때문. 끄면 조합 중이던 글자는 폐기된다(Ime::Disabled).
                let was_super = self.modifiers.state().super_key();
                self.modifiers = modifiers;
                let is_super = modifiers.state().super_key();
                if was_super != is_super {
                    self.state
                        .as_ref()
                        .unwrap()
                        .window
                        .set_ime_allowed(!is_super);
                }
            }
            WindowEvent::Resized(size) => {
                let state = self.state.as_mut().unwrap();
                state.renderer.resize(size.width, size.height);
                for i in 0..state.tabs.len() {
                    state.relayout_tab(i);
                }
                state.window.request_redraw();
            }
            // 배율이 다른 모니터로 창을 옮기면 폰트 픽셀 크기를 다시 맞춘다.
            // (이 이벤트 뒤에 Resized가 따라오지만, 오지 않는 경로도 있어
            // 여기서도 relayout까지 해 둔다.)
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                let state = self.state.as_mut().unwrap();
                state.renderer.set_scale_factor(scale_factor as f32);
                for i in 0..state.tabs.len() {
                    state.relayout_tab(i);
                }
                state.window.request_redraw();
            }
            // 파일 드롭 → 셸 인용된 경로 붙여넣기 (여러 파일은 이벤트가
            // 파일마다 한 번씩 온다).
            WindowEvent::DroppedFile(path) => self.drop_file(&path),
            WindowEvent::KeyboardInput { event, .. } => self.on_key(event, event_loop),
            WindowEvent::Ime(ime) => self.on_ime(ime),
            WindowEvent::CursorMoved { position, .. } => self.on_cursor_moved(position),
            WindowEvent::MouseInput {
                state: button_state,
                button,
                ..
            } => self.on_mouse_input(button_state, button),
            WindowEvent::MouseWheel { delta, .. } => self.on_mouse_wheel(delta),
            WindowEvent::RedrawRequested => self.redraw(),
            _ => {}
        }
    }
}

impl App {
    /// 현재 탭/페인 구조를 layout.json에 저장한다.
    ///
    /// 구조가 변할 때마다 즉시 호출한다 (split/close/new_tab/탭 전환/드래그
    /// 리사이즈 종료). 종료 훅에 걸지 않는 이유: 크래시·강제 종료에서도
    /// 마지막 구조가 남아야 세션 지속성의 목적에 맞다.
    pub(super) fn save_layout(&self) {
        let Some(state) = &self.state else { return };
        let tabs: Vec<layout_persist::TabLayout> = state
            .tabs
            .iter()
            .filter_map(|t| layout_persist::snapshot(&t.root, t.focused))
            .collect();
        layout_persist::save(&layout_persist::to_json(
            &tabs,
            state.active,
            self.daemon_boot,
        ));
    }

    /// 데몬에 살아있는 세션이 있으면 layout.json의 분할 구조대로 복원하고,
    /// 파일이 없거나 깨졌거나 미래 버전이면 세션당 탭 1개로 폴백한다.
    /// 세션이 하나도 없으면 새 탭을 연다.
    fn restore_or_create_tabs(&mut self) {
        let _ = mux::ensure_daemon();
        let (surviving, boot) = session::Session::list();
        self.daemon_boot = boot;
        if surviving.is_empty() {
            self.new_tab();
            return;
        }

        let alive: std::collections::HashSet<u64> = surviving.iter().copied().collect();
        let plans =
            layout_persist::load().and_then(|v| layout_persist::from_json(&v, &alive, boot));
        match plans {
            Some((tab_plans, active)) => {
                // attach가 실패해 탭이 통째로 사라지면 인덱스가 당겨지므로,
                // 활성 탭은 "그 계획이 실제로 붙은 위치"로 다시 계산한다.
                let mut new_active = 0;
                for (i, plan) in tab_plans.into_iter().enumerate() {
                    let attached = self.attach_tab_plan(plan);
                    if attached && i == active {
                        new_active = self.state.as_ref().unwrap().tabs.len() - 1;
                    }
                }
                if let Some(state) = self.state.as_mut() {
                    state.active = new_active.min(state.tabs.len().saturating_sub(1));
                }
            }
            None => {
                for id in surviving {
                    self.attach_tab(id);
                }
                if let Some(state) = self.state.as_mut() {
                    state.active = 0;
                }
            }
        }

        if self.state.as_ref().unwrap().tabs.is_empty() {
            // 모든 세션이 attach 직전에 사라졌으면 새로 만든다
            self.new_tab();
        }
        // 가지치기·attach 실패가 반영된 실제 구조로 파일을 되쓴다 —
        // 죽은 세션이 파일에 계속 남지 않게.
        self.save_layout();
    }

    /// 데몬에는 살아있지만 어느 탭에도 없는 세션을 단독 탭으로 붙인다.
    ///
    /// GUI 밖에서 만든 세션(`eden new`)이 여기로 들어온다. 창 포커스 때
    /// 부르므로 데몬에 LIST 한 번 묻는 값싼 일이고, 사용자가 창을 볼 때
    /// 새 탭이 이미 있는 것이 자연스럽다. 이미 GUI가 만든 세션은 탭에 있으므로
    /// 건너뛴다.
    fn attach_orphans(&mut self) {
        let Some(state) = &self.state else { return };
        let known: std::collections::HashSet<u64> = state
            .tabs
            .iter()
            .flat_map(|t| t.root.panes())
            .map(|p| p.session.id())
            .collect();
        let (alive, _) = session::Session::list();
        let orphans: Vec<u64> = alive.into_iter().filter(|id| !known.contains(id)).collect();
        if orphans.is_empty() {
            return;
        }
        for id in orphans {
            self.attach_tab(id);
        }
        self.save_layout();
    }

    /// 복원 계획(세션 ID 트리) 하나를 탭으로 붙인다. 탭이 만들어졌으면 true.
    ///
    /// list()와 attach 사이에 세션이 죽을 수 있으므로, attach에 실패한 leaf는
    /// 죽은 것으로 간주하고 가지치기와 같은 접기 규칙을 적용한다.
    fn attach_tab_plan(&mut self, plan: layout_persist::TabLayout) -> bool {
        let Some(state) = &self.state else {
            return false;
        };
        // 각 페인을 **최종 크기로** 붙인다. 전체 크기로 붙였다가 relayout으로
        // 줄이면, 그 사이에 재생된 리플레이가 넓은 폭 기준으로 그려진 뒤 좁은
        // 폭으로 리플로우돼 프롬프트가 두 번 그려진 것처럼 보인다(실측).
        // 세션 ID 트리로도 배치를 계산할 수 있어서(PaneNode가 페이로드
        // 제네릭) attach 전에 각 leaf의 크기를 알 수 있다.
        let content = state.content_rect();
        let mut rects = Vec::new();
        plan.root.layout(content, &mut rects);
        let sizes: Vec<(u64, WindowSize)> = rects
            .iter()
            .map(|&(id, rect)| (id as u64, state.window_size(rect)))
            .collect();
        // 배치에 없는 leaf(있을 수 없지만)는 콘텐츠 전체 크기로 폴백한다.
        let fallback = state.window_size(content);
        let mut attached: Vec<(u64, usize)> = Vec::new();
        let Some(root) = self.build_pane_tree(plan.root, &sizes, fallback, &mut attached) else {
            return false; // 전 leaf attach 실패 → 탭 버림
        };
        let focused = attached
            .iter()
            .find(|(sid, _)| *sid == plan.focused)
            .map(|(_, pane_id)| *pane_id)
            // 포커스 세션이 attach에 실패했으면 첫 leaf로 폴백.
            .or_else(|| root.first_id());
        let Some(focused) = focused else { return false };

        let state = self.state.as_mut().unwrap();
        state.tabs.push(Tab {
            root,
            focused,
            zoomed: false,
        });
        state.relayout_tab(state.tabs.len() - 1);
        state.window.request_redraw();
        true
    }

    /// 세션 ID 트리를 걸으며 leaf마다 attach해 런타임 페인 트리를 만든다.
    ///
    /// `sizes`는 세션 ID별 최종 페인 크기다 — 리플레이가 처음부터 맞는 폭으로
    /// 그려지도록 attach 시점에 넘긴다. `attached`에 (세션 ID, 페인 ID) 대응을
    /// 쌓는다 (포커스 환산용).
    fn build_pane_tree(
        &mut self,
        node: PaneNode<u64>,
        sizes: &[(u64, WindowSize)],
        fallback: WindowSize,
        attached: &mut Vec<(u64, usize)>,
    ) -> Option<PaneNode<Pane>> {
        match node {
            PaneNode::Empty => None,
            PaneNode::Leaf(session_id) => {
                let id = self.next_pane_id;
                let ws = sizes
                    .iter()
                    .find(|(sid, _)| *sid == session_id)
                    .map(|(_, ws)| *ws)
                    .unwrap_or(fallback);
                let session = session::Session::attach(
                    session::EventProxy::new(self.proxy.clone(), id),
                    session_id,
                    ws,
                    self.config.scrollback,
                    self.config.kitty_keyboard,
                )?;
                self.next_pane_id += 1;
                attached.push((session_id, id));
                Some(PaneNode::Leaf(Pane {
                    id,
                    session,
                    title: "zsh".to_string(),
                    unseen_exit: None,
                }))
            }
            PaneNode::Split {
                dir,
                ratio,
                first,
                second,
            } => {
                let first = self.build_pane_tree(*first, sizes, fallback, attached);
                let second = self.build_pane_tree(*second, sizes, fallback, attached);
                match (first, second) {
                    (Some(a), Some(b)) => Some(PaneNode::Split {
                        dir,
                        ratio,
                        first: Box::new(a),
                        second: Box::new(b),
                    }),
                    // 한쪽이 죽었으면 남은 자식으로 접는다 (remove와 동일 규칙).
                    (Some(a), None) | (None, Some(a)) => Some(a),
                    (None, None) => None,
                }
            }
        }
    }

    /// 세션(PTY)에서 올라온 터미널 이벤트를 처리한다.
    fn on_term_event(&mut self, pane_id: usize, event: TermEvent, event_loop: &ActiveEventLoop) {
        let state = self.state.as_mut().unwrap();
        let Some(tab_index) = state
            .tabs
            .iter()
            .position(|t| t.root.pane(pane_id).is_some())
        else {
            return;
        };
        match event {
            TermEvent::Wakeup => {
                if tab_index == state.active {
                    state.window.request_redraw();
                }
            }
            // 터미널이 앱의 질의(커서 위치 등)에 응답할 때 — 반드시 PTY로 되돌려준다.
            TermEvent::PtyWrite(text) => {
                if let Some(pane) = state.tabs[tab_index].root.pane(pane_id) {
                    pane.session.write(text.into_bytes());
                }
            }
            TermEvent::Title(title) => {
                if let Some(pane) = state.tabs[tab_index].root.pane_mut(pane_id) {
                    pane.title = title.clone();
                }
                if tab_index == state.active && state.tabs[tab_index].focused == pane_id {
                    state.window.set_title(&title);
                }
                state.window.request_redraw();
            }
            // OSC 52: 앱이 클립보드에 쓰기를 요청
            TermEvent::ClipboardStore(_, text) => {
                if let Some(clipboard) = self.clipboard.as_mut() {
                    let _ = clipboard.set_text(text);
                }
            }
            // OSC 52: 앱이 클립보드 읽기를 요청
            TermEvent::ClipboardLoad(_, formatter) => {
                let text = self
                    .clipboard
                    .as_mut()
                    .and_then(|c| c.get_text().ok())
                    .unwrap_or_default();
                if let Some(pane) = state.tabs[tab_index].root.pane(pane_id) {
                    pane.session.write(formatter(&text).into_bytes());
                }
            }
            // 셸 종료 → 해당 페인 닫기
            TermEvent::Exit => self.close_pane(pane_id, false, event_loop),
            _ => {}
        }
    }
}
