//! wgpu 기반 렌더러: glyph atlas에 글리프를 캐싱하고,
//! 셀 배경 → 글리프 순서의 인스턴스 드로우 2패스로 그리드를 그린다.
//!
//! - [`text`] 글리프 아틀라스와 텍스트 한 줄 그리기
//! - [`pane`] 터미널 그리드 (셀·커서·블록 거터·IME preedit)
//! - [`chrome`] 탭 바·상태바·AI 바·팔레트
//! - [`color`] 테마와 ANSI 색 변환

mod chrome;
mod color;
mod pane;
mod text;

use std::sync::Arc;

use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::Term;
use bytemuck::{Pod, Zeroable};
use winit::window::Window;

use crate::app::search::AbsMatch;
use crate::layout::Rect;
use crate::session::{Block, EventProxy};
use color::Theme;
use text::Atlas;

/// 한 프레임을 그리는 데 필요한 전부.
///
/// 위치 인자로 넘기던 것을 구조체로 묶었다 — 오버레이가 늘어날 때마다
/// 인자가 하나씩 붙으면서 호출부에서 무엇이 무엇인지 읽기 어려워졌다.
pub struct DrawParams<'a> {
    pub panes: &'a [PaneView<'a>],
    pub preedit: Option<&'a str>,
    pub tab_titles: &'a [String],
    pub active_tab: usize,
    /// AI 입력 바 한 줄 (Cmd+K).
    pub ai_bar: Option<&'a str>,
    /// 검색 바: (입력 줄, `3/17` 같은 상태) (Cmd+F).
    pub search: Option<(&'a str, &'a str)>,
    /// 커맨드 팔레트: (쿼리, 항목, 선택 인덱스) (Cmd+Shift+P).
    pub palette: Option<(&'a str, &'a [String], usize)>,
    /// 하단 상태바: (왼쪽, 오른쪽).
    pub status: Option<(&'a str, &'a str)>,
}

/// 한 페인을 그리는 데 필요한 정보.
pub struct PaneView<'a> {
    pub term: &'a FairMutex<Term<EventProxy>>,
    pub blocks: &'a [Block],
    pub rect: Rect,
    pub focused: bool,
    /// 검색 매치 (절대 줄 번호). 검색이 닫혀 있으면 빈 슬라이스.
    pub matches: &'a [AbsMatch],
    /// `matches` 안에서 현재 선택된 매치의 인덱스.
    pub current_match: Option<usize>,
}

/// 페인 가장자리 여백 (물리 픽셀). 왼쪽 여백은 블록 상태 바 거터로도 쓴다.
///
/// 마우스 좌표를 셀로 환산할 때도 같은 값이 필요하므로 크레이트에 공개한다
/// (예전에는 `app::mouse`가 같은 값을 따로 들고 있어 어긋날 수 있었다).
pub(crate) const PADDING: f32 = 8.0;

/// macOS 시스템 고정폭 폰트 후보 (앞에서부터 시도, 첫 성공이 주 폰트).
/// Nerd Font(파워라인 프롬프트 글리프 포함)가 설치돼 있으면 우선한다.
const FONT_CANDIDATES: &[&str] = &[
    "~/Library/Fonts/MesloLGS NF Regular.ttf",
    "/Library/Fonts/MesloLGS NF Regular.ttf",
    "/System/Library/Fonts/Menlo.ttc",
    "/System/Library/Fonts/Monaco.ttf",
    "/Library/Fonts/SF-Mono-Regular.otf",
];

/// PUA(파워라인·Nerd Font 아이콘) 글리프 폴백.
/// 주 폰트가 Nerd Font가 아닐 때(예: font-path로 Menlo 지정) 여기서 찾는다.
const NERD_FALLBACK_FONTS: &[&str] = &[
    "~/Library/Fonts/MesloLGS NF Regular.ttf",
    "/Library/Fonts/MesloLGS NF Regular.ttf",
];

/// 주 폰트에 없는 글리프(한글 등)를 위한 폴백 폰트.
const FALLBACK_FONTS: &[&str] = &[
    "/System/Library/Fonts/AppleSDGothicNeo.ttc",
    "/System/Library/Fonts/Apple Symbols.ttf",
];

/// 경로 맨 앞의 `~/`를 홈 디렉터리로 푼다.
fn expand_home(path: &str) -> String {
    match (path.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(home)) => format!("{home}/{rest}"),
        _ => path.to_string(),
    }
}

/// UI 크롬 폰트는 본문보다 작다 (iTerm2 탭 텍스트).
const UI_FONT_SCALE: f32 = 0.8;

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
    /// fonts와 인덱스 대응: PUA(파워라인 등) 글리프를 이 폰트에서 찾아도 되는가.
    pua_ok: Vec<bool>,
    /// 설정 파일의 논리 폰트 크기(pt). 모니터 배율 변경 시 재계산의 기준.
    base_font_size: f32,
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
        let theme = Theme::from_config(config);
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
            let bytes = std::fs::read(expand_home(path)).ok()?;
            fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()).ok()
        };
        let mut primary_path = None;
        let primary = cfg_font_path
            .as_deref()
            .into_iter()
            .chain(FONT_CANDIDATES.iter().copied())
            .find_map(|p| {
                let font = load_font(p)?;
                primary_path = Some(expand_home(p));
                Some(font)
            })
            .expect("고정폭 폰트를 찾지 못함");
        let mut fonts = vec![primary];
        // PUA 글리프를 믿고 찾아도 되는 폰트 표시 (fonts와 인덱스 대응).
        // 일반 폴백(한글 등)은 PUA에 엉뚱한 글리프를 돌려줄 수 있어 제외한다.
        let mut pua_ok = vec![true];
        for path in NERD_FALLBACK_FONTS {
            if primary_path.as_deref() == Some(expand_home(path).as_str()) {
                continue; // 주 폰트와 동일 파일이면 중복 로드하지 않는다
            }
            if let Some(font) = load_font(path) {
                fonts.push(font);
                pua_ok.push(true);
                break; // Regular 하나면 충분
            }
        }
        for path in FALLBACK_FONTS {
            if let Some(font) = load_font(path) {
                fonts.push(font);
                pua_ok.push(false);
            }
        }
        let font_px = cfg_font_size * scale;
        let line_metrics = fonts[0]
            .horizontal_line_metrics(font_px)
            .expect("폰트 라인 메트릭 없음");
        let ascent = line_metrics.ascent;
        let cell_height =
            (line_metrics.ascent - line_metrics.descent + line_metrics.line_gap).ceil();
        let cell_width = fonts[0].metrics('M', font_px).advance_width.round();

        // UI 크롬(탭/상태바)용 작은 폰트 — iTerm2 탭 텍스트처럼 본문보다 작게
        let ui_px = (font_px * UI_FONT_SCALE).round();
        let ui_metrics = fonts[0]
            .horizontal_line_metrics(ui_px)
            .expect("폰트 라인 메트릭 없음");
        let ui_ascent = ui_metrics.ascent;
        let ui_line_height = (ui_metrics.ascent - ui_metrics.descent + ui_metrics.line_gap).ceil();
        let ui_advance = fonts[0].metrics('M', ui_px).advance_width;

        let atlas = Atlas::new(&device);

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
            pua_ok,
            base_font_size: cfg_font_size,
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

    pub fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
    }

    /// 모니터 배율(scale factor)이 바뀌면 폰트 픽셀 크기와 셀 메트릭을
    /// 다시 계산하고 glyph atlas를 비운다. 배율이 다른 모니터로 창을
    /// 옮겼을 때 글자가 커지거나 작아진 채로 남는 문제를 막는다.
    pub fn set_scale_factor(&mut self, scale: f32) {
        let font_px = self.base_font_size * scale;
        if (font_px - self.font_px).abs() < f32::EPSILON {
            return;
        }
        self.font_px = font_px;
        let line_metrics = self.fonts[0]
            .horizontal_line_metrics(font_px)
            .expect("폰트 라인 메트릭 없음");
        self.ascent = line_metrics.ascent;
        self.cell_height =
            (line_metrics.ascent - line_metrics.descent + line_metrics.line_gap).ceil();
        self.cell_width = self.fonts[0].metrics('M', font_px).advance_width.round();

        self.ui_px = (font_px * UI_FONT_SCALE).round();
        let ui_metrics = self.fonts[0]
            .horizontal_line_metrics(self.ui_px)
            .expect("폰트 라인 메트릭 없음");
        self.ui_ascent = ui_metrics.ascent;
        self.ui_line_height = (ui_metrics.ascent - ui_metrics.descent + ui_metrics.line_gap).ceil();
        self.ui_advance = self.fonts[0].metrics('M', self.ui_px).advance_width;

        // 기존 글리프는 이전 배율로 래스터라이즈됐으므로 캐시를 비워
        // 다음 프레임부터 새 크기로 다시 올린다.
        self.atlas.reset();
    }

    /// 모든 페인과 크롬(탭 바·상태바·오버레이)을 그린다.
    /// IME 후보창 배치를 위해 포커스된 페인의 커서 물리 좌표(좌하단)를 돌려준다.
    pub fn draw(&mut self, params: DrawParams) -> Option<(f64, f64)> {
        let DrawParams {
            panes,
            preedit,
            tab_titles,
            active_tab,
            ai_bar,
            search,
            palette,
            status,
        } = params;

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

        self.draw_tab_bar(
            tab_titles,
            active_tab,
            &mut bg_instances,
            &mut text_instances,
        );
        // AI 바·검색 바·팔레트는 오버레이이므로 마지막에 (페인 위에) 그린다.
        if let Some(line) = ai_bar {
            ime_pos = Some(self.draw_ai_bar(line, &mut bg_instances, &mut text_instances));
        }
        if let Some((line, status)) = search {
            ime_pos =
                Some(self.draw_search_bar(line, status, &mut bg_instances, &mut text_instances));
        }
        if let Some((query, items, selected)) = palette {
            self.draw_palette(
                query,
                items,
                selected,
                &mut bg_instances,
                &mut text_instances,
            );
        }
        self.submit(&bg_instances, &text_instances);
        ime_pos
    }

    /// 인스턴스 버퍼가 모자라면 2의 거듭제곱으로 키운다.
    fn upload_instances(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        buffer: &mut wgpu::Buffer,
        capacity: &mut usize,
        label: &'static str,
        bytes: &[u8],
    ) {
        if bytes.len() > *capacity {
            *capacity = bytes.len().next_power_of_two();
            *buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: *capacity as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !bytes.is_empty() {
            queue.write_buffer(buffer, 0, bytes);
        }
    }

    /// 인스턴스를 업로드하고 프레임을 그린다.
    fn submit(&mut self, bg_instances: &[BgInstance], text_instances: &[TextInstance]) {
        let globals = Globals {
            screen: [
                self.config.width as f32,
                self.config.height as f32,
                0.0,
                0.0,
            ],
        };
        self.queue
            .write_buffer(&self.globals_buffer, 0, bytemuck::bytes_of(&globals));

        let bg_bytes: &[u8] = bytemuck::cast_slice(bg_instances);
        Self::upload_instances(
            &self.device,
            &self.queue,
            &mut self.bg_buffer,
            &mut self.bg_capacity,
            "bg instances",
            bg_bytes,
        );
        let text_bytes: &[u8] = bytemuck::cast_slice(text_instances);
        Self::upload_instances(
            &self.device,
            &self.queue,
            &mut self.text_buffer,
            &mut self.text_capacity,
            "text instances",
            text_bytes,
        );

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
            // 페인 사이 틈이 구분선으로 보이도록 크롬 색으로 클리어한다.
            // 알파는 배경 불투명도 (투명 모드에서 데스크톱이 비쳐 보이게).
            let chrome = self.theme.chrome();
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("terminal pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: chrome[0] as f64,
                            g: chrome[1] as f64,
                            b: chrome[2] as f64,
                            a: self.theme.opacity as f64,
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
