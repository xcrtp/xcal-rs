// SPDX-FileCopyrightText: 2026 Academa, Inc.
// SPDX-FileCopyrightText: 2021 The Android Open Source Project
// SPDX-License-Identifier: Apache-2.0
// Modified by Academa, Inc.

// A 3D view's ambient occlusion (`occlusion.rs`): scalable ambient obscurance, over the view's opaque depth: as
// distances along the view (`linearize`), a pyramid of them (`depth_mip`), the occlusion (`sao`), and its low-pass,
// which stops where depth jumps (`blur_h`, `blur_v`).

// The view: its half extent at unit distance (x, y), its near plane's distance, its pyramid's coarsest level; the
// occlusion's radius, twice its power, its intensity's share a tap, its bias; its peak squared, one over its radius
// squared, the view's pixels a unit at unit distance across it (y); the spiral's start, its turn a tap, the taps, one
// over the depth within which the low-pass takes a neighbour as the same surface.
struct Params {
    proj: vec4<f32>,
    ao0: vec4<f32>,
    ao1: vec4<f32>,
    ao2: vec4<f32>,
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var depth: texture_depth_2d;      // the view's opaque depth (reversed Z: near / distance; 0: none)
@group(0) @binding(3) var depth0: texture_2d<f32>;      // distances along the view (0: nothing there)
@group(0) @binding(4) var depth_mips: texture_2d<f32>;  // the pyramid: its level l + 1 in mip l
@group(0) @binding(5) var ao_src: texture_2d<f32>;      // the occlusion before its low-pass
@group(0) @binding(6) var out: texture_storage_2d<r32float, write>;
@group(0) @binding(7) var finer: texture_2d<f32>;       // the pyramid's level before the one written

// ── the view's distances ────────────────────────────────────────────────────────────────────

@compute @workgroup_size(8, 8)
fn linearize(@builtin(global_invocation_id) id: vec3<u32>) {
    if (any(id.xy >= textureDimensions(depth))) {
        return;
    }
    let d = textureLoad(depth, id.xy, 0);
    textureStore(out, id.xy, vec4<f32>(select(0.0, params.proj.z / d, d > 0.0)));
}

// ── the pyramid: each texel one of its four children, on a rotated grid (no averaging: every level holds real
//    surfaces' distances) ─────────────────────────────────────────────────────────────────────────

@compute @workgroup_size(8, 8)
fn depth_mip(@builtin(global_invocation_id) id: vec3<u32>) {
    if (any(id.xy >= textureDimensions(out))) {
        return;
    }
    let c = vec2<i32>(id.xy);
    let child = clamp(2 * c + vec2<i32>(c.y & 1, c.x & 1), vec2<i32>(0), vec2<i32>(textureDimensions(finer)) - 1);
    textureStore(out, c, vec4<f32>(textureLoad(finer, child, 0).x));
}

// ── the occlusion ─────────────────────────────────────────────────────────────────────────────

// Where a pixel's spiral starts, as a share of a turn and of a tap's step out: one of 16, interleaved 4 x 4 in Bayer's
// order, so the low-pass (as wide as the interleave) averages every start in every neighbourhood: 16 taps look as
// smooth as 32 interleaved-gradient ones (Jimenez 2014), which streak at 16 (experiments/ambient-occlusion).
const STARTS = array<f32, 16>(0.0, 8.0, 2.0, 10.0, 12.0, 4.0, 14.0, 6.0, 3.0, 11.0, 1.0, 9.0, 15.0, 7.0, 13.0, 5.0);

fn start(p: vec2<u32>) -> f32 {
    let b = p & vec2<u32>(3u);
    return (STARTS[b.y * 4u + b.x] + 0.5) / 16.0;
}

fn sq(x: f32) -> f32 {
    return x * x;
}

fn depth_texel(p: vec2<i32>) -> f32 {
    return textureLoad(depth0, clamp(p, vec2<i32>(0), vec2<i32>(textureDimensions(depth0)) - 1), 0).x;
}

// A tap at uv on the pyramid's level `level`: the distance its texel there holds (its edge held beyond it), and where
// that distance was seen — the view's own pixel it is a copy of (the child rule, down to level 0), at its centre. A
// tap is a real point of a surface, where it is: the distance placed at uv itself would lift a tap off a slanted plane
// by the depth's change over the texel's offset, and a flat floor seen at a slant would shut out its own light.
fn tap(uv: vec2<f32>, level: u32) -> vec3<f32> {
    let size0 = vec2<i32>(textureDimensions(depth0));
    if (level == 0u) {
        let c = clamp(vec2<i32>(floor(uv * vec2<f32>(size0))), vec2<i32>(0), size0 - 1);
        return vec3<f32>((vec2<f32>(c) + 0.5) / vec2<f32>(size0), textureLoad(depth0, c, 0).x);
    }
    let size = vec2<i32>(textureDimensions(depth_mips, level - 1u));
    var c = clamp(vec2<i32>(floor(uv * vec2<f32>(size))), vec2<i32>(0), size - 1);
    let z = textureLoad(depth_mips, c, i32(level) - 1).x;
    for (var k = i32(level) - 1; k >= 0; k--) {
        let finer = max(size0 >> vec2<u32>(u32(k)), vec2<i32>(1));
        c = clamp(2 * c + vec2<i32>(c.y & 1, c.x & 1), vec2<i32>(0), finer - 1);
    }
    return vec3<f32>((vec2<f32>(c) + 0.5) / vec2<f32>(size0), z);
}

// Where the view sees at uv (y down) and distance z: x right, y up, z toward the viewer.
fn view_pos(uv: vec2<f32>, z: f32) -> vec3<f32> {
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - 2.0 * uv.y);
    return vec3<f32>(ndc.x * params.proj.x * z, ndc.y * params.proj.y * z, -z);
}

fn inv_depth(z: f32) -> f32 {
    return 1.0 / max(z, 1e-6);
}

// How much of the light from all around reaches the surface pixel `id` shows (1: all of it; nothing there: all).
fn visibility(id: vec2<u32>) -> f32 {
    let p = vec2<i32>(id);
    let texel = 1.0 / vec2<f32>(textureDimensions(depth0));
    let frag = vec2<f32>(id) + 0.5;
    let uv = frag * texel;
    let z = depth_texel(p);
    if (z <= 0.0) {
        return 1.0;
    }
    let origin = view_pos(uv, z);
    // its normal: per axis, the difference toward the neighbour that continues its surface (1 / z is affine across a
    // plane: the one whose next neighbour it extrapolates best)
    let zl = depth_texel(p - vec2<i32>(1, 0));
    let zr = depth_texel(p + vec2<i32>(1, 0));
    let hx = abs(vec2<f32>(2.0 * inv_depth(zl) - inv_depth(depth_texel(p - vec2<i32>(2, 0))), 2.0 * inv_depth(zr) - inv_depth(depth_texel(p + vec2<i32>(2, 0)))) - inv_depth(z));
    let dpdx = select(view_pos(uv + vec2<f32>(texel.x, 0.0), zr) - origin, origin - view_pos(uv - vec2<f32>(texel.x, 0.0), zl), hx.x < hx.y);
    let zu = depth_texel(p - vec2<i32>(0, 1)); // the row above
    let zd = depth_texel(p + vec2<i32>(0, 1)); // the row below
    let hy = abs(vec2<f32>(2.0 * inv_depth(zd) - inv_depth(depth_texel(p + vec2<i32>(0, 2))), 2.0 * inv_depth(zu) - inv_depth(depth_texel(p - vec2<i32>(0, 2)))) - inv_depth(z));
    let dpdy = select(view_pos(uv - vec2<f32>(0.0, texel.y), zu) - origin, origin - view_pos(uv + vec2<f32>(0.0, texel.y), zd), hy.x < hy.y);
    let n = normalize(cross(dpdx, dpdy));
    // the taps: a spiral out to the radius as the view sees it there, each farther tap read from a coarser level
    let noise = start(id);
    let a0 = 6.2831853 * 2.4 * noise + params.ao2.x;
    var dir = vec2<f32>(cos(a0), sin(a0));
    let ci = cos(params.ao2.y);
    let si = sin(params.ao2.y);
    let samples = params.ao2.z;
    let disk = params.ao1.z * params.ao0.x / z;
    var occluded = 0.0;
    for (var i = 0.0; i < samples; i += 1.0) {
        let t = sq((i + noise + 0.5) / (samples - 0.5));
        let r = max(1.0, t * disk);
        // (the view's y grows downward: the tap mirrored, the spiral turns as it would with y up)
        let q = uv + r * vec2<f32>(dir.x, -dir.y) * texel;
        dir = vec2<f32>(dir.x * ci - dir.y * si, dir.x * si + dir.y * ci);
        let level = u32(clamp(floor(log2(r)) - 3.0, 0.0, params.proj.w));
        let s = tap(q, level);
        if (s.z <= 0.0) {
            continue; // nothing there
        }
        let v = view_pos(s.xy, s.z) - origin;
        let vv = dot(v, v);
        let vn = dot(v, n);
        let w = sq(max(0.0, 1.0 - vv * params.ao1.y));
        occluded += w * max(0.0, vn + origin.z * params.ao0.w) / (vv + params.ao1.x);
    }
    occluded = sqrt(occluded * params.ao0.z);
    return pow(clamp(1.0 - occluded, 0.0, 1.0), params.ao0.y);
}

@compute @workgroup_size(8, 8)
fn sao(@builtin(global_invocation_id) id: vec3<u32>) {
    if (any(id.xy >= textureDimensions(depth0))) {
        return;
    }
    textureStore(out, id.xy, vec4<f32>(visibility(id.xy)));
}

// ── its low-pass: a Gaussian (sigma 2.5 pixels) over the neighbours on the pixel's own surface — each weighted by
//    how far its distance is from the surface's plane through the pixel, 1 - (off / threshold)^2, so a slope keeps its
//    neighbours and no occlusion bleeds across a jump in depth. Separable, one pass across (`blur_h`) and one down
//    (`blur_v`), each reading its neighbours from the distances and the pass before: no workgroup memory, whose reads
//    after a barrier some mobile drivers return scrambled, banding the result every tile row ─────────

const APRON: i32 = 5; // how far a pixel's neighbours reach

// Along a line of distances with z0 at the centre and zm, zp either side: d(1 / z) a step on the pixel's surface,
// the side's that changes least (the other may be across an edge; none where neither is a surface).
fn slope(z0: f32, zm: f32, zp: f32) -> f32 {
    let m = select(1e30, 1.0 / z0 - 1.0 / zm, zm > 0.0);
    let p = select(1e30, 1.0 / zp - 1.0 / z0, zp > 0.0);
    let g = select(p, m, abs(m) < abs(p));
    return select(g, 0.0, abs(g) >= 1e30);
}

// A neighbour i steps away at distance z, its Gaussian weight g, seen from a pixel at z0 whose surface's 1 / z changes
// by s a step: its weight (nothing there: none).
fn weight(z0: f32, s: f32, i: f32, z: f32, g: f32) -> f32 {
    let off = (z - 1.0 / max(1.0 / z0 + i * s, 1e-12)) * params.ao2.w;
    return select(0.0, g * max(0.0, 1.0 - off * off), z > 0.0);
}

// The Gaussian's weight i steps out (exp(-i^2 / 12.5)).
fn gauss(i: i32) -> f32 {
    var g = 0.92311635;
    switch i {
        case 2: { g = 0.72614904; }
        case 3: { g = 0.48675226; }
        case 4: { g = 0.27803730; }
        case 5: { g = 0.13533528; }
        default: {}
    }
    return g;
}

fn ao_texel(p: vec2<i32>) -> f32 {
    return textureLoad(ao_src, clamp(p, vec2<i32>(0), vec2<i32>(textureDimensions(ao_src)) - 1), 0).x;
}

// The pixel p low-passed along `step`: its own value and its neighbours' either side, weighted by the Gaussian and by
// how far each lies from the surface's plane.
fn blurred(z0: f32, p: vec2<i32>, step: vec2<i32>) -> f32 {
    var sum = ao_texel(p);
    if (z0 <= 0.0) {
        return sum;
    }
    let s = slope(z0, depth_texel(p - step), depth_texel(p + step));
    var total = 1.0;
    for (var i = 1; i <= APRON; i++) {
        let g = gauss(i);
        let near = p + step * i;
        let far = p - step * i;
        let wf = weight(z0, s, -f32(i), depth_texel(far), g);
        let wn = weight(z0, s, f32(i), depth_texel(near), g);
        sum += ao_texel(far) * wf + ao_texel(near) * wn;
        total += wf + wn;
    }
    return sum / total;
}

@compute @workgroup_size(16, 16)
fn blur_h(@builtin(global_invocation_id) id: vec3<u32>) {
    let p = vec2<i32>(id.xy);
    if (any(p >= vec2<i32>(textureDimensions(out)))) {
        return;
    }
    textureStore(out, p, vec4<f32>(blurred(depth_texel(p), p, vec2<i32>(1, 0))));
}

@compute @workgroup_size(16, 16)
fn blur_v(@builtin(global_invocation_id) id: vec3<u32>) {
    let p = vec2<i32>(id.xy);
    if (any(p >= vec2<i32>(textureDimensions(out)))) {
        return;
    }
    textureStore(out, p, vec4<f32>(blurred(depth_texel(p), p, vec2<i32>(0, 1))));
}
