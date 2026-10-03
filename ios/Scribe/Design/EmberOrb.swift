import SwiftUI

/// The ember palette from the old app — gold core, tangerine, coral, ember pink
/// — shared by the orb and the edge glow so the app has one fire.
enum Ember {
    static let gold = Color(red: 1, green: 214 / 255, blue: 140 / 255)
    static let tangerine = Color(red: 1, green: 176 / 255, blue: 80 / 255)
    static let orange = Color(red: 1, green: 120 / 255, blue: 54 / 255)
    static let coral = Color(red: 1, green: 72 / 255, blue: 64 / 255)
    static let pink = Color(red: 1, green: 98 / 255, blue: 150 / 255)
    static let all = [gold, tangerine, orange, coral, pink]
}

/// A living sphere of fire, after the iOS Siri orb but in ember colours.
///
/// A 3×3 mesh gradient whose inner points drift on slow, out-of-phase orbits,
/// so the colours swirl rather than rotate. Voice level swells it and speeds the
/// swirl; at rest it breathes. A glass highlight sits on top.
struct EmberOrb: View {
    /// 0…1 input level; 0 when idle.
    var level: Float
    var active: Bool
    var reduceMotion = false

    @State private var smoothed: Double = 0

    var body: some View {
        TimelineView(.animation(paused: reduceMotion)) { context in
            let t = context.date.timeIntervalSinceReferenceDate
            let speed = active ? 0.9 + smoothed * 1.6 : 0.35
            let phase = t * speed
            let breathe = active ? 1 + smoothed * 0.16 : 1 + 0.025 * sin(t * 1.4)

            ZStack {
                // Outer bloom.
                Circle()
                    .fill(RadialGradient(colors: [Ember.orange.opacity(active ? 0.55 : 0.3), .clear],
                                         center: .center, startRadius: 10, endRadius: 170))
                    .scaleEffect(1.35 * breathe)
                    .blur(radius: 24)

                MeshGradient(
                    width: 3, height: 3,
                    points: meshPoints(phase),
                    colors: [
                        Ember.coral, Ember.orange, Ember.pink,
                        Ember.tangerine, Ember.gold, Ember.coral,
                        Ember.pink, Ember.orange, Ember.tangerine,
                    ]
                )
                .clipShape(Circle())
                .overlay {
                    // Glass: a soft specular cap and a rim.
                    Circle().fill(LinearGradient(colors: [.white.opacity(0.45), .white.opacity(0.0)],
                                                 startPoint: .topLeading, endPoint: .center))
                        .padding(10)
                        .blur(radius: 6)
                        .blendMode(.screen)
                }
                .overlay(Circle().stroke(.white.opacity(0.18), lineWidth: 1))
                .scaleEffect(breathe)
                .shadow(color: Ember.coral.opacity(0.5), radius: active ? 30 : 16)
            }
        }
        .onChange(of: level) { _, new in
            withAnimation(.easeOut(duration: 0.15)) { smoothed = Double(new) }
        }
        .accessibilityHidden(true)
    }

    /// Corners fixed, the inner points drifting on separate orbits.
    private func meshPoints(_ p: Double) -> [SIMD2<Float>] {
        func wobble(_ base: Float, _ amp: Float, _ f: Double, _ off: Double) -> Float {
            base + amp * Float(sin(p * f + off))
        }
        let a: Float = active ? 0.16 : 0.09
        return [
            [0, 0], [wobble(0.5, a, 0.9, 0), 0], [1, 0],
            [0, wobble(0.5, a, 1.1, 1)],
            [wobble(0.5, a * 1.3, 1.3, 2), wobble(0.5, a * 1.3, 0.8, 3)],
            [1, wobble(0.5, a, 0.7, 4)],
            [0, 1], [wobble(0.5, a, 1.2, 5), 1], [1, 1],
        ]
    }
}

/// The glow that runs around the screen's edge while recording, like Siri's,
/// in ember: a slowly turning angular gradient, stroked at three widths and
/// blurred, brightening with the voice.
struct EdgeGlow: View {
    var level: Float
    var reduceMotion = false

    var body: some View {
        TimelineView(.animation(paused: reduceMotion)) { context in
            let t = context.date.timeIntervalSinceReferenceDate
            let angle = Angle.degrees((t * 40).truncatingRemainder(dividingBy: 360))
            let gradient = AngularGradient(colors: Ember.all + [Ember.gold], center: .center, angle: angle)
            let lift = 0.55 + Double(level) * 0.45
            ZStack {
                RoundedRectangle(cornerRadius: 56, style: .continuous).stroke(gradient, lineWidth: 26).blur(radius: 26).opacity(0.55 * lift)
                RoundedRectangle(cornerRadius: 56, style: .continuous).stroke(gradient, lineWidth: 10).blur(radius: 9).opacity(0.75 * lift)
                RoundedRectangle(cornerRadius: 56, style: .continuous).stroke(gradient, lineWidth: 3).opacity(0.9 * lift)
            }
            .ignoresSafeArea()
        }
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }
}
