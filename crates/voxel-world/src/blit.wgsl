@group(0) @binding(0) var scene:texture_2d<f32>;
@group(0) @binding(1) var scene_sampler:sampler;
struct Output {@builtin(position) position:vec4<f32>,@location(0) uv:vec2<f32>}
@vertex fn vs_main(@location(0) p:vec2<f32>)->Output {var o:Output;o.position=vec4(p,0.0,1.0);o.uv=vec2(p.x*0.5+0.5,0.5-p.y*0.5);return o;}
@fragment fn fs_main(o:Output)->@location(0) vec4<f32>{return textureSample(scene,scene_sampler,o.uv);}
