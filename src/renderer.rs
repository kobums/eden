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

use crate::session::EventProxy;

/// 창 가장자리 여백 (물리 픽셀, 스케일 적용 전).
const PADDING: f32 = 8.0;
/// 폰트 크기 (논리 픽셀).
const FONT_SIZE: f32 = 14.0;
const ATLAS_SIZE: u32 = 2048;

/// 기본 배경/전경색.
const DEFAULT_BG: [f32; 3] = [0.086, 0.086, 0.11];
const DEFAULT_FG: [f32; 3] = [0.85, 0.85, 0.87];
/// 선택 영역 배경색.
const SELECTION_BG: [f32; 3] = [0.23, 0.33, 0.48];

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

struct Atlas {
    texture: wgpu::Texture,
    glyphs: HashMap<char, Option<Glyph>>,
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

    pub cell_width: f32,
    pub cell_height: f32,
}

impl Renderer {
    pub fn new(window: Arc<Window>) -> Self {
        let scale = window.scale_factor() as f32;
        let size = window.inner_size();

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
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        // --- 폰트 ---
        let primary = FONT_CANDIDATES
            .iter()
            .find_map(|path| {
                let bytes = std::fs::read(path).ok()?;
                fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()).ok()
            })
            .expect("고정폭 폰트를 찾지 못함");
        let mut fonts = vec![primary];
        for path in FALLBACK_FONTS {
            if let Ok(bytes) = std::fs::read(path) {
                if let Ok(font) =
                    fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default())
                {
                    fonts.push(font);
                }
            }
        }
        let font_px = FONT_SIZE * scale;
        let line_metrics = fonts[0]
            .horizontal_line_metrics(font_px)
            .expect("폰트 라인 메트릭 없음");
        let ascent = line_metrics.ascent;
        let cell_height = (line_metrics.ascent - line_metrics.descent + line_metrics.line_gap)
            .ceil();
        let cell_width = fonts[0].metrics('M', font_px).advance_width.round();

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
            cell_width,
            cell_height,
        }
    }

    /// 현재 창 크기에서 그리드 크기(열, 행)를 계산한다.
    pub fn grid_size(&self, width: u32, height: u32) -> (usize, usize) {
        let cols = ((width as f32 - PADDING * 2.0) / self.cell_width).floor() as usize;
        let lines = ((height as f32 - PADDING * 2.0) / self.cell_height).floor() as usize;
        (cols.max(2), lines.max(1))
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
    }

    /// 글리프를 atlas에서 찾거나 새로 래스터라이즈해 올린다.
    fn glyph(&mut self, c: char) -> Option<Glyph> {
        if let Some(cached) = self.atlas.glyphs.get(&c) {
            return *cached;
        }

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
            let (metrics, bitmap) = font.rasterize(c, self.font_px);
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
                    self.atlas.glyphs.insert(c, None);
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
                        self.ascent - metrics.height as f32 - metrics.ymin as f32,
                    ],
                })
            }
        } else {
            None
        };

        self.atlas.glyphs.insert(c, glyph);
        glyph
    }

    /// 그리드를 그린다. IME 후보창 배치를 위해 커서의 물리 좌표(좌하단)를 돌려준다.
    pub fn draw(
        &mut self,
        term: &FairMutex<Term<EventProxy>>,
        preedit: Option<&str>,
    ) -> Option<(f64, f64)> {
        let mut bg_instances: Vec<BgInstance> = Vec::new();
        let mut text_instances: Vec<TextInstance> = Vec::new();
        let mut ime_pos = None;

        {
            let term = term.lock();
            let content = term.renderable_content();
            // 스크롤백을 위로 올렸을 때: 그리드 좌표(line)는 화면 좌표(row)와
            // display_offset만큼 어긋난다.
            let display_offset = content.display_offset as i32;
            let selection = content.selection;
            let cursor_point = content.cursor.point;
            let cursor_row = cursor_point.line.0 + display_offset;
            let visible_lines = Dimensions::screen_lines(term.grid()) as i32;
            let cursor_visible = cursor_row >= 0 && cursor_row < visible_lines;
            let cursor_x = PADDING + cursor_point.column.0 as f32 * self.cell_width;
            let cursor_y = PADDING + cursor_row as f32 * self.cell_height;
            if cursor_visible {
                ime_pos = Some((cursor_x as f64, (cursor_y + self.cell_height) as f64));
            }

            for indexed in content.display_iter {
                let row = indexed.point.line.0 + display_offset;
                if row < 0 {
                    continue;
                }
                let col = indexed.point.column.0;
                let x = PADDING + col as f32 * self.cell_width;
                let y = PADDING + row as f32 * self.cell_height;

                let flags = indexed.flags;
                if flags.contains(Flags::WIDE_CHAR_SPACER) {
                    continue;
                }

                let mut fg = ansi_to_rgb(&indexed.fg, DEFAULT_FG);
                let mut bg = ansi_to_rgb(&indexed.bg, DEFAULT_BG);
                if flags.contains(Flags::INVERSE) {
                    std::mem::swap(&mut fg, &mut bg);
                }

                let selected = selection
                    .as_ref()
                    .is_some_and(|range| range.contains(indexed.point));
                if selected {
                    bg = SELECTION_BG;
                }

                let is_cursor =
                    cursor_visible && preedit.is_none() && indexed.point == cursor_point;
                if is_cursor {
                    // 블록 커서: 배경을 전경색으로, 글자를 배경색으로 반전
                    bg = DEFAULT_FG;
                    fg = DEFAULT_BG;
                }

                if is_cursor || selected || bg != DEFAULT_BG {
                    let width_cells = if flags.contains(Flags::WIDE_CHAR) { 2.0 } else { 1.0 };
                    bg_instances.push(BgInstance {
                        rect: [x, y, self.cell_width * width_cells, self.cell_height],
                        color: [bg[0], bg[1], bg[2], 1.0],
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

            // IME 조합 중 문자열(preedit)을 커서 위치에 오버레이로 그린다.
            if let (Some(text), true) = (preedit, cursor_visible) {
                let mut x = cursor_x;
                for ch in text.chars() {
                    use unicode_width::UnicodeWidthChar;
                    let cells = if ch.width().unwrap_or(1) >= 2 { 2.0 } else { 1.0 };
                    let w = self.cell_width * cells;
                    bg_instances.push(BgInstance {
                        rect: [x, cursor_y, w, self.cell_height],
                        color: [DEFAULT_FG[0], DEFAULT_FG[1], DEFAULT_FG[2], 1.0],
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
                            color: [DEFAULT_BG[0], DEFAULT_BG[1], DEFAULT_BG[2], 1.0],
                        });
                    }
                    x += w;
                }
            }
        }

        // --- 업로드 ---
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
                    _ => return ime_pos,
                }
            }
            _ => return ime_pos,
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
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: DEFAULT_BG[0] as f64,
                            g: DEFAULT_BG[1] as f64,
                            b: DEFAULT_BG[2] as f64,
                            a: 1.0,
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
        ime_pos
    }
}

/// ANSI 색 → RGB. 16색 팔레트와 256색 확장을 지원한다.
fn ansi_to_rgb(color: &AnsiColor, default: [f32; 3]) -> [f32; 3] {
    match color {
        AnsiColor::Spec(rgb) => [
            rgb.r as f32 / 255.0,
            rgb.g as f32 / 255.0,
            rgb.b as f32 / 255.0,
        ],
        AnsiColor::Named(named) => named_color(*named, default),
        AnsiColor::Indexed(idx) => indexed_color(*idx, default),
    }
}

fn named_color(named: NamedColor, default: [f32; 3]) -> [f32; 3] {
    match named {
        NamedColor::Black | NamedColor::DimBlack => rgb8(0x2e, 0x2e, 0x3e),
        NamedColor::Red | NamedColor::DimRed => rgb8(0xf3, 0x8b, 0xa8),
        NamedColor::Green | NamedColor::DimGreen => rgb8(0xa6, 0xe3, 0xa1),
        NamedColor::Yellow | NamedColor::DimYellow => rgb8(0xf9, 0xe2, 0xaf),
        NamedColor::Blue | NamedColor::DimBlue => rgb8(0x89, 0xb4, 0xfa),
        NamedColor::Magenta | NamedColor::DimMagenta => rgb8(0xcb, 0xa6, 0xf7),
        NamedColor::Cyan | NamedColor::DimCyan => rgb8(0x94, 0xe2, 0xd5),
        NamedColor::White | NamedColor::DimWhite => rgb8(0xba, 0xc2, 0xde),
        NamedColor::BrightBlack => rgb8(0x58, 0x5b, 0x70),
        NamedColor::BrightRed => rgb8(0xf3, 0x8b, 0xa8),
        NamedColor::BrightGreen => rgb8(0xa6, 0xe3, 0xa1),
        NamedColor::BrightYellow => rgb8(0xf9, 0xe2, 0xaf),
        NamedColor::BrightBlue => rgb8(0x89, 0xb4, 0xfa),
        NamedColor::BrightMagenta => rgb8(0xcb, 0xa6, 0xf7),
        NamedColor::BrightCyan => rgb8(0x94, 0xe2, 0xd5),
        NamedColor::BrightWhite => rgb8(0xff, 0xff, 0xff),
        NamedColor::Background => DEFAULT_BG,
        NamedColor::Foreground | NamedColor::BrightForeground | NamedColor::Cursor => DEFAULT_FG,
        _ => default,
    }
}

fn indexed_color(idx: u8, default: [f32; 3]) -> [f32; 3] {
    match idx {
        0..=7 => named_color(
            match idx {
                0 => NamedColor::Black,
                1 => NamedColor::Red,
                2 => NamedColor::Green,
                3 => NamedColor::Yellow,
                4 => NamedColor::Blue,
                5 => NamedColor::Magenta,
                6 => NamedColor::Cyan,
                _ => NamedColor::White,
            },
            default,
        ),
        8..=15 => named_color(
            match idx {
                8 => NamedColor::BrightBlack,
                9 => NamedColor::BrightRed,
                10 => NamedColor::BrightGreen,
                11 => NamedColor::BrightYellow,
                12 => NamedColor::BrightBlue,
                13 => NamedColor::BrightMagenta,
                14 => NamedColor::BrightCyan,
                _ => NamedColor::BrightWhite,
            },
            default,
        ),
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
