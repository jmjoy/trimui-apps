struct Scene {
    screen: vec4<f32>,
    board: vec4<f32>,
    marks: vec4<f32>,
    animation: vec4<f32>,
    winning: vec4<f32>,
    cells: array<vec4<u32>, 57>,
}
@group(0) @binding(0) var<uniform> scene: Scene;

@vertex fn vs_main(@location(0) position: vec2<f32>) -> @builtin(position) vec4<f32> {
    return vec4(position, 0.0, 1.0);
}
fn round_box(p: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let q = abs(p) - half_size + radius;
    return length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}
fn segment(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let ab = b - a;
    return length(p - a - ab * clamp(dot(p - a, ab) / max(dot(ab, ab), 0.001), 0.0, 1.0));
}
fn disk(distance: f32, radius: f32, aa: f32) -> f32 {
    return 1.0 - smoothstep(radius - aa, radius + aa, distance);
}
fn output_color(color: vec3<f32>) -> vec4<f32> {
    if scene.screen.w == 1.0 { return vec4(pow(max(color, vec3(0.0)), vec3(2.2)), 1.0); }
    return vec4(color, 1.0);
}
@fragment fn fs_main(@builtin(position) pixel: vec4<f32>) -> @location(0) vec4<f32> {
    let p = pixel.xy;
    let dpi = scene.screen.z;
    var color = mix(vec3(0.960, 0.945, 0.913), vec3(0.919, 0.904, 0.866), p.y / scene.screen.y);
    let side = scene.board.z;
    if side <= 0.0 { return output_color(color); }
    let center = scene.board.xy + side * 0.5;
    let half_size = vec2(side * 0.5);
    let distance = round_box(p - center, half_size, 10.0 * dpi);
    let shadow_distance = round_box(p - center - vec2(2.0, 8.0) * dpi, half_size, 10.0 * dpi);
    color = mix(color, vec3(0.33, 0.25, 0.14), exp(-max(shadow_distance, 0.0) / (8.0 * dpi)) * 0.22);
    if distance <= dpi {
        let local = (p - scene.board.xy) / side;
        let warp = sin(local.y * 13.0) * 0.25 + sin(local.y * 43.0) * 0.04;
        let grain = sin(local.x * 220.0 + warp * 8.0) * 0.012
            + sin(local.x * 920.0 + warp * 15.0) * 0.005
            + sin(local.x * 34.0 + local.y * 2.0) * 0.015;
        var wood = mix(vec3(0.86, 0.66, 0.39), vec3(0.91, 0.73, 0.47), local.y) + grain;
        let rim = round_box(p - center, half_size - vec2(5.0 * dpi), 7.0 * dpi);
        wood = mix(wood, vec3(0.47, 0.30, 0.15), smoothstep(-dpi, dpi, rim) * 0.7);
        color = mix(color, wood, 1.0 - smoothstep(-dpi, dpi, distance));

        let origin = scene.board.xy + side * 0.085;
        let step = side * 0.83 / 14.0;
        let g = (p - origin) / step;
        let nearest = clamp(round(g), vec2(0.0), vec2(14.0));
        let within_x = g.x >= 0.0 && g.x <= 14.0;
        let within_y = g.y >= 0.0 && g.y <= 14.0;
        let line_x = (1.0 - smoothstep(0.45 * dpi, 1.2 * dpi, abs(g.x - nearest.x) * step)) * select(0.0, 1.0, within_y);
        let line_y = (1.0 - smoothstep(0.45 * dpi, 1.2 * dpi, abs(g.y - nearest.y) * step)) * select(0.0, 1.0, within_x);
        color = mix(color, vec3(0.33, 0.23, 0.13), max(line_x, line_y) * 0.73);
        // Four corner stars at (3/11, 3/11), plus the center at (7, 7).
        // Avoid dynamic indexing of a local array on the GE8300 driver.
        let from_center = g - vec2(7.0);
        let star_distance = min(length(from_center), length(abs(from_center) - vec2(4.0)));
        color = mix(color, vec3(0.30, 0.20, 0.10), disk(star_distance * step, step * 0.075, dpi * 0.65));
        let index = u32(nearest.y) * 15u + u32(nearest.x);
        let stone = scene.cells[index / 4u][index % 4u];
        let stone_center = origin + nearest * step;
        let delta = p - stone_center;
        let is_last = f32(index) == scene.marks.x;
        let progress = clamp(scene.animation.x / 0.18, 0.0, 1.0);
        let scale = select(1.0, 1.0 - 0.22 * pow(1.0 - progress, 3.0), is_last);
        let radius = step * 0.425 * scale;
        if stone != 0u {
            let shadow = disk(length(delta - vec2(0.025, 0.05) * step), radius + step * 0.01, step * 0.04);
            color = mix(color, vec3(0.26, 0.19, 0.11), shadow * 0.38);
            let q = delta / radius;
            let z = sqrt(max(1.0 - dot(q, q), 0.0));
            let normal = vec3(q, z);
            let light = normalize(vec3(-0.5, -0.65, 1.1));
            let diffuse = max(dot(normal, light), 0.0);
            let shine = pow(max(dot(normal, normalize(light + vec3(0.0, 0.0, 1.0))), 0.0), 38.0);
            var body = vec3(0.075, 0.085, 0.085) + diffuse * 0.11 + shine * 0.24;
            if stone == 2u { body = vec3(0.71, 0.72, 0.70) + diffuse * 0.24 + shine * 0.10; }
            color = mix(color, body, disk(length(delta), radius, dpi * 0.75));
            if is_last {
                color = mix(color, select(vec3(0.26, 0.47, 0.40), vec3(0.91, 0.69, 0.35), stone == 1u), disk(length(delta), step * 0.055, dpi * 0.5));
            }
        } else {
            if f32(index) == scene.marks.z {
                color = mix(color, select(vec3(0.97), vec3(0.10, 0.13, 0.12), scene.marks.w == 1.0), disk(length(delta), radius, dpi) * 0.35);
            }
            if f32(index) == scene.marks.y {
                let ring = 1.0 - smoothstep(step * 0.024, step * 0.045, abs(length(delta) - step * 0.30));
                color = mix(color, vec3(0.16, 0.43, 0.34), ring);
                color = mix(color, vec3(0.16, 0.43, 0.34), disk(length(delta), step * 0.05, dpi * 0.6));
            }
        }
        if scene.winning.x >= 0.0 {
            let a = origin + scene.winning.xy * step;
            let b = origin + scene.winning.zw * step;
            let win = 1.0 - smoothstep(step * 0.04, step * 0.075, segment(p, a, b));
            color = mix(color, vec3(0.40, 0.76, 0.57), win * 0.93);
        }
    }
    return output_color(color);
}
