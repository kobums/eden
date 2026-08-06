//! macOS 네이티브 터미널 — 진입점.
//!
//! GUI 프로세스는 창·렌더링·터미널 상태를 담당하고, 셸/PTY는 `--daemon`으로
//! 뜨는 mux 데몬이 소유한다 (창을 닫아도 세션이 살아남는 구조).
//! 자세한 구조는 `docs/architecture.md` 참고.

// 그리기 프리미티브와 좌표 변환은 위치·크기·색처럼 함께 다니는 스칼라를
// 여러 개 받는다 (`draw_text`, `draw_block_gutter`, `cell_at`). 린트를 맞추려고
// 구조체로 묶으면 호출부가 더 장황해지므로 그대로 둔다.
#![allow(clippy::too_many_arguments)]

mod ai;
mod app;
mod config;
mod layout;
mod mux;
mod renderer;
mod session;

use winit::event_loop::EventLoop;

use session::AppEvent;

/// Dock 아이콘을 런타임에 지정한다.
///
/// 번들(.app)은 Info.plist의 AppIcon.icns로 아이콘이 잡히지만,
/// `cargo run` 같은 맨 바이너리 실행은 번들 메타데이터가 없어
/// 제네릭 실행파일 아이콘이 뜬다. 둘이 같아 보이도록 여기서 덮어쓴다.
fn set_dock_icon() {
    use objc2::ClassType;
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::{MainThreadMarker, NSData};

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let data = NSData::with_bytes(include_bytes!("../assets/icon.png"));
    if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
        unsafe {
            NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&image));
        }
    }
}

fn main() {
    // `--daemon`: mux 데몬으로 실행 (세션/PTY 소유, GUI와 독립적으로 생존)
    if std::env::args().any(|a| a == "--daemon") {
        mux::run_daemon();
    }

    let event_loop = EventLoop::<AppEvent>::with_user_event()
        .build()
        .expect("이벤트 루프 생성 실패");
    set_dock_icon();
    let mut app = app::App::new(event_loop.create_proxy());
    event_loop.run_app(&mut app).expect("이벤트 루프 실행 실패");
}
