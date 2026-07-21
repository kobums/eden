//! Quake 드롭다운: Ctrl+` 전역 핫키로 어느 앱에서든 소환/숨김.

use winit::dpi::PhysicalPosition;

use super::App;
use crate::session::AppEvent;

impl App {
    /// Ctrl+` 전역 핫키를 등록하고, 이벤트를 winit 루프로 포워딩한다.
    pub(super) fn register_quake_hotkey(&mut self) {
        use global_hotkey::hotkey::{Code, HotKey, Modifiers};
        use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

        let Ok(manager) = GlobalHotKeyManager::new() else {
            return; // 핫키 등록 실패 시 Quake 없이 계속
        };
        let hotkey = HotKey::new(Some(Modifiers::CONTROL), Code::Backquote);
        if manager.register(hotkey).is_err() {
            return;
        }
        self._hotkey = Some(manager);

        // OS 콜백이 채우는 채널을 블로킹으로 읽어 winit으로 포워딩한다.
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let receiver = GlobalHotKeyEvent::receiver();
            while let Ok(event) = receiver.recv() {
                if event.state == HotKeyState::Pressed
                    && proxy.send_event(AppEvent::QuakeToggle).is_err()
                {
                    break; // 앱 종료
                }
            }
        });
    }

    /// 토글: 보이면 숨기고, 숨겨졌으면 현재 모니터 상단 중앙에 띄운다.
    pub(super) fn toggle_quake(&mut self) {
        let Some(state) = &self.state else { return };
        if !self.quake_hidden {
            state.window.set_visible(false);
            self.quake_hidden = true;
            return;
        }
        if let Some(monitor) = state.window.current_monitor() {
            let mpos = monitor.position();
            let msize = monitor.size();
            let win = state.window.inner_size();
            let x = mpos.x + (msize.width as i32 - win.width as i32) / 2;
            state
                .window
                .set_outer_position(PhysicalPosition::new(x.max(mpos.x), mpos.y));
        }
        state.window.set_visible(true);
        state.window.focus_window();
        self.quake_hidden = false;
    }
}
