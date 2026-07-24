#import bevy_pbr::forward_io::VertexOutput

struct StarSurfaceMaterial {
    surface_r: f32,
    surface_g: f32,
    surface_b: f32,
    _pad0: f32,
    hot_r: f32,
    hot_g: f32,
    hot_b: f32,
    time: f32,
    cell_r: f32,
    cell_g: f32,
    cell_b: f32,
    animation_speed: f32,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var<uniform> material: StarSurfaceMaterial;

fn hash3(p: vec3<f32>) -> f32 {
    let q = vec3<f32>(
        dot(p, vec3<f32>(127.1, 311.7, 74.7)),
        dot(p, vec3<f32>(269.5, 183.3, 246.1)),
        dot(p, vec3<f32>(113.5, 271.9, 124.6))
    );
    return fract(sin(q.x + q.y + q.z) * 43758.5453);
}

fn value_noise(p: vec3<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);

    let n000 = hash3(i + vec3<f32>(0.0, 0.0, 0.0));
    let n100 = hash3(i + vec3<f32>(1.0, 0.0, 0.0));
    let n010 = hash3(i + vec3<f32>(0.0, 1.0, 0.0));
    let n110 = hash3(i + vec3<f32>(1.0, 1.0, 0.0));
    let n001 = hash3(i + vec3<f32>(0.0, 0.0, 1.0));
    let n101 = hash3(i + vec3<f32>(1.0, 0.0, 1.0));
    let n011 = hash3(i + vec3<f32>(0.0, 1.0, 1.0));
    let n111 = hash3(i + vec3<f32>(1.0, 1.0, 1.0));

    let x00 = mix(n000, n100, u.x);
    let x10 = mix(n010, n110, u.x);
    let x01 = mix(n001, n101, u.x);
    let x11 = mix(n011, n111, u.x);
    let y0 = mix(x00, x10, u.y);
    let y1 = mix(x01, x11, u.y);
    return mix(y0, y1, u.z);
}

fn fbm(p: vec3<f32>) -> f32 {
    var value = 0.0;
    var amp = 0.5;
    var freq = 1.0;
    for (var i = 0; i < 4; i = i + 1) {
        value = value + amp * value_noise(p * freq);
        freq = freq * 2.03;
        amp = amp * 0.52;
    }
    return value;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let t = material.time * material.animation_speed;
    let normal = normalize(in.world_normal);
    let flow = vec3<f32>(0.0, t * 0.055, t * 0.035);
    let cells = fbm(normal * 4.2 + flow);
    let shimmer = fbm(normal * 18.0 + vec3<f32>(t * 0.12, -t * 0.08, t * 0.06));
    let hot_mask = smoothstep(0.62, 0.92, cells + shimmer * 0.18);
    let cell_mask = smoothstep(0.25, 0.72, 1.0 - cells);

    let surface = vec3<f32>(material.surface_r, material.surface_g, material.surface_b);
    let hot = vec3<f32>(material.hot_r, material.hot_g, material.hot_b);
    let cell = vec3<f32>(material.cell_r, material.cell_g, material.cell_b);

    var color = mix(surface, cell, cell_mask * 0.55);
    color = mix(color, hot, hot_mask);
    color = color * (1.15 + shimmer * 0.18);
    return vec4<f32>(color, 1.0);
}
