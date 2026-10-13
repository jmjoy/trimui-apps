// Sky in three cheap pieces, all at the far plane behind the terrain:
// a gradient grid coloured per vertex, square pixel-art sun and moon, and
// star quads at night. Fragment work stays tiny on the tile-based GPU.
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
    // Rotation-only view-projection for directions at infinity.
    rotation: mat4x4<f32>,
}
@group(0) @binding(0) var<uniform> camera: Camera;

const GRID_X: u32 = 24u;
const GRID_Y: u32 = 14u;

struct Gradient {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec3<f32>,
}

fn sky_color(dir: vec3<f32>) -> vec3<f32> {
    let up = dir.y;
    var color = mix(camera.horizon.rgb, camera.zenith.rgb, smoothstep(0.0, 0.55, up));
    // Below the horizon the fog colour continues so distant terrain blends.
    color = mix(color, camera.fog.rgb, smoothstep(0.02, -0.25, up));
    // Sunrise and sunset glow on the horizon around the sun's side.
    let flat_dir = normalize(vec3(dir.x, 0.0, dir.z) + vec3(0.0, 0.0, 1e-4));
    let flat_sun = normalize(vec3(camera.sun.x, 0.0, camera.sun.z) + vec3(0.0, 0.0, 1e-4));
    let side = max(dot(flat_dir, flat_sun), 0.0);
    let low = 1.0 - smoothstep(-0.1, 0.45, up);
    color = mix(color, vec3(0.98, 0.5, 0.26), camera.sun_right.w * side * side * low * 0.85);
    let g = max(dot(dir, camera.sun.xyz), 0.0);
    let g2 = g * g;
    let g8 = g2 * g2 * g2 * g2;
    let glow = g8 * g8 * g8;
    return color + vec3(1.0, 0.75, 0.45) * glow * 0.35 * (1.0 - camera.zenith.w * 0.5);
}

@vertex fn vs_gradient(@builtin(vertex_index) i: u32) -> Gradient {
    // Two triangles per grid cell covering the whole screen.
    let cell = i / 6u;
    let corner = array<vec2<u32>, 6>(
        vec2(0u, 0u), vec2(1u, 0u), vec2(1u, 1u), vec2(0u, 0u), vec2(1u, 1u), vec2(0u, 1u)
    )[i % 6u];
    let g = vec2(cell % GRID_X, cell / GRID_X) + corner;
    let ndc = vec2(f32(g.x) / f32(GRID_X), f32(g.y) / f32(GRID_Y)) * 2.0 - 1.0;
    let far = camera.inverse * vec4(ndc, 1.0, 1.0);
    var o: Gradient;
    o.clip = vec4(ndc, 1.0, 1.0);
    o.color = sky_color(normalize(far.xyz / far.w));
    return o;
}
@fragment fn fs_gradient(input: Gradient) -> @location(0) vec4<f32> {
    return vec4(input.color, 1.0);
}

struct Disc {
    @builtin(position) clip: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) moon: u32,
}

/// A point at infinity in direction `dir`, pushed onto the far plane.
fn at_infinity(dir: vec3<f32>) -> vec4<f32> {
    let p = camera.rotation * vec4(dir, 0.0);
    return vec4(p.xy, p.w, p.w);
}

@vertex fn vs_disc(@builtin(vertex_index) i: u32, @builtin(instance_index) which: u32) -> Disc {
    let corner = array<vec2<f32>, 6>(
        vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(1.0, 1.0),
        vec2(-1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, 1.0)
    )[i];
    let moon = which == 1u;
    let axis = select(camera.sun.xyz, -camera.sun.xyz, moon);
    let right = select(camera.sun_right.xyz, -camera.sun_right.xyz, moon);
    let size = select(0.09, 0.07, moon);
    let dir = axis + (right * corner.x + camera.sun_up.xyz * corner.y) * size;
    var o: Disc;
    o.clip = at_infinity(dir);
    o.local = corner * 0.5 + 0.5;
    o.moon = u32(moon);
    return o;
}
fn hash(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2(12.9898, 78.233))) * 43758.5453);
}
@fragment fn fs_disc(input: Disc) -> @location(0) vec4<f32> {
    // 8×8 pixel-art discs: a sun with a warm rim, a moon with craters.
    let q = min(floor(input.local * 8.0), vec2(7.0));
    let edge = min(min(q.x, 7.0 - q.x), min(q.y, 7.0 - q.y));
    if input.moon == 0u {
        return vec4(select(vec3(1.0, 0.85, 0.45), vec3(1.0, 0.98, 0.82), edge >= 1.0), 1.0);
    }
    var moon = select(vec3(0.86, 0.88, 0.92), vec3(0.62, 0.64, 0.7), hash(q) > 0.78);
    moon = select(vec3(0.7, 0.72, 0.78), moon, edge >= 1.0);
    return vec4(moon, 1.0);
}

struct Star {
    @builtin(position) clip: vec4<f32>,
    @location(0) brightness: f32,
}
@vertex fn vs_star(@builtin(vertex_index) i: u32, @builtin(instance_index) n: u32) -> Star {
    let corner = array<vec2<f32>, 6>(
        vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(1.0, 1.0),
        vec2(-1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, 1.0)
    )[i];
    let seed = vec2(f32(n), 0.5);
    // Uniform direction on the sphere from two hashes.
    let z = hash(seed) * 2.0 - 1.0;
    let a = hash(seed + 17.3) * 6.2831853;
    let r = sqrt(1.0 - z * z);
    var dir = vec3(r * cos(a), z, r * sin(a));
    // The starry sky turns with the sun around the z axis.
    let turn = camera.sun.w * 6.2831853;
    let c = cos(turn);
    let s = sin(turn);
    dir = vec3(c * dir.x - s * dir.y, s * dir.x + c * dir.y, dir.z);
    let size = 0.0022 + 0.0018 * hash(seed + 5.1);
    let right = normalize(cross(dir, vec3(0.0, 1.0, 0.001)));
    let up = cross(right, dir);
    var o: Star;
    o.clip = at_infinity(dir + (right * corner.x + up * corner.y) * size);
    let twinkle = 0.65 + 0.35 * sin(camera.sun.w * 4000.0 * (0.5 + hash(seed + 9.7)));
    o.brightness = camera.horizon.w * twinkle * smoothstep(-0.05, 0.2, dir.y);
    return o;
}
@fragment fn fs_star(input: Star) -> @location(0) vec4<f32> {
    // Blended over the gradient, so faint stars fade instead of popping.
    return vec4(0.92, 0.94, 1.0, input.brightness);
}
