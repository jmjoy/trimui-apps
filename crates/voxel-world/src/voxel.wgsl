// Terrain: camera-relative compact vertices with baked smooth light and AO.
struct Camera {
    matrix: mat4x4<f32>,
    inverse: mat4x4<f32>,
    eye: vec4<f32>,
    fog: vec4<f32>,
    zenith: vec4<f32>,
    horizon: vec4<f32>,
    sun: vec4<f32>,
    sun_right: vec4<f32>,
    sun_up: vec4<f32>,
    rotation: mat4x4<f32>,
}
@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var blocks: texture_2d_array<f32>;
@group(0) @binding(2) var pixels: sampler;

struct Input {
    @location(0) pos: vec4<u32>,
    @location(1) uv: vec2<i32>,
    @location(2) info: vec4<u32>,
    // Section origin relative to the camera's block (per draw).
    @location(3) origin: vec4<f32>,
}
struct Output {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) layer: u32,
    // Final light colour; computed per vertex to keep fragments cheap.
    @location(2) light: vec3<f32>,
    @location(3) relative: vec2<f32>,
}

// East, west, up, down, south, north.
const FACE_SHADE = array<f32, 6>(0.62, 0.62, 1.0, 0.5, 0.8, 0.8);
const AO = array<f32, 4>(0.42, 0.62, 0.82, 1.0);

// Light level 0–1 to brightness; roughly 0.8 per level like the original
// game, with a little ambient light so caves are never pitch black.
fn curve(level: f32) -> f32 {
    return 0.03 + 0.97 * exp2((level - 1.0) * 15.0 * 0.2863);
}

@vertex fn vs_main(input: Input) -> Output {
    var o: Output;
    let p = vec3<f32>(input.pos.xyz) / 16.0 + input.origin.xyz;
    o.clip = camera.matrix * vec4(p, 1.0);
    o.uv = vec2<f32>(input.uv) / 16.0;
    o.layer = input.info.x;
    let level = vec2<f32>(f32(input.info.y), f32(input.info.z)) / 60.0;
    let shade = FACE_SHADE[input.info.w & 7u] * AO[(input.info.w >> 3u) & 3u];
    let daylight = camera.zenith.w;
    let night = 1.0 - smoothstep(0.2, 0.7, daylight);
    let sky_tint = mix(vec3(1.0), vec3(0.62, 0.72, 1.0), night);
    let sky = curve(level.x * daylight) * sky_tint;
    let torch = curve(level.y) * vec3(1.0, 0.86, 0.66);
    o.light = max(sky, torch) * shade;
    o.relative = p.xz - camera.eye.xz;
    return o;
}

@fragment fn fs_main(input: Output) -> @location(0) vec4<f32> {
    let texel = textureSample(blocks, pixels, vec2(input.uv.x, 1.0 - input.uv.y), i32(input.layer));
    let fog = smoothstep(camera.fog.w * 0.6, camera.fog.w, length(input.relative));
    return vec4(mix(texel.rgb * input.light, camera.fog.rgb, fog), 1.0);
}
