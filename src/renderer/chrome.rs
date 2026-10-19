//! UI 크롬: 탭 바, 하단 상태바, AI 입력 바, 커맨드 팔레트 오버레이.

use super::text::FontSize;
use super::{BgInstance, HintBadge, Renderer, TabLabel, TabStatus, TextInstance};

/// AI 입력 바 배경색.
const AI_BAR_BG: [f32; 3] = [0.1, 0.14, 0.24];
/// 검색 바 배경색. AI 바와 같은 자리를 쓰므로 색으로 구분한다.
const SEARCH_BAR_BG: [f32; 3] = [0.16, 0.13, 0.08];
/// 커맨드 팔레트 배경색.
const PALETTE_BG: [f32; 3] = [0.12, 0.13, 0.18];
/// 팔레트에 한 번에 보여줄 최대 항목 수.
const PALETTE_MAX_ROWS: usize = 10;
/// 힌트 라벨 배경 (kitty hints·Vimium처럼 노랑) / 글자색.
const HINT_BG: [f32; 3] = [0.98, 0.84, 0.25];
const HINT_FG: [f32; 3] = [0.08, 0.08, 0.1];
const HINT_TYPED_FG: [f32; 3] = [0.55, 0.47, 0.15];
/// 여러 줄 패널(AI 설명)의 본문 최대 줄 수. 넘치면 뒤쪽(최신)을 보여준다.
const PANEL_MAX_ROWS: usize = 18;

/// 단색 사각형 하나를 넣는 헬퍼 (크롬은 전부 불투명).
fn fill(out: &mut Vec<BgInstance>, rect: [f32; 4], color: [f32; 3]) {
    out.push(BgInstance {
        rect,
        color: [color[0], color[1], color[2], 1.0],
    });
}

impl Renderer {
    /// 탭 바 높이 (물리 픽셀). iTerm2처럼 컴팩트하게 UI 폰트 기준.
    pub fn tab_bar_height(&self) -> f32 {
        (self.ui_line_height * 1.7).ceil()
    }

    /// 하단 상태바 높이 (물리 픽셀). UI 폰트 기준.
    pub fn status_bar_height(&self) -> f32 {
        (self.ui_line_height * 1.6).ceil()
    }

    /// 탭 바 좌표의 클릭이 몇 번째 탭인지 계산한다.
    pub fn tab_hit(&self, x: f64, y: f64, tab_count: usize) -> Option<usize> {
        if y >= self.tab_bar_height() as f64 || tab_count == 0 {
            return None;
        }
        let tab_width = self.config.width as f64 / tab_count as f64;
        let index = (x / tab_width) as usize;
        (index < tab_count).then_some(index)
    }

    /// 탭 바. iTerm2 스타일: 제목만 가운데 정렬, 작은 UI 폰트, 얇은 구분선.
    /// 크롬 색은 테마 배경에서 파생한다 (활성 탭이 비활성보다 살짝 밝음).
    ///
    /// 탭의 셸 상태(`TabStatus`)는 제목 앞의 점으로 나타낸다. 색은 블록
    /// 거터와 같은 의미다 — 실행 중·성공·실패. 점이 없으면 조용한 탭이다.
    pub(super) fn draw_tab_bar(
        &mut self,
        tabs: &[TabLabel],
        active_tab: usize,
        bg_instances: &mut Vec<BgInstance>,
        text_instances: &mut Vec<TextInstance>,
    ) {
        let theme = self.theme;
        let bar_h = self.tab_bar_height();
        let width = self.config.width as f32;

        fill(bg_instances, [0.0, 0.0, width, bar_h], theme.chrome());

        let tab_count = tabs.len().max(1);
        let tab_width = width / tab_count as f32;
        let label_y = (bar_h - self.ui_line_height) / 2.0;
        let pad = self.ui_advance;
        // 상태 점 + 간격. 점이 있을 때만 폭을 차지해 제목이 여전히 가운데 온다.
        let dot_w = self.ui_advance * 1.6;

        for (i, TabLabel { title, status }) in tabs.iter().enumerate() {
            let tab_x = i as f32 * tab_width;
            let dot = match status {
                TabStatus::Idle => None,
                TabStatus::Running => Some(theme.block_running),
                TabStatus::Done(Some(0)) => Some(theme.block_ok),
                TabStatus::Done(_) => Some(theme.block_fail),
            };
            let dot_space = if dot.is_some() { dot_w } else { 0.0 };
            // 탭이 하나뿐이면 강조할 대상이 없으므로 크롬 배경 그대로 둔다.
            if i == active_tab && tab_count > 1 {
                fill(
                    bg_instances,
                    [tab_x, 0.0, tab_width, bar_h],
                    theme.active_tab(),
                );
            }
            let fg = if i == active_tab {
                theme.fg
            } else {
                theme.inactive_fg()
            };

            // 제목만 표시 (번호 없음), 폭에 맞게 자르고 가운데 정렬
            let (label, label_w) =
                self.truncate_to_width(title, tab_width - pad * 2.0 - dot_space, FontSize::Ui);
            let start_x = tab_x + ((tab_width - label_w - dot_space) / 2.0).max(pad);
            if let Some(color) = dot {
                self.draw_text(
                    "●",
                    start_x,
                    label_y,
                    tab_x + tab_width,
                    color,
                    FontSize::Ui,
                    text_instances,
                );
            }
            let label_x = start_x + dot_space;
            self.draw_text(
                &label,
                label_x,
                label_y,
                tab_x + tab_width,
                fg,
                FontSize::Ui,
                text_instances,
            );

            // 탭 사이 얇은 구분선
            if i > 0 {
                fill(
                    bg_instances,
                    [tab_x, bar_h * 0.2, 1.0, bar_h * 0.6],
                    theme.separator(),
                );
            }
        }
    }

    /// 하단 상태바. 왼쪽 정렬 `left`, 오른쪽 정렬 `right`.
    pub(super) fn draw_status_bar(
        &mut self,
        left: &str,
        right: &str,
        bg_instances: &mut Vec<BgInstance>,
        text_instances: &mut Vec<TextInstance>,
    ) {
        let theme = self.theme;
        let bar_h = self.status_bar_height();
        let width = self.config.width as f32;
        let y = self.config.height as f32 - bar_h;
        fill(bg_instances, [0.0, y, width, bar_h], theme.chrome());

        let text_y = y + (bar_h - self.ui_line_height) / 2.0;
        let pad = self.ui_advance;
        let max_x = width - pad;

        let left_end = self.draw_text(
            left,
            pad,
            text_y,
            max_x,
            theme.fg,
            FontSize::Ui,
            text_instances,
        );
        // 오른쪽 블록은 우측 정렬하되, 왼쪽 텍스트를 침범하지 않는다.
        // 남는 폭이 모자라면 `draw_text`가 `max_x`에서 자른다 — 창 밖으로 넘겨
        // 그리는 대신 바 안에서 끊는다 (긴 cwd + 좁은 창).
        let right_x = (max_x - self.text_width(right, FontSize::Ui)).max(left_end);
        self.draw_text(
            right,
            right_x,
            text_y,
            max_x,
            theme.fg,
            FontSize::Ui,
            text_instances,
        );
    }

    /// 화면 하단의 AI 입력 바. IME 후보창 배치를 위해 입력 끝 좌표를 돌려준다.
    pub(super) fn draw_ai_bar(
        &mut self,
        line: &str,
        bg_instances: &mut Vec<BgInstance>,
        text_instances: &mut Vec<TextInstance>,
    ) -> (f64, f64) {
        let theme = self.theme;
        let bar_h = self.tab_bar_height();
        let width = self.config.width as f32;
        let y = self.config.height as f32 - bar_h;
        fill(bg_instances, [0.0, y, width, bar_h], AI_BAR_BG);

        let label_y = y + (bar_h - self.cell_height) / 2.0;
        let pad = self.cell_width;
        let end_x = self.draw_text(
            line,
            pad,
            label_y,
            width - pad,
            theme.fg,
            FontSize::Term,
            text_instances,
        );
        (end_x as f64, (y + bar_h) as f64)
    }

    /// 화면 하단의 검색 바. AI 바와 같은 자리를 쓰되 색으로 구분한다.
    ///
    /// 왼쪽에 쿼리, 오른쪽에 `3/17` 같은 카운터를 우측 정렬한다.
    /// IME 후보창 배치를 위해 입력 끝 좌표를 돌려준다.
    pub(super) fn draw_search_bar(
        &mut self,
        line: &str,
        status: &str,
        bg_instances: &mut Vec<BgInstance>,
        text_instances: &mut Vec<TextInstance>,
    ) -> (f64, f64) {
        let theme = self.theme;
        let bar_h = self.tab_bar_height();
        let width = self.config.width as f32;
        let y = self.config.height as f32 - bar_h;
        fill(bg_instances, [0.0, y, width, bar_h], SEARCH_BAR_BG);

        let label_y = y + (bar_h - self.cell_height) / 2.0;
        let pad = self.cell_width;

        // 카운터 자리를 먼저 확보해 긴 쿼리가 그 위로 넘어오지 않게 한다.
        let status_w = if status.is_empty() {
            0.0
        } else {
            self.text_width(status, FontSize::Term) + pad
        };
        let query_max_x = width - pad - status_w;

        let end_x = self.draw_text(
            line,
            pad,
            label_y,
            query_max_x,
            theme.fg,
            FontSize::Term,
            text_instances,
        );

        if !status.is_empty() {
            let status_x = width - pad - self.text_width(status, FontSize::Term);
            self.draw_text(
                status,
                status_x,
                label_y,
                width - pad,
                theme.inactive_fg(),
                FontSize::Term,
                text_instances,
            );
        }

        (end_x as f64, (y + bar_h) as f64)
    }

    /// 커맨드 팔레트 오버레이 (화면 중앙 상단). 쿼리 줄 + 필터된 액션 목록.
    pub(super) fn draw_palette(
        &mut self,
        query: &str,
        items: &[String],
        selected: usize,
        bg_instances: &mut Vec<BgInstance>,
        text_instances: &mut Vec<TextInstance>,
    ) {
        let theme = self.theme;
        let screen_w = self.config.width as f32;
        let row_h = self.cell_height;
        let rows = items.len().min(PALETTE_MAX_ROWS);
        let box_w = (screen_w * 0.6).min(720.0);
        let box_x = (screen_w - box_w) / 2.0;
        let box_y = self.tab_bar_height() + row_h;
        // 쿼리 줄(1) + 목록(rows), 위아래 여백 0.5줄
        let box_h = row_h * (rows as f32 + 1.5);

        fill(bg_instances, [box_x, box_y, box_w, box_h], PALETTE_BG);

        let text_x = box_x + self.cell_width;
        let max_x = box_x + box_w - self.cell_width;

        // 쿼리 줄
        self.draw_text(
            &format!("{query}_"),
            text_x,
            box_y + row_h * 0.25,
            max_x,
            theme.fg,
            FontSize::Term,
            text_instances,
        );

        // 액션 목록
        for (i, item) in items.iter().take(rows).enumerate() {
            let row_y = box_y + row_h * (i as f32 + 1.5);
            if i == selected {
                fill(bg_instances, [box_x, row_y, box_w, row_h], theme.selection);
            }
            self.draw_text(
                item,
                text_x,
                row_y,
                max_x,
                theme.fg,
                FontSize::Term,
                text_instances,
            );
        }
    }

    /// 힌트 라벨 배지. 링크 첫 글자 위에 겹쳐 그린다 (화면 픽셀 좌표).
    pub(super) fn draw_hints(
        &mut self,
        hints: &[HintBadge],
        bg_instances: &mut Vec<BgInstance>,
        text_instances: &mut Vec<TextInstance>,
    ) {
        for hint in hints {
            let w = hint.label.chars().count() as f32 * self.cell_width;
            fill(bg_instances, [hint.x, hint.y, w, self.cell_height], HINT_BG);
            let mut x = hint.x;
            for (i, ch) in hint.label.chars().enumerate() {
                let color = if i < hint.typed {
                    HINT_TYPED_FG
                } else {
                    HINT_FG
                };
                x = self.draw_text(
                    &ch.to_string(),
                    x,
                    hint.y,
                    hint.x + w,
                    color,
                    FontSize::Term,
                    text_instances,
                );
            }
        }
    }

    /// 여러 줄 패널 (AI 에러 설명). 팔레트와 같은 자리·폭에, 제목 한 줄 +
    /// 폭에 맞춰 줄바꿈한 본문 + 하단 안내 한 줄. 스트리밍 중에는 본문이
    /// 계속 늘어나므로 넘치면 마지막 줄들을 보여준다.
    pub(super) fn draw_panel(
        &mut self,
        title: &str,
        body: &str,
        footer: &str,
        bg_instances: &mut Vec<BgInstance>,
        text_instances: &mut Vec<TextInstance>,
    ) {
        let theme = self.theme;
        let screen_w = self.config.width as f32;
        let row_h = self.cell_height;
        let box_w = (screen_w * 0.7).min(900.0);
        let box_x = (screen_w - box_w) / 2.0;
        let box_y = self.tab_bar_height() + row_h;
        let text_x = box_x + self.cell_width;
        let max_x = box_x + box_w - self.cell_width;

        let lines = self.wrap_text(body, max_x - text_x);
        let shown = &lines[lines.len().saturating_sub(PANEL_MAX_ROWS)..];
        // 제목(1) + 빈 줄(0.5) + 본문 + 빈 줄(0.5) + 안내(1), 위아래 여백 0.25씩
        let box_h = row_h * (shown.len() as f32 + 3.5);
        fill(bg_instances, [box_x, box_y, box_w, box_h], PALETTE_BG);

        self.draw_text(
            title,
            text_x,
            box_y + row_h * 0.25,
            max_x,
            theme.fg,
            FontSize::Term,
            text_instances,
        );
        for (i, line) in shown.iter().enumerate() {
            self.draw_text(
                line,
                text_x,
                box_y + row_h * (i as f32 + 1.75),
                max_x,
                theme.fg,
                FontSize::Term,
                text_instances,
            );
        }
        self.draw_text(
            footer,
            text_x,
            box_y + box_h - row_h * 1.25,
            max_x,
            theme.inactive_fg(),
            FontSize::Term,
            text_instances,
        );
    }

    /// 픽셀 폭에 맞춰 줄바꿈한다. 단어 경계를 우선하되, 한 단어가 폭보다
    /// 길면(긴 경로·URL) 글자 단위로 자른다. 원문의 개행은 그대로 줄이 된다.
    fn wrap_text(&self, text: &str, max_w: f32) -> Vec<String> {
        let mut out = Vec::new();
        for para in text.split('\n') {
            let mut line = String::new();
            let mut width = 0.0;
            for word in para.split_inclusive(' ') {
                let word_w: f32 = word
                    .chars()
                    .map(|c| self.char_advance(c, FontSize::Term))
                    .sum();
                if width + word_w > max_w && !line.is_empty() {
                    out.push(std::mem::take(&mut line).trim_end().to_string());
                    width = 0.0;
                }
                for ch in word.chars() {
                    let w = self.char_advance(ch, FontSize::Term);
                    if width + w > max_w && !line.is_empty() {
                        out.push(std::mem::take(&mut line));
                        width = 0.0;
                    }
                    line.push(ch);
                    width += w;
                }
            }
            out.push(line.trim_end().to_string());
        }
        out
    }
}
