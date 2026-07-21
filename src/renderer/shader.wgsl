// 셀 배경(단색 사각형)과 글리프(atlas 샘플링) 두 파이프라인의 공용 셰이더.

struct Globals {
    // xy: 화면 크기(물리 픽셀), zw: 예약
    screen: vec4<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;

fn corner(vi: u32) -> vec2<f32> {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
    );
    return corners[vi];
}

fn to_ndc(pos: vec2<f32>) -> vec4<f32> {
    let ndc = vec2<f32>(
        pos.x / globals.screen.x * 2.0 - 1.0,
        1.0 - pos.y / globals.screen.y * 2.0,
    );
    return vec4<f32>(ndc, 0.0, 1.0);
}

// --- 배경 사각형 ---

struct BgOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_bg(
    @builtin(vertex_index) vi: u32,
    @location(0) rect: vec4<f32>,
    @location(1) color: vec4<f32>,
) -> BgOut {
    let c = corner(vi);
    var out: BgOut;
    out.pos = to_ndc(rect.xy + c * rect.zw);
    out.color = color;
    return out;
}

@fragment
fn fs_bg(in: BgOut) -> @location(0) vec4<f32> {
    return in.color;
}

// --- 글리프 ---

struct TextOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_text(
    @builtin(vertex_index) vi: u32,
    @location(0) rect: vec4<f32>,
    @location(1) uv_rect: vec4<f32>,
    @location(2) color: vec4<f32>,
) -> TextOut {
    let c = corner(vi);
    var out: TextOut;
    out.pos = to_ndc(rect.xy + c * rect.zw);
    out.uv = uv_rect.xy + c * uv_rect.zw;
    out.color = color;
    return out;
}

@group(1) @binding(0) var atlas_tex: texture_2d<f32>;
@group(1) @binding(1) var atlas_smp: sampler;

@fragment
fn fs_text(in: TextOut) -> @location(0) vec4<f32> {
    let alpha = textureSample(atlas_tex, atlas_smp, in.uv).r;
    return vec4<f32>(in.color.rgb, in.color.a * alpha);
}
