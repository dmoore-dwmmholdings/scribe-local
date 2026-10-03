#include <metal_stdlib>
#include <SwiftUI/SwiftUI_Metal.h>
using namespace metal;

// Shaders for the recording visuals: the ember orb and the glow that runs
// around the screen's edge. Both are SwiftUI colour effects, evaluated per
// pixel, so they stay sharp at any size where blurred shapes go muddy.

// MARK: Noise

static float hash21(float2 p) {
    p = fract(p * float2(123.34, 456.21));
    p += dot(p, p + 45.32);
    return fract(p.x * p.y);
}

static float vnoise(float2 p) {
    float2 i = floor(p), f = fract(p);
    float a = hash21(i), b = hash21(i + float2(1, 0));
    float c = hash21(i + float2(0, 1)), d = hash21(i + float2(1, 1));
    float2 u = f * f * (3.0 - 2.0 * f);
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

static float fbm(float2 p) {
    float v = 0.0, a = 0.5;
    const float2x2 m = float2x2(1.6, 1.2, -1.2, 1.6);
    for (int i = 0; i < 5; i++) {
        v += a * vnoise(p);
        p = m * p;
        a *= 0.5;
    }
    return v;
}

// MARK: Palette

// The old app's ember palette, gold core to ember pink, ending in a deep red
// for the shadows. `t` 0 is the hottest.
static half3 ember(float t) {
    const half3 stops[6] = {
        half3(1.00, 0.88, 0.62),  // gold
        half3(1.00, 0.69, 0.31),  // tangerine
        half3(1.00, 0.47, 0.21),  // orange
        half3(1.00, 0.28, 0.25),  // coral
        half3(0.96, 0.33, 0.55),  // ember pink
        half3(0.42, 0.07, 0.14),  // deep ember
    };
    float x = clamp(t, 0.0, 1.0) * 5.0;
    int i = min(int(x), 4);
    return mix(stops[i], stops[i + 1], half(x - float(i)));
}

// The same colours as a loop, for travelling around the screen's edge.
static half3 emberLoop(float t) {
    const half3 stops[5] = {
        half3(1.00, 0.80, 0.45),
        half3(1.00, 0.52, 0.22),
        half3(1.00, 0.30, 0.28),
        half3(0.98, 0.38, 0.62),
        half3(1.00, 0.55, 0.30),
    };
    float x = fract(t) * 5.0;
    int i = int(x) % 5;
    return mix(stops[i], stops[(i + 1) % 5], half(smoothstep(0.0, 1.0, x - float(i))));
}

// MARK: Orb

static float fbm3(float2 p) {
    float v = 0.0, a = 0.55;
    const float2x2 m = float2x2(1.5, 1.1, -1.1, 1.5);
    for (int i = 0; i < 3; i++) {
        v += a * vnoise(p);
        p = m * p;
        a *= 0.5;
    }
    return v;
}

/// A drop of liquid glass with light flowing inside, after the Siri orb in
/// ember colours. Not a lit ball: the outline wobbles like a liquid, the glass
/// is flat across the middle and curves only at the edge, so the strands inside
/// magnify and bend towards the rim as they would through a lens, and the edge
/// carries Liquid Glass's specular rim — bright upper left, fainter lower right
/// — over a thin dark line that gives the glass its thickness. `level` (0…1)
/// speeds the flow, stirs it, and makes the drop wobble more.
[[ stitchable ]] half4 emberOrb(float2 pos, half4 color, float2 size, float time, float level) {
    float halfSize = min(size.x, size.y) * 0.5;
    float2 uv = (pos - size * 0.5) / halfSize;
    float base = 0.70 + 0.04 * level;
    float2 q0 = uv / base;
    float ang = atan2(q0.y, q0.x);
    float2 ringPt = float2(cos(ang), sin(ang));

    // A liquid outline: the radius wobbles with slow noise around the edge.
    float wob = (vnoise(ringPt * 1.3 + float2(time * 0.35, -time * 0.25)) - 0.5) * (0.05 + 0.08 * level)
              + (vnoise(ringPt * 2.6 + float2(-time * 0.6, time * 0.4) + 9.0) - 0.5) * (0.02 + 0.04 * level);
    float R = 1.0 + wob;
    float r = length(q0) / R;      // 1 at the wobbling edge
    float px = 1.0 / (halfSize * base * R);
    float frameEdge = 1.0 / base;

    // Halo, gone before the frame's edge.
    float haloT = clamp((length(q0) - R) / max(frameEdge * 0.92 - R, 0.01), 0.0, 1.0);
    float halo = pow(1.0 - haloT, 2.8) * (0.26 + 0.40 * level);
    half3 haloColor = mix(half3(1.0, 0.45, 0.25), half3(1.0, 0.35, 0.50), half(0.5 + 0.5 * sin(time * 0.5)));
    if (r > 1.0 + px) {
        return half4(haloColor * half(halo), half(halo));
    }

    // Lens profile: flat in the middle, curving only near the edge.
    float edgeCurve = pow(clamp(r, 0.0, 1.0), 6.0);
    // Refraction: the inside is seen magnified and bent towards the rim.
    float2 q = (q0 / R) * (1.0 - 0.38 * edgeCurve);

    // Flow.
    float t = time * (0.22 + 0.7 * level);
    float2 p = q * 0.62;
    float2 w = float2(fbm3(p + float2(t * 0.55, -t * 0.4)),
                      fbm3(p + float2(-t * 0.45, t * 0.3) + 3.7));
    float field = fbm3(p * 1.3 + (1.2 + 1.3 * level) * w + float2(0.0, t * 0.2));

    // Tinted glass body, lighter than a solid ball: warm, deep, even.
    half3 col = mix(half3(0.30, 0.07, 0.07), half3(0.52, 0.15, 0.11), half(0.5 + 0.5 * w.x));

    // Strands of light: contour lines of the field, each its own ember colour.
    float k = field * 3.4 + 0.12 * t;
    float width = 0.10 + 0.05 * level;
    float strand = exp(-pow(abs(sin(3.14159 * k)) / width, 2.0));
    half3 strandColor = emberLoop(floor(k) * 0.29 + 0.1 * w.y);
    float k2 = w.y * 4.0 - 0.08 * t + 0.5;
    float back = exp(-pow(abs(sin(3.14159 * k2)) / (width * 2.4), 2.0));
    half3 backColor = emberLoop(floor(k2) * 0.37 + 0.5);
    col += backColor * half(back * (0.30 + 0.2 * level));
    col += strandColor * half(strand * (0.95 + 0.4 * level));
    col += half3(1.0, 0.92, 0.8) * half(pow(strand, 6.0) * 0.45);

    // A warm light deep in the middle.
    float centre = exp(-dot(q, q) * 2.2);
    col += half3(1.0, 0.55, 0.28) * half(centre * (0.20 + 0.28 * level));

    // The glass edge. A thin dark line just inside the rim (the glass's
    // thickness), then the rim itself catching light — bright upper left,
    // fainter lower right — as Liquid Glass does.
    float d = 1.0 - r;                                  // distance in from the edge
    float2 dir = q0 / max(length(q0), 1e-4);
    float topLeft = clamp(dot(dir, normalize(float2(-0.6, -0.8))), 0.0, 1.0);
    float bottomRight = clamp(dot(dir, normalize(float2(0.6, 0.8))), 0.0, 1.0);
    float inner = exp(-pow((d - 0.055) / 0.03, 2.0));
    col *= half(1.0 - 0.35 * inner);
    float rim = exp(-pow(d / 0.018, 2.0));
    col += half3(1.0, 0.95, 0.9) * half(rim * (0.25 + 0.85 * pow(topLeft, 1.5) + 0.35 * pow(bottomRight, 2.0)));
    // A soft edge glow all round, in the palette, where the glass is thickest.
    col += half3(1.0, 0.5, 0.55) * half(edgeCurve * 0.28);
    // A broad, soft reflection across the top.
    float2 rq = q0 / R - float2(-0.18, -0.42);
    float reflection = exp(-pow(length(rq * float2(0.8, 1.6)) / 0.42, 2.0));
    col += half3(1.0, 0.97, 0.94) * half(reflection * 0.16);

    float inside = smoothstep(1.0 + px, 1.0 - px, r);
    float alpha = mix(halo, 1.0, inside);
    return half4(min(col, half3(1.0)) * half(inside) + haloColor * half(halo * (1.0 - inside)), half(alpha));
}

// MARK: Edge glow

static float sdRoundRect(float2 p, float2 b, float r) {
    float2 q = abs(p) - b + r;
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
}

/// The glow around the screen while recording: a crisp line hugging the
/// display's corners, a bloom falling off inward, and a faint haze. Its
/// thickness ripples in waves that travel around the edge and its colour flows
/// along it; `level` thickens and brightens it.
[[ stitchable ]] half4 edgeGlow(float2 pos, half4 color, float2 size, float cornerRadius, float time, float level) {
    float2 c = size * 0.5;
    float e = -sdRoundRect(pos - c, c, cornerRadius);  // distance in from the edge
    if (e < 0.0) {
        return half4(0.0);
    }

    // Position around the perimeter, 0…1.
    float around = atan2(pos.y - c.y, pos.x - c.x) / (2.0 * M_PI_F) + 0.5;

    // Noise sampled on a circle, so it repeats seamlessly around the screen
    // (sampling `around` directly left a seam where it wraps from 1 to 0).
    float2 ring = float2(cos(around * 6.2831853), sin(around * 6.2831853));
    float w1 = vnoise(ring * 1.6 + float2(time * 0.5, -time * 0.3));
    float w2 = vnoise(ring * 3.4 + float2(-time * 0.9, 7.0 + time * 0.45));
    // Thickness ripples travel around the edge; the voice swells them.
    float thick = 0.7 + 1.1 * level + 1.8 * w1 * w1 * (0.6 + level) + 0.9 * w2 * level;

    float core = exp(-e / (1.1 * thick));
    float bloom = exp(-e / (4.5 * thick)) * (0.38 + 0.32 * level);
    float haze = exp(-e / (14.0 + 18.0 * level)) * (0.03 + 0.08 * level);

    float hue = around + time * 0.06 + 0.2 * vnoise(ring * 1.1 + float2(time * 0.35, 3.0));
    half3 col = emberLoop(hue);

    float a = clamp(core + bloom + haze, 0.0, 1.0);
    // A hot, nearly white centre on the thinnest line.
    half3 outCol = col * half(a) + half3(1.0, 0.95, 0.88) * half(core * core * 0.5);
    return half4(min(outCol, half3(1.0)), half(a));
}
