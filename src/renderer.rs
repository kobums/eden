//! wgpu 기반 렌더러: glyph atlas에 글리프를 캐싱하고,
//! 셀 배경 → 글리프 순서의 인스턴스 드로우 2패스로 그리드를 그린다.

use std::collections::HashMap;
use std::sync::Arc;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::Term;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, NamedColor};
use bytemuck::{Pod, Zeroable};
use winit::window::Window;

use crate::layout::Rect;
use crate::session::{Block, EventProxy};

/// 한 페인을 그리는 데 필요한 정보.
pub struct PaneView<'a> {
    pub term: &'a FairMutex<Term<EventProxy>>,
    pub blocks: &'a [Block],
    pub rect: Rect,
    pub focused: bool,
}

/// 창 가장자리 여백 (물리 픽셀, 스케일 적용 전).
const PADDING: f32 = 8.0;
const ATLAS_SIZE: u32 = 2048;

/// 설정에서 온 배경/전경/선택/커서 색 + 16색 ANSI 팔레트.
#[derive(Clone, Copy)]
struct Theme {
    bg: [f32; 3],
    fg: [f32; 3],
    selection: [f32; 3],
    cursor: [f32; 3],
    palette: [[f32; 3]; 16],
    cursor_style: crate::config::CursorStyle,
    /// 기본 배경 불투명도 (셀 배경색·텍스트는 항상 불투명 — iTerm2와 동일)
    opacity: f32,
}

/// 블록 상태 바 색: 실행 중 / 성공 / 실패
const BLOCK_RUNNING: [f32; 3] = [0.35, 0.55, 0.95];
const BLOCK_OK: [f32; 3] = [0.35, 0.72, 0.46];
const BLOCK_FAIL: [f32; 3] = [0.92, 0.42, 0.46];


/// AI 입력 바 색.
const AI_BAR_BG: [f32; 3] = [0.1, 0.14, 0.24];

/// macOS 시스템 고정폭 폰트 후보 (앞에서부터 시도, 첫 성공이 주 폰트).
const FONT_CANDIDATES: &[&str] = &[
    "/System/Library/Fonts/Menlo.ttc",
    "/System/Library/Fonts/Monaco.ttf",
    "/Library/Fonts/SF-Mono-Regular.otf",
];

/// 주 폰트에 없는 글리프(한글 등)를 위한 폴백 폰트.
const FALLBACK_FONTS: &[&str] = &[
    "/System/Library/Fonts/AppleSDGothicNeo.ttc",
    "/System/Library/Fonts/Apple Symbols.ttf",
];

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    screen: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct BgInstance {
    rect: [f32; 4],
    color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct TextInstance {
    rect: [f32; 4],
    uv: [f32; 4],
    color: [f32; 4],
}

/// atlas에 캐싱된 글리프 하나.
#[derive(Clone, Copy)]
struct Glyph {
    /// atlas 내 정규화 uv (x, y, w, h)
    uv: [f32; 4],
    /// 물리 픽셀 크기
    size: [f32; 2],
    /// 셀 원점 기준 오프셋 (x: 왼쪽 베어링, y: 셀 상단→글리프 상단)
    offset: [f32; 2],
}

/// 글리프 크기 종류: 터미널 본문 / UI 크롬(탭·상태바, 더 작게).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum FontSize {
    Term,
    Ui,
}

struct Atlas {
    texture: wgpu::Texture,
    glyphs: HashMap<(char, FontSize), Option<Glyph>>,
    cursor_x: u32,
    cursor_y: u32,
    row_height: u32,
}

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,

    bg_pipeline: wgpu::RenderPipeline,
    text_pipeline: wgpu::RenderPipeline,
    globals_buffer: wgpu::Buffer,
    globals_bind_group: wgpu::BindGroup,
    atlas_bind_group: wgpu::BindGroup,

    bg_buffer: wgpu::Buffer,
    bg_capacity: usize,
    text_buffer: wgpu::Buffer,
    text_capacity: usize,

    atlas: Atlas,
    fonts: Vec<fontdue::Font>,
    font_px: f32,
    ascent: f32,
    /// UI 크롬(탭/상태바)용 작은 폰트 크기와 메트릭 (iTerm2식 탭 텍스트)
    ui_px: f32,
    ui_ascent: f32,
    ui_advance: f32,
    ui_line_height: f32,
    theme: Theme,

    pub cell_width: f32,
    pub cell_height: f32,
}

impl Renderer {
    pub fn new(window: Arc<Window>, config: &crate::config::Config) -> Self {
        let scale = window.scale_factor() as f32;
        let size = window.inner_size();
        let theme = Theme {
            bg: config.background,
            fg: config.foreground,
            selection: config.selection,
            cursor: config.cursor,
            palette: config.palette,
            cursor_style: config.cursor_style,
            opacity: config.background_opacity,
        };
        // 아래에서 wgpu의 SurfaceConfiguration도 `config`라는 지역 변수를 쓰므로,
        // 앱 설정 값은 여기서 미리 꺼내 둔다.
        let cfg_font_size = config.font_size;
        let cfg_font_path = config.font_path.clone();

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance.create_surface(window).expect("surface 생성 실패");
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .expect("GPU 어댑터 없음");
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .expect("GPU 디바이스 생성 실패");

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .unwrap_or(caps.formats[0]);
        // 투명도 사용 시 스트레이트 알파(PostMultiplied)로 컴포지트.
        // 미지원이면 불투명으로 폴백한다.
        let alpha_mode = if theme.opacity < 1.0
            && caps
                .alpha_modes
                .contains(&wgpu::CompositeAlphaMode::PostMultiplied)
        {
            wgpu::CompositeAlphaMode::PostMultiplied
        } else {
            caps.alpha_modes[0]
        };
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        // --- 폰트 ---
        // 설정의 font-path가 있으면 우선, 없으면 시스템 후보.
        let load_font = |path: &str| -> Option<fontdue::Font> {
            let bytes = std::fs::read(path).ok()?;
            fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()).ok()
        };
        let primary = cfg_font_path
            .as_deref()
            .and_then(load_font)
            .or_else(|| FONT_CANDIDATES.iter().find_map(|p| load_font(p)))
            .expect("고정폭 폰트를 찾지 못함");
        let mut fonts = vec![primary];
        for path in FALLBACK_FONTS {
            if let Some(font) = load_font(path) {
                fonts.push(font);
            }
        }
        let font_px = cfg_font_size * scale;
        let line_metrics = fonts[0]
            .horizontal_line_metrics(font_px)
            .expect("폰트 라인 메트릭 없음");
        let ascent = line_metrics.ascent;
        let cell_height = (line_metrics.ascent - line_metrics.descent + line_metrics.line_gap)
            .ceil();
        let cell_width = fonts[0].metrics('M', font_px).advance_width.round();

        // UI 크롬(탭/상태바)용 작은 폰트 — iTerm2 탭 텍스트처럼 본문보다 작게
        let ui_px = (font_px * 0.8).round();
        let ui_metrics = fonts[0]
            .horizontal_line_metrics(ui_px)
            .expect("폰트 라인 메트릭 없음");
        let ui_ascent = ui_metrics.ascent;
        let ui_line_height =
            (ui_metrics.ascent - ui_metrics.descent + ui_metrics.line_gap).ceil();
        let ui_advance = fonts[0].metrics('M', ui_px).advance_width;

        // --- atlas ---
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph atlas"),
            size: wgpu::Extent3d {
                width: ATLAS_SIZE,
                height: ATLAS_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let atlas = Atlas {
            texture,
            glyphs: HashMap::new(),
            cursor_x: 0,
            cursor_y: 0,
            row_height: 0,
        };

        // --- 파이프라인 ---
        let shader = device.create_shader_module(wgpu::include_wgsl!("shader.wgsl"));

        let globals_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let globals_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals bind group"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals_buffer.as_entire_binding(),
            }],
        });

        let atlas_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("atlas layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("atlas sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let atlas_view = atlas
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let atlas_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("atlas bind group"),
            layout: &atlas_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        let bg_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("bg pipeline layout"),
            bind_group_layouts: &[Some(&globals_layout)],
            immediate_size: 0,
        });
        let text_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("text pipeline layout"),
            bind_group_layouts: &[Some(&globals_layout), Some(&atlas_layout)],
            immediate_size: 0,
        });

        let blend = wgpu::BlendState::ALPHA_BLENDING;
        let bg_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("bg pipeline"),
            layout: Some(&bg_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_bg"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<BgInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_bg"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(blend),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let text_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("text pipeline"),
            layout: Some(&text_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_text"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<TextInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x4, 1 => Float32x4, 2 => Float32x4
                    ],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_text"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(blend),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let bg_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bg instances"),
            size: 1024,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let text_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("text instances"),
            size: 1024,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            surface,
            device,
            queue,
            config,
            bg_pipeline,
            text_pipeline,
            globals_buffer,
            globals_bind_group,
            atlas_bind_group,
            bg_buffer,
            bg_capacity: 1024,
            text_buffer,
            text_capacity: 1024,
            atlas,
            fonts,
            font_px,
            ascent,
            ui_px,
            ui_ascent,
            ui_advance,
            ui_line_height,
            theme,
            cell_width,
            cell_height,
        }
    }

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

    /// 페인 사각형에서 그리드 크기(열, 행)를 계산한다.
    pub fn pane_grid_size(&self, rect: Rect) -> (usize, usize) {
        let cols = ((rect.w - PADDING * 2.0) / self.cell_width).floor() as usize;
        let lines = ((rect.h - PADDING * 2.0) / self.cell_height).floor() as usize;
        (cols.max(2), lines.max(1))
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
    }

    /// 터미널 본문 크기 글리프.
    fn glyph(&mut self, c: char) -> Option<Glyph> {
        self.glyph_sized(c, FontSize::Term)
    }

    /// UI(탭/상태바) 크기 글리프.
    fn ui_glyph(&mut self, c: char) -> Option<Glyph> {
        self.glyph_sized(c, FontSize::Ui)
    }

    /// 글리프를 atlas에서 찾거나 새로 래스터라이즈해 올린다.
    /// 터미널 본문(Term)과 UI 크롬(Ui, 더 작은 크기) 두 크기를 캐싱한다.
    fn glyph_sized(&mut self, c: char, size: FontSize) -> Option<Glyph> {
        let key = (c, size);
        if let Some(cached) = self.atlas.glyphs.get(&key) {
            return *cached;
        }
        let (px, ascent) = match size {
            FontSize::Term => (self.font_px, self.ascent),
            FontSize::Ui => (self.ui_px, self.ui_ascent),
        };

        // 주 폰트 → 폴백 폰트 순서로 글리프를 가진 폰트를 찾는다.
        // 단 PUA(사용자 영역, powerline 아이콘 등)는 폴백 폰트가 엉뚱한 글리프를
        // 돌려주는 경우가 있어 주 폰트에서만 찾는다.
        let is_pua = ('\u{E000}'..='\u{F8FF}').contains(&c);
        let font = if is_pua {
            self.fonts.first().filter(|f| f.lookup_glyph_index(c) != 0)
        } else {
            self.fonts.iter().find(|f| f.lookup_glyph_index(c) != 0)
        };
        let glyph = if let Some(font) = font {
            let (metrics, bitmap) = font.rasterize(c, px);
            if metrics.width == 0 || metrics.height == 0 {
                None
            } else {
                let w = metrics.width as u32;
                let h = metrics.height as u32;
                // shelf packing
                if self.atlas.cursor_x + w + 1 > ATLAS_SIZE {
                    self.atlas.cursor_x = 0;
                    self.atlas.cursor_y += self.atlas.row_height + 1;
                    self.atlas.row_height = 0;
                }
                if self.atlas.cursor_y + h + 1 > ATLAS_SIZE {
                    // atlas 가득 참 — Phase 2에서 축출/증설 처리
                    self.atlas.glyphs.insert(key, None);
                    return None;
                }
                let x = self.atlas.cursor_x;
                let y = self.atlas.cursor_y;
                self.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &self.atlas.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d { x, y, z: 0 },
                        aspect: wgpu::TextureAspect::All,
                    },
                    &bitmap,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(w),
                        rows_per_image: Some(h),
                    },
                    wgpu::Extent3d {
                        width: w,
                        height: h,
                        depth_or_array_layers: 1,
                    },
                );
                self.atlas.cursor_x += w + 1;
                self.atlas.row_height = self.atlas.row_height.max(h);

                let inv = 1.0 / ATLAS_SIZE as f32;
                Some(Glyph {
                    uv: [x as f32 * inv, y as f32 * inv, w as f32 * inv, h as f32 * inv],
                    size: [metrics.width as f32, metrics.height as f32],
                    offset: [
                        metrics.xmin as f32,
                        // 셀 상단 → 글리프 상단: baseline(ascent) 위로 (height + ymin)만큼
                        ascent - metrics.height as f32 - metrics.ymin as f32,
                    ],
                })
            }
        } else {
            None
        };

        self.atlas.glyphs.insert(key, glyph);
        glyph
    }

    /// 모든 페인과 탭 바를 그린다.
    /// IME 후보창 배치를 위해 포커스된 페인의 커서 물리 좌표(좌하단)를 돌려준다.
    pub fn draw(
        &mut self,
        panes: &[PaneView],
        preedit: Option<&str>,
        tab_titles: &[String],
        active_tab: usize,
        ai_bar: Option<&str>,
        palette: Option<(&str, &[String], usize)>,
        status: Option<(&str, &str)>,
    ) -> Option<(f64, f64)> {
        let mut bg_instances: Vec<BgInstance> = Vec::new();
        let mut text_instances: Vec<TextInstance> = Vec::new();
        let mut ime_pos = None;

        if let Some((left, right)) = status {
            self.draw_status_bar(left, right, &mut bg_instances, &mut text_instances);
        }

        for pane in panes {
            let pane_ime = self.draw_pane(
                pane,
                if pane.focused { preedit } else { None },
                &mut bg_instances,
                &mut text_instances,
            );
            if pane.focused {
                ime_pos = pane_ime;
            }
        }

        self.draw_tab_bar(tab_titles, active_tab, &mut bg_instances, &mut text_instances);
        if let Some(line) = ai_bar {
            ime_pos = Some(self.draw_ai_bar(line, &mut bg_instances, &mut text_instances));
        }
        if let Some((query, items, selected)) = palette {
            self.draw_palette(query, items, selected, &mut bg_instances, &mut text_instances);
        }
        self.submit(&bg_instances, &text_instances);
        ime_pos
    }

    /// 하단 상태바. 왼쪽 정렬 `left`, 오른쪽 정렬 `right`.
    fn draw_status_bar(
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
        let bar_bg = mix(theme.bg, [0.0; 3], 0.35);
        bg_instances.push(BgInstance {
            rect: [0.0, y, width, bar_h],
            color: [bar_bg[0], bar_bg[1], bar_bg[2], 1.0],
        });

        let text_y = y + (bar_h - self.ui_line_height) / 2.0;
        let ui_adv = self.ui_advance;
        let char_w = |c: char| {
            use unicode_width::UnicodeWidthChar;
            ui_adv * if c.width().unwrap_or(1) >= 2 { 2.0 } else { 1.0 }
        };
        // 왼쪽
        let mut x = ui_adv;
        for ch in left.chars() {
            let adv = char_w(ch);
            if x + adv > width - ui_adv {
                break;
            }
            if let Some(g) = self.ui_glyph(ch) {
                text_instances.push(TextInstance {
                    rect: [x + g.offset[0], text_y + g.offset[1], g.size[0], g.size[1]],
                    uv: g.uv,
                    color: [theme.fg[0], theme.fg[1], theme.fg[2], 1.0],
                });
            }
            x += adv;
        }
        // 오른쪽 (폭 계산 후 우측 정렬)
        let right_w: f32 = right.chars().map(char_w).sum();
        let mut rx = (width - ui_adv - right_w).max(x);
        for ch in right.chars() {
            let adv = char_w(ch);
            if let Some(g) = self.ui_glyph(ch) {
                text_instances.push(TextInstance {
                    rect: [rx + g.offset[0], text_y + g.offset[1], g.size[0], g.size[1]],
                    uv: g.uv,
                    color: [theme.fg[0], theme.fg[1], theme.fg[2], 1.0],
                });
            }
            rx += adv;
        }
    }

    /// 커맨드 팔레트 오버레이 (화면 중앙 상단). 쿼리 줄 + 필터된 액션 목록.
    fn draw_palette(
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
        let rows = items.len().min(10);
        let box_w = (screen_w * 0.6).min(720.0);
        let box_x = (screen_w - box_w) / 2.0;
        let box_y = self.tab_bar_height() + row_h;
        let box_h = row_h * (rows as f32 + 1.5);

        // 팔레트 배경
        bg_instances.push(BgInstance {
            rect: [box_x, box_y, box_w, box_h],
            color: [0.12, 0.13, 0.18, 1.0],
        });

        let text_x = box_x + self.cell_width;
        let draw_text = |s: &str, x: f32, y: f32, color: [f32; 3],
                         renderer: &mut Self,
                         out: &mut Vec<TextInstance>| {
            let mut cx = x;
            for ch in s.chars() {
                use unicode_width::UnicodeWidthChar;
                let adv = renderer.cell_width * if ch.width().unwrap_or(1) >= 2 { 2.0 } else { 1.0 };
                if cx + adv > box_x + box_w - renderer.cell_width {
                    break;
                }
                if let Some(g) = renderer.glyph(ch) {
                    out.push(TextInstance {
                        rect: [cx + g.offset[0], y + g.offset[1], g.size[0], g.size[1]],
                        uv: g.uv,
                        color: [color[0], color[1], color[2], 1.0],
                    });
                }
                cx += adv;
            }
        };

        // 쿼리 줄
        let query_y = box_y + row_h * 0.25;
        draw_text(
            &format!("> {query}_"),
            text_x,
            query_y,
            theme.fg,
            self,
            text_instances,
        );

        // 액션 목록
        for (i, item) in items.iter().take(rows).enumerate() {
            let row_y = box_y + row_h * (i as f32 + 1.5);
            if i == selected {
                bg_instances.push(BgInstance {
                    rect: [box_x, row_y, box_w, row_h],
                    color: [theme.selection[0], theme.selection[1], theme.selection[2], 1.0],
                });
            }
            draw_text(item, text_x, row_y, theme.fg, self, text_instances);
        }
    }

    /// 화면 하단의 AI 입력 바. IME 후보창 배치를 위해 입력 끝 좌표를 돌려준다.
    fn draw_ai_bar(
        &mut self,
        line: &str,
        bg_instances: &mut Vec<BgInstance>,
        text_instances: &mut Vec<TextInstance>,
    ) -> (f64, f64) {
        let theme = self.theme;
        let bar_h = self.tab_bar_height();
        let width = self.config.width as f32;
        let y = self.config.height as f32 - bar_h;
        bg_instances.push(BgInstance {
            rect: [0.0, y, width, bar_h],
            color: [AI_BAR_BG[0], AI_BAR_BG[1], AI_BAR_BG[2], 1.0],
        });

        let label_y = y + (bar_h - self.cell_height) / 2.0;
        let mut x = self.cell_width;
        for ch in line.chars() {
            use unicode_width::UnicodeWidthChar;
            let advance = self.cell_width * if ch.width().unwrap_or(1) >= 2 { 2.0 } else { 1.0 };
            if x + advance > width - self.cell_width {
                break;
            }
            if let Some(glyph) = self.glyph(ch) {
                text_instances.push(TextInstance {
                    rect: [
                        x + glyph.offset[0],
                        label_y + glyph.offset[1],
                        glyph.size[0],
                        glyph.size[1],
                    ],
                    uv: glyph.uv,
                    color: [theme.fg[0], theme.fg[1], theme.fg[2], 1.0],
                });
            }
            x += advance;
        }
        (x as f64, (y + bar_h) as f64)
    }

    /// 페인 하나를 인스턴스 버퍼에 그린다. 포커스된 페인이면 커서 좌표를 돌려준다.
    fn draw_pane(
        &mut self,
        view: &PaneView,
        preedit: Option<&str>,
        bg_instances: &mut Vec<BgInstance>,
        text_instances: &mut Vec<TextInstance>,
    ) -> Option<(f64, f64)> {
        let theme = self.theme;
        let rect = view.rect;
        let origin_x = rect.x + PADDING;
        let origin_y = rect.y + PADDING;
        let blocks = view.blocks;
        let mut ime_pos = None;

        // 페인 배경 (구분선은 페인 사이 틈으로 드러난다).
        // iTerm2처럼 기본 배경에만 투명도를 적용한다 (셀 배경색·텍스트는 불투명).
        bg_instances.push(BgInstance {
            rect: [rect.x, rect.y, rect.w, rect.h],
            color: [theme.bg[0], theme.bg[1], theme.bg[2], theme.opacity],
        });

        {
            let term = view.term.lock();
            let content = term.renderable_content();
            // 스크롤백을 위로 올렸을 때: 그리드 좌표(line)는 화면 좌표(row)와
            // display_offset만큼 어긋난다.
            let display_offset = content.display_offset as i32;
            let selection = content.selection;
            let cursor_point = content.cursor.point;
            let history = term.grid().history_size() as i64;
            let cursor_row = cursor_point.line.0 + display_offset;
            let visible_lines = Dimensions::screen_lines(term.grid()) as i32;
            let cursor_visible = cursor_row >= 0 && cursor_row < visible_lines;
            let cursor_x = origin_x + cursor_point.column.0 as f32 * self.cell_width;
            let cursor_y = origin_y + cursor_row as f32 * self.cell_height;
            if cursor_visible {
                ime_pos = Some((cursor_x as f64, (cursor_y + self.cell_height) as f64));
            }

            // --- 블록 상태 바 (왼쪽 거터) ---
            // 화면 최상단의 절대 줄 번호. 블록의 절대 줄 → 화면 행 변환에 사용.
            let top_abs = history - display_offset as i64;
            let bottom_abs = history + cursor_point.line.0 as i64;
            for block in blocks {
                if block.cmd_abs.is_none() {
                    continue; // 명령이 실행되지 않은 프롬프트는 표시하지 않음
                }
                let end_abs = block.end_abs.map(|d| d - 1).unwrap_or(bottom_abs);
                let top_row = block.start_abs - top_abs;
                let bottom_row = (end_abs - top_abs).min(visible_lines as i64 - 1);
                if bottom_row < 0 || top_row >= visible_lines as i64 {
                    continue;
                }
                let top_row = top_row.max(0);
                let color = match (block.end_abs, block.exit) {
                    (None, _) => BLOCK_RUNNING,
                    (_, Some(0)) => BLOCK_OK,
                    (_, Some(_)) => BLOCK_FAIL,
                    _ => BLOCK_OK,
                };
                bg_instances.push(BgInstance {
                    rect: [
                        rect.x + 1.0,
                        origin_y + top_row as f32 * self.cell_height,
                        PADDING - 2.0,
                        (bottom_row - top_row + 1) as f32 * self.cell_height,
                    ],
                    color: [color[0], color[1], color[2], 1.0],
                });
            }

            for indexed in content.display_iter {
                let row = indexed.point.line.0 + display_offset;
                if row < 0 {
                    continue;
                }
                let col = indexed.point.column.0;
                let x = origin_x + col as f32 * self.cell_width;
                let y = origin_y + row as f32 * self.cell_height;

                let flags = indexed.flags;
                if flags.contains(Flags::WIDE_CHAR_SPACER) {
                    continue;
                }

                let mut fg = ansi_to_rgb(&indexed.fg, &theme);
                let mut bg = ansi_to_rgb(&indexed.bg, &theme);
                if flags.contains(Flags::INVERSE) {
                    std::mem::swap(&mut fg, &mut bg);
                }

                let selected = selection
                    .as_ref()
                    .is_some_and(|range| range.contains(indexed.point));
                if selected {
                    bg = theme.selection;
                }

                let is_cursor = view.focused
                    && cursor_visible
                    && preedit.is_none()
                    && indexed.point == cursor_point;
                // 블록 커서만 셀을 반전한다. 바/밑줄은 루프 뒤에서 사각형으로 그린다.
                let block_cursor =
                    is_cursor && theme.cursor_style == crate::config::CursorStyle::Block;
                if block_cursor {
                    bg = theme.cursor;
                    fg = theme.bg;
                }

                let width_cells = if flags.contains(Flags::WIDE_CHAR) { 2.0 } else { 1.0 };
                if block_cursor || selected || bg != theme.bg {
                    bg_instances.push(BgInstance {
                        rect: [x, y, self.cell_width * width_cells, self.cell_height],
                        color: [bg[0], bg[1], bg[2], 1.0],
                    });
                }

                // OSC 8 하이퍼링크: 밑줄로 클릭 가능함을 표시
                if indexed.hyperlink().is_some() {
                    bg_instances.push(BgInstance {
                        rect: [
                            x,
                            y + self.cell_height - 2.0,
                            self.cell_width * width_cells,
                            1.5,
                        ],
                        color: [fg[0], fg[1], fg[2], 1.0],
                    });
                }

                let c = indexed.c;
                if c == ' ' || flags.contains(Flags::HIDDEN) {
                    continue;
                }
                // 주의: self.glyph는 &mut self가 필요하므로 lock 밖으로 문자를 모으는
                // 대신, FairMutex 잠금 중 atlas 업로드를 허용한다 (queue.write_texture는
                // 즉시 반환되므로 잠금 시간에 미치는 영향은 작다).
                if let Some(glyph) = self.glyph(c) {
                    text_instances.push(TextInstance {
                        rect: [
                            x + glyph.offset[0],
                            y + glyph.offset[1],
                            glyph.size[0],
                            glyph.size[1],
                        ],
                        uv: glyph.uv,
                        color: [fg[0], fg[1], fg[2], 1.0],
                    });
                }
            }

            // 바/밑줄 커서 (블록은 셀 반전으로 이미 처리됨)
            if view.focused && cursor_visible && preedit.is_none() {
                use crate::config::CursorStyle;
                let c = theme.cursor;
                match theme.cursor_style {
                    CursorStyle::Bar => bg_instances.push(BgInstance {
                        rect: [cursor_x, cursor_y, 2.0, self.cell_height],
                        color: [c[0], c[1], c[2], 1.0],
                    }),
                    CursorStyle::Underline => bg_instances.push(BgInstance {
                        rect: [
                            cursor_x,
                            cursor_y + self.cell_height - 2.0,
                            self.cell_width,
                            2.0,
                        ],
                        color: [c[0], c[1], c[2], 1.0],
                    }),
                    CursorStyle::Block => {}
                }
            }

            // IME 조합 중 문자열(preedit)을 커서 위치에 오버레이로 그린다.
            if let (Some(text), true) = (preedit, cursor_visible) {
                let mut x = cursor_x;
                for ch in text.chars() {
                    use unicode_width::UnicodeWidthChar;
                    let cells = if ch.width().unwrap_or(1) >= 2 { 2.0 } else { 1.0 };
                    let w = self.cell_width * cells;
                    bg_instances.push(BgInstance {
                        rect: [x, cursor_y, w, self.cell_height],
                        color: [theme.fg[0], theme.fg[1], theme.fg[2], 1.0],
                    });
                    if let Some(glyph) = self.glyph(ch) {
                        text_instances.push(TextInstance {
                            rect: [
                                x + glyph.offset[0],
                                cursor_y + glyph.offset[1],
                                glyph.size[0],
                                glyph.size[1],
                            ],
                            uv: glyph.uv,
                            color: [theme.bg[0], theme.bg[1], theme.bg[2], 1.0],
                        });
                    }
                    x += w;
                }
            }
        }

        ime_pos
    }

    /// 탭 바를 인스턴스 버퍼에 그린다.
    /// iTerm2 스타일 탭 바: 제목만 가운데 정렬, 작은 UI 폰트, 얇은 구분선.
    /// 크롬 색은 테마 배경에서 파생한다 (활성 탭이 비활성보다 살짝 밝음).
    fn draw_tab_bar(
        &mut self,
        tab_titles: &[String],
        active_tab: usize,
        bg_instances: &mut Vec<BgInstance>,
        text_instances: &mut Vec<TextInstance>,
    ) {
        let theme = self.theme;
        let bar_h = self.tab_bar_height();
        let width = self.config.width as f32;

        let bar_bg = mix(theme.bg, [0.0; 3], 0.35); // 터미널보다 어두운 크롬
        let active_bg = mix(theme.bg, [1.0; 3], 0.10); // 활성 탭은 살짝 밝게
        let separator = mix(theme.bg, [1.0; 3], 0.22);
        let inactive_fg = mix(theme.fg, theme.bg, 0.45);

        bg_instances.push(BgInstance {
            rect: [0.0, 0.0, width, bar_h],
            color: [bar_bg[0], bar_bg[1], bar_bg[2], 1.0],
        });

        let tab_count = tab_titles.len().max(1);
        let tab_width = width / tab_count as f32;
        let label_y = (bar_h - self.ui_line_height) / 2.0;
        let ui_adv = self.ui_advance;
        let char_w = |c: char| {
            use unicode_width::UnicodeWidthChar;
            ui_adv * if c.width().unwrap_or(1) >= 2 { 2.0 } else { 1.0 }
        };

        for (i, title) in tab_titles.iter().enumerate() {
            let tab_x = i as f32 * tab_width;
            if i == active_tab && tab_count > 1 {
                bg_instances.push(BgInstance {
                    rect: [tab_x, 0.0, tab_width, bar_h],
                    color: [active_bg[0], active_bg[1], active_bg[2], 1.0],
                });
            }
            let fg = if i == active_tab { theme.fg } else { inactive_fg };

            // 제목만 표시 (번호 없음), 폭에 맞게 끝을 자르고 가운데 정렬
            let max_w = tab_width - ui_adv * 2.0;
            let mut label = String::new();
            let mut label_w = 0.0;
            let mut truncated = false;
            for ch in title.chars() {
                let w = char_w(ch);
                if label_w + w > max_w {
                    truncated = true;
                    break;
                }
                label.push(ch);
                label_w += w;
            }
            // 잘렸으면 iTerm2처럼 말줄임표 표시
            if truncated {
                let ell = '…';
                let ell_w = char_w(ell);
                while label_w + ell_w > max_w {
                    match label.pop() {
                        Some(c) => label_w -= char_w(c),
                        None => break,
                    }
                }
                label.push(ell);
                label_w += ell_w;
            }
            let mut x = tab_x + ((tab_width - label_w) / 2.0).max(ui_adv);
            for ch in label.chars() {
                if let Some(glyph) = self.ui_glyph(ch) {
                    text_instances.push(TextInstance {
                        rect: [
                            x + glyph.offset[0],
                            label_y + glyph.offset[1],
                            glyph.size[0],
                            glyph.size[1],
                        ],
                        uv: glyph.uv,
                        color: [fg[0], fg[1], fg[2], 1.0],
                    });
                }
                x += char_w(ch);
            }

            // 탭 사이 얇은 구분선
            if i > 0 {
                bg_instances.push(BgInstance {
                    rect: [tab_x, bar_h * 0.2, 1.0, bar_h * 0.6],
                    color: [separator[0], separator[1], separator[2], 1.0],
                });
            }
        }
    }

    /// 인스턴스를 업로드하고 프레임을 그린다.
    fn submit(&mut self, bg_instances: &[BgInstance], text_instances: &[TextInstance]) {
        let globals = Globals {
            screen: [self.config.width as f32, self.config.height as f32, 0.0, 0.0],
        };
        self.queue
            .write_buffer(&self.globals_buffer, 0, bytemuck::bytes_of(&globals));

        let bg_bytes: &[u8] = bytemuck::cast_slice(&bg_instances);
        if bg_bytes.len() > self.bg_capacity {
            self.bg_capacity = bg_bytes.len().next_power_of_two();
            self.bg_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("bg instances"),
                size: self.bg_capacity as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !bg_bytes.is_empty() {
            self.queue.write_buffer(&self.bg_buffer, 0, bg_bytes);
        }

        let text_bytes: &[u8] = bytemuck::cast_slice(&text_instances);
        if text_bytes.len() > self.text_capacity {
            self.text_capacity = text_bytes.len().next_power_of_two();
            self.text_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("text instances"),
                size: self.text_capacity as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !text_bytes.is_empty() {
            self.queue.write_buffer(&self.text_buffer, 0, text_bytes);
        }

        // --- 드로우 ---
        use wgpu::CurrentSurfaceTexture;
        let frame = match self.surface.get_current_texture() {
            CurrentSurfaceTexture::Success(frame) | CurrentSurfaceTexture::Suboptimal(frame) => {
                frame
            }
            CurrentSurfaceTexture::Lost | CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.config);
                match self.surface.get_current_texture() {
                    CurrentSurfaceTexture::Success(frame)
                    | CurrentSurfaceTexture::Suboptimal(frame) => frame,
                    _ => return,
                }
            }
            _ => return,
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("terminal pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // 페인 사이 틈이 구분선으로 보이도록 배경보다 어두운 색으로 클리어
                        // (탭/상태바 크롬과 같은 테마 파생색).
                        // 알파는 배경 불투명도 (투명 모드에서 데스크톱이 비쳐 보이게).
                        load: wgpu::LoadOp::Clear({
                            let chrome = mix(self.theme.bg, [0.0; 3], 0.35);
                            wgpu::Color {
                                r: chrome[0] as f64,
                                g: chrome[1] as f64,
                                b: chrome[2] as f64,
                                a: self.theme.opacity as f64,
                            }
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.globals_bind_group, &[]);
            if !bg_instances.is_empty() {
                pass.set_pipeline(&self.bg_pipeline);
                pass.set_vertex_buffer(0, self.bg_buffer.slice(..bg_bytes.len() as u64));
                pass.draw(0..6, 0..bg_instances.len() as u32);
            }
            if !text_instances.is_empty() {
                pass.set_pipeline(&self.text_pipeline);
                pass.set_bind_group(1, &self.atlas_bind_group, &[]);
                pass.set_vertex_buffer(0, self.text_buffer.slice(..text_bytes.len() as u64));
                pass.draw(0..6, 0..text_instances.len() as u32);
            }
        }
        self.queue.submit(std::iter::once(encoder.finish()));
        self.queue.present(frame);
    }
}

/// ANSI 색 → RGB. 설정 팔레트(16색)와 256색 확장을 지원한다.
fn ansi_to_rgb(color: &AnsiColor, theme: &Theme) -> [f32; 3] {
    match color {
        AnsiColor::Spec(rgb) => [
            rgb.r as f32 / 255.0,
            rgb.g as f32 / 255.0,
            rgb.b as f32 / 255.0,
        ],
        AnsiColor::Named(named) => named_color(*named, theme),
        AnsiColor::Indexed(idx) => indexed_color(*idx, theme),
    }
}

fn named_color(named: NamedColor, theme: &Theme) -> [f32; 3] {
    let p = &theme.palette;
    match named {
        NamedColor::Black | NamedColor::DimBlack => p[0],
        NamedColor::Red | NamedColor::DimRed => p[1],
        NamedColor::Green | NamedColor::DimGreen => p[2],
        NamedColor::Yellow | NamedColor::DimYellow => p[3],
        NamedColor::Blue | NamedColor::DimBlue => p[4],
        NamedColor::Magenta | NamedColor::DimMagenta => p[5],
        NamedColor::Cyan | NamedColor::DimCyan => p[6],
        NamedColor::White | NamedColor::DimWhite => p[7],
        NamedColor::BrightBlack => p[8],
        NamedColor::BrightRed => p[9],
        NamedColor::BrightGreen => p[10],
        NamedColor::BrightYellow => p[11],
        NamedColor::BrightBlue => p[12],
        NamedColor::BrightMagenta => p[13],
        NamedColor::BrightCyan => p[14],
        NamedColor::BrightWhite => p[15],
        NamedColor::Background => theme.bg,
        NamedColor::Foreground | NamedColor::BrightForeground => theme.fg,
        NamedColor::Cursor => theme.cursor,
        _ => theme.fg,
    }
}

fn indexed_color(idx: u8, theme: &Theme) -> [f32; 3] {
    match idx {
        0..=15 => theme.palette[idx as usize],
        16..=231 => {
            let i = idx as u32 - 16;
            let steps = [0u8, 95, 135, 175, 215, 255];
            let r = steps[(i / 36) as usize];
            let g = steps[((i % 36) / 6) as usize];
            let b = steps[(i % 6) as usize];
            rgb8(r, g, b)
        }
        232..=255 => {
            let v = 8 + (idx as u32 - 232) * 10;
            rgb8(v as u8, v as u8, v as u8)
        }
    }
}

fn rgb8(r: u8, g: u8, b: u8) -> [f32; 3] {
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0]
}

/// 두 색을 t(0~1)로 섞는다. UI 크롬 색을 테마에서 파생할 때 사용.
fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}
