//! Phase 1: winit 창 + wgpu 렌더러 + 키보드 입력 → PTY.
//!
//! `셸 → PTY → 파서 → 그리드`(Phase 0) 위에 `그리드 → GPU 렌더링`과
//! `키 입력 → PTY`를 연결해 눈으로 보고 타이핑할 수 있는 터미널을 만든다.

mod renderer;
mod session;

use std::sync::Arc;

use alacritty_terminal::event::{Event as TermEvent, WindowSize};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, Modifiers, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

struct State {
    window: Arc<Window>,
    renderer: renderer::Renderer,
    session: session::Session,
}

struct App {
    proxy: EventLoopProxy<TermEvent>,
    state: Option<State>,
    modifiers: Modifiers,
}

impl App {
    fn window_size(renderer: &renderer::Renderer, width: u32, height: u32) -> WindowSize {
        let (cols, lines) = renderer.grid_size(width, height);
        WindowSize {
            num_cols: cols as u16,
            num_lines: lines as u16,
            cell_width: renderer.cell_width as u16,
            cell_height: renderer.cell_height as u16,
        }
    }
}

impl ApplicationHandler<TermEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("terminal")
            .with_inner_size(LogicalSize::new(960.0, 640.0));
        let window = Arc::new(event_loop.create_window(attrs).expect("창 생성 실패"));

        let renderer = renderer::Renderer::new(Arc::clone(&window));
        let size = window.inner_size();
        let ws = Self::window_size(&renderer, size.width, size.height);
        let session = session::Session::new(session::EventProxy::new(self.proxy.clone()), ws);

        self.state = Some(State {
            window,
            renderer,
            session,
        });
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: TermEvent) {
        let Some(state) = &mut self.state else {
            return;
        };
        match event {
            TermEvent::Wakeup => state.window.request_redraw(),
            // 터미널이 앱의 질의(커서 위치 등)에 응답할 때 — 반드시 PTY로 되돌려준다.
            TermEvent::PtyWrite(text) => state.session.write(text.into_bytes()),
            TermEvent::Title(title) => state.window.set_title(&title),
            TermEvent::Exit => event_loop.exit(),
            _ => {}
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(state) = &mut self.state else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers,
            WindowEvent::Resized(size) => {
                state.renderer.resize(size.width, size.height);
                let ws = Self::window_size(&state.renderer, size.width, size.height);
                state.session.resize(ws);
                state.window.request_redraw();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    if let Some(bytes) = key_to_bytes(&event, self.modifiers.state()) {
                        state.session.write(bytes);
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                state.renderer.draw(&state.session.term);
            }
            _ => {}
        }
    }
}

/// 키 입력을 PTY로 보낼 바이트 시퀀스로 변환한다.
fn key_to_bytes(event: &KeyEvent, mods: ModifiersState) -> Option<Vec<u8>> {
    // Cmd 조합은 앱 단축키 영역으로 남겨둔다.
    if mods.super_key() {
        return None;
    }

    if let Key::Named(named) = &event.logical_key {
        let seq: Option<&[u8]> = match named {
            NamedKey::Enter => Some(b"\r"),
            NamedKey::Backspace => Some(b"\x7f"),
            NamedKey::Tab => Some(b"\t"),
            NamedKey::Escape => Some(b"\x1b"),
            NamedKey::ArrowUp => Some(b"\x1b[A"),
            NamedKey::ArrowDown => Some(b"\x1b[B"),
            NamedKey::ArrowRight => Some(b"\x1b[C"),
            NamedKey::ArrowLeft => Some(b"\x1b[D"),
            NamedKey::Home => Some(b"\x1b[H"),
            NamedKey::End => Some(b"\x1b[F"),
            NamedKey::PageUp => Some(b"\x1b[5~"),
            NamedKey::PageDown => Some(b"\x1b[6~"),
            NamedKey::Delete => Some(b"\x1b[3~"),
            NamedKey::Space => {
                if mods.control_key() {
                    return Some(vec![0]);
                }
                Some(b" ")
            }
            _ => None,
        };
        if let Some(seq) = seq {
            return Some(seq.to_vec());
        }
    }

    // Ctrl+A..Z → C0 제어 문자
    if mods.control_key() {
        if let Key::Character(s) = &event.logical_key {
            let c = s.chars().next()?.to_ascii_lowercase();
            if c.is_ascii_lowercase() {
                return Some(vec![c as u8 - b'a' + 1]);
            }
        }
    }

    event
        .text
        .as_ref()
        .filter(|t| !t.is_empty())
        .map(|t| t.as_bytes().to_vec())
}

fn main() {
    let event_loop = EventLoop::<TermEvent>::with_user_event()
        .build()
        .expect("이벤트 루프 생성 실패");
    let proxy = event_loop.create_proxy();
    let mut app = App {
        proxy,
        state: None,
        modifiers: Modifiers::default(),
    };
    event_loop.run_app(&mut app).expect("이벤트 루프 실행 실패");
}
