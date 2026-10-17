// Cell renderer for an R8 coverage glyph atlas.
//
// The atlas holds one byte of coverage per texel, and this shader reads it as
// alpha. That is all it does with it, which is why the same shader draws both
// halves of the catalogue: a low-resolution face arrives thresholded to 0 or
// 255 (`Rasterization::Binary`, antialiasing off) and a scalable one arrives as
// the rasteriser's own coverage (`Rasterization::Coverage`, antialiasing on),
// and neither the pipeline nor this code branches on which. The mode is chosen
// once, at the atlas, from the catalogue's `lowResolutionFont` flag.
//
// There is no sampler in this shader, and that is the point. A `Nearest`
// sampler gets the right answer most of the time, but "most of the time" is a
// half-texel rounding error away from a column of glyphs that is one pixel
// wrong at one scale factor and right at every other. Instead the fragment
// shader recovers its own integer pixel coordinate, subtracts the quad's
// integer origin, divides by the integer scale, and reads that texel with
// `textureLoad`. No filtering can occur because no filter is ever consulted.
// This is not the binary path's private arrangement: a scalable face resolves
// to `integer_scale` 1 (`fonts::sizing::resolve` rasterises it at the device
// pixel size it draws at), so its coverage is read one texel to one pixel and
// there is nothing for a filter to do either.
//
// The other half of the design: instances are written in *unscaled* raster
// pixels and the scale lives in a uniform. Layout therefore happens once, and
// a DPR change under a pixel face is a uniform write rather than a rebuild of
// anything. That is what makes its atlas survive a monitor change intact.

struct Uniforms {
    viewport: vec2<f32>,
    // Integer magnification. Every quad's geometry is multiplied by it, and
    // every texel lookup divided by it.
    scale: i32,
    // Top-left of the grid inside the target, in physical pixels.
    origin_x: i32,
    origin_y: i32,
    _pad0: i32,
    _pad1: i32,
    _pad2: i32,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var atlas: texture_2d<f32>;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    // Flat, because these must arrive at the fragment shader as the exact
    // integers the CPU wrote. Interpolation would reintroduce the fractions
    // this whole design exists to avoid.
    @location(0) @interpolate(flat) dst: vec2<i32>,
    @location(1) @interpolate(flat) src: vec2<i32>,
    @location(2) @interpolate(flat) size: vec2<i32>,
    @location(3) @interpolate(flat) color: vec4<f32>,
};

@vertex
fn vs(
    @builtin(vertex_index) vertex_index: u32,
    @location(0) dst: vec2<i32>,
    @location(1) size: vec2<i32>,
    @location(2) src: vec2<i32>,
    @location(3) color: vec4<f32>,
) -> VsOut {
    var corners = array<vec2<i32>, 6>(
        vec2<i32>(0, 0), vec2<i32>(1, 0), vec2<i32>(0, 1),
        vec2<i32>(1, 0), vec2<i32>(1, 1), vec2<i32>(0, 1),
    );
    let corner = corners[vertex_index];
    // Quad corners land on exact pixel boundaries: origin plus a whole number
    // of scaled texels. With the top-left fill rule this covers exactly the
    // intended pixels and no others.
    let origin = vec2<i32>(u.origin_x, u.origin_y);
    let px = origin + (dst + corner * size) * u.scale;
    let ndc = vec2<f32>(
        f32(px.x) / u.viewport.x * 2.0 - 1.0,
        1.0 - f32(px.y) / u.viewport.y * 2.0,
    );

    var out: VsOut;
    out.pos = vec4<f32>(ndc, 0.0, 1.0);
    out.dst = dst;
    out.src = src;
    out.size = size;
    out.color = color;
    return out;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    // A negative atlas origin marks a solid instance: a cell background, an
    // underline, a cursor block. It covers its quad exactly, with no texture
    // read at all.
    if (in.src.x < 0) {
        let a = in.color.a;
        return vec4<f32>(in.color.rgb * a, a);
    }

    // builtin(position) is the pixel centre, so flooring gives the pixel index.
    let pixel = vec2<i32>(floor(in.pos.xy)) - vec2<i32>(u.origin_x, u.origin_y);
    // Integer division. Both operands are non-negative inside the quad, so
    // truncation is floor and every run of `scale` pixels maps to one texel.
    let texel = in.src + (pixel / u.scale - in.dst);
    let coverage = textureLoad(atlas, texel, 0).r;
    // Premultiplied, blended with One / OneMinusSrcAlpha. For a binary atlas
    // coverage is exactly 0.0 or 1.0, so this is either "leave the destination
    // alone" or "replace it with the colour", both exact in fixed point,
    // neither capable of inventing an intermediate value. For a coverage atlas
    // it is the ordinary antialiased composite, and the intermediate values it
    // does produce are the antialiasing -- the same line of arithmetic, told
    // apart only by what is in the texture.
    let a = in.color.a * coverage;
    return vec4<f32>(in.color.rgb * a, a);
}
