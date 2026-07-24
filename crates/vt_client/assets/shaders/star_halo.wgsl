#import bevy_pbr::forward_io::VertexOutput

struct StarHaloMaterial {
    color_r: f32,
    color_g: f32,
    color_b: f32,
    alpha: f32,
    time: f32,
    animation_speed: f32,
    _pad0: f32,
    _pad1: f32,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var<uniform> material: StarHaloMaterial;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let uv = in.uv * 2.0 - vec2<f32>(1.0, 1.0);
    let dist = length(uv);
    if (dist > 1.0) {
        discard;
    }

    let pulse = 0.92 + 0.08 * sin(material.time * material.animation_speed * 1.2);
    let core = smoothstep(1.0, 0.08, dist);
    let corona = pow(max(1.0 - dist, 0.0), 1.7);
    let alpha = material.alpha * pulse * max(core * 0.32, corona);
    let color = vec3<f32>(material.color_r, material.color_g, material.color_b) * (1.0 + corona * 1.2);
    return vec4<f32>(color, alpha);
}
