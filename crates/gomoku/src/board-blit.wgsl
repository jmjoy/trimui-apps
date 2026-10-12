@group(0) @binding(0) var board: texture_2d<f32>;

// Explicit vertices avoid the GE8300's broken vertex-index array lookup.
@vertex fn vs_main(@location(0) position: vec2<f32>) -> @builtin(position) vec4<f32> {
    return vec4(position, 0.0, 1.0);
}

@fragment fn fs_main(@builtin(position) pixel: vec4<f32>) -> @location(0) vec4<f32> {
    return textureLoad(board, vec2<i32>(pixel.xy), 0);
}
