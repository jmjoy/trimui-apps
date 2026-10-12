struct Camera { matrix:mat4x4<f32>, eye:vec4<f32>, fog:vec4<f32> }
@group(0) @binding(0) var<uniform> camera:Camera;
@group(0) @binding(1) var blocks:texture_2d_array<f32>;
@group(0) @binding(2) var pixels:sampler;
struct Input {@location(0) position:vec3<f32>,@location(1) uv:vec2<f32>,@location(2) layer:u32,@location(3) shade:f32}
struct Output {@builtin(position) clip:vec4<f32>,@location(0) uv:vec2<f32>,@location(1) @interpolate(flat) layer:u32,@location(2) shade:f32,@location(3) position:vec3<f32>}
@vertex fn vs_main(input:Input)->Output {
    var o:Output;o.clip=camera.matrix*vec4(input.position,1.0);o.uv=input.uv;o.layer=input.layer;o.shade=input.shade;o.position=input.position;return o;
}
@fragment fn fs_main(input:Output)->@location(0) vec4<f32> {
    let color=textureSample(blocks,pixels,vec2(input.uv.x,1.0-input.uv.y),i32(input.layer)).rgb*input.shade;
    let distance=length(input.position.xz-camera.eye.xz);
    let fog=smoothstep(camera.fog.w*0.65,camera.fog.w,distance);
    return vec4(mix(color,camera.fog.rgb,fog),1.0);
}
