//! winit 애플리케이션: 창/탭/페인 상태와 이벤트 루프 배선.
//!
//! 입력 처리와 각 오버레이는 하위 모듈로 나뉜다.
//! - [`input`] 키보드·IME → PTY 바이트
//! - [`mouse`] 클릭·드래그 선택·휠·하이퍼링크
//! - [`clipboard`] 복사/붙여넣기
//! - [`palette`] 커맨드 팔레트 (Cmd+Shift+P)
//! - [`search`] 스크롤백 검색 (Cmd+F)
//! - [`ai_bar`] AI 명령 생성 바 (Cmd+K)
//! - [`quake`] Ctrl+` 전역 드롭다운
//! - [`status`] 하단 상태바 문자열

mod ai_bar;
mod clipboard;
mod input;
mod mouse;
mod mouse_report;
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
        let mut out = Vec::new();
        self.active_tab().root.layout(self.content_rect(), &mut out);
        out
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

    /// 탭의 모든 페인 세션 크기를 현재 배치에 맞춘다.
    fn relayout_tab(&self, tab_index: usize) {
        let mut rects = Vec::new();
        self.tabs[tab_index]
            .root
            .layout(self.content_rect(), &mut rects);
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
    /// Quake 전역 핫키 매니저 (살아있어야 핫키가 유지됨)
    _hotkey: Option<global_hotkey::GlobalHotKeyManager>,
    /// Quake 드롭다운으로 숨겨진 상태인지
    quake_hidden: bool,
    /// 상태바용 시스템 지표 (틱마다 갱신)
    sys: sysinfo::System,

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

    // IME 조합 중 문자열 (포커스된 페인에 적용)
    preedit: Option<String>,
}

impl App {
    pub fn new(proxy: EventLoopProxy<AppEvent>) -> Self {
        Self {
            proxy,
            state: None,
            modifiers: Modifiers::default(),
            clipboard: None,
            next_pane_id: 0,
            config: config::Config::load(),
            ai: AiState::Idle,
            ai_seq: 0,
            palette: None,
            search: None,
            _hotkey: None,
            quake_hidden: false,
            sys: sysinfo::System::new(),
            mouse_pos: PhysicalPosition::new(0.0, 0.0),
            left_button_down: false,
            last_click_at: None,
            last_click_point: None,
            click_count: 0,
            scroll_accum: 0.0,
            held_button: None,
            last_report_cell: None,
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
        );
        Pane {
            id,
            session,
            title: "zsh".to_string(),
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
        });
        state.active = state.tabs.len() - 1;
        self.preedit = None;
        state.window.request_redraw();
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
        ) else {
            return; // 세션이 이미 사라졌으면 건너뛴다
        };
        self.next_pane_id += 1;
        let pane = Pane {
            id,
            session,
            title: "zsh".to_string(),
        };
        let state = self.state.as_mut().unwrap();
        state.tabs.push(Tab {
            root: PaneNode::Leaf(pane),
            focused: id,
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
            state.tabs[active].focused = new_id;
            state.relayout_tab(active);
            self.preedit = None;
            state.window.request_redraw();
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
        if kill {
            if let Some(pane) = state.tabs[tab_index].root.pane(pane_id) {
                pane.session.kill();
            }
        }

        let is_last_pane = state.tabs[tab_index].root.panes().len() == 1;
        if is_last_pane {
            state.tabs.remove(tab_index);
            if state.tabs.is_empty() {
                event_loop.exit();
                return;
            }
            if state.active >= state.tabs.len() {
                state.active = state.tabs.len() - 1;
            }
        } else {
            state.tabs[tab_index].root.remove(pane_id);
            if state.tabs[tab_index].focused == pane_id {
                state.tabs[tab_index].focused = state.tabs[tab_index].root.first_id().unwrap_or(0);
            }
            state.relayout_tab(tab_index);
        }
        self.preedit = None;
        state.window.request_redraw();
    }

    fn switch_tab(&mut self, index: usize) {
        let Some(state) = &mut self.state else { return };
        if index < state.tabs.len() && index != state.active {
            state.active = index;
            self.preedit = None;
            let title = state.focused_pane().title.clone();
            state.window.set_title(&title);
            state.window.request_redraw();
        }
    }

    /// 방향키로 포커스를 인접 페인으로 옮긴다.
    fn move_focus(&mut self, dx: f32, dy: f32) {
        let Some(state) = &mut self.state else { return };
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
        let tab = &tabs[*active];
        let focused = tab.focused;
        let mut rects = Vec::new();
        tab.root.layout(content, &mut rects);

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

        let titles: Vec<String> = tabs
            .iter()
            .map(|t| {
                t.root
                    .pane(t.focused)
                    .map(|p| p.title.clone())
                    .unwrap_or_else(|| "zsh".to_string())
            })
            .collect();

        let palette_arg = palette_items
            .as_ref()
            .map(|(query, names, selected)| (query.as_str(), names.as_slice(), *selected));

        let ime_pos = renderer.draw(renderer::DrawParams {
            panes: &views,
            preedit: pane_preedit.as_deref(),
            tab_titles: &titles,
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
            .with_title("terminal")
            .with_inner_size(LogicalSize::new(960.0, 640.0))
            // 배경 불투명도 < 1.0이면 창을 투명 모드로 (iTerm2 Transparency)
            .with_transparent(self.config.background_opacity < 1.0);
        let window = Arc::new(event_loop.create_window(attrs).expect("창 생성 실패"));
        window.set_ime_allowed(true);

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
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers,
            WindowEvent::Resized(size) => {
                let state = self.state.as_mut().unwrap();
                state.renderer.resize(size.width, size.height);
                for i in 0..state.tabs.len() {
                    state.relayout_tab(i);
                }
                state.window.request_redraw();
            }
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
    /// 데몬에 살아있는 세션이 있으면 탭으로 복원하고, 없으면 새 탭을 연다.
    fn restore_or_create_tabs(&mut self) {
        let _ = mux::ensure_daemon();
        let surviving = session::Session::list();
        if surviving.is_empty() {
            self.new_tab();
            return;
        }
        for id in surviving {
            self.attach_tab(id);
        }
        if self.state.as_ref().unwrap().tabs.is_empty() {
            // 모든 세션이 attach 직전에 사라졌으면 새로 만든다
            self.new_tab();
        } else {
            self.state.as_mut().unwrap().active = 0;
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
