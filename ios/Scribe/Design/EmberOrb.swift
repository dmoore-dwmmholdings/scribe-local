import SwiftUI
import UIKit

/// The ember palette from the old app — gold core, tangerine, coral, ember pink.
enum Ember {
    static let gold = Color(red: 1, green: 214 / 255, blue: 140 / 255)
    static let tangerine = Color(red: 1, green: 176 / 255, blue: 80 / 255)
    static let orange = Color(red: 1, green: 120 / 255, blue: 54 / 255)
    static let coral = Color(red: 1, green: 72 / 255, blue: 64 / 255)
    static let pink = Color(red: 1, green: 98 / 255, blue: 150 / 255)
}

/// Eases the input level towards its target each frame: quick to rise, slow to
/// fall, so the visuals swell with a voice and settle instead of jumping at
/// every ~10 Hz level update.
final class LevelSmoother {
    private(set) var value: Float = 0
    private var last = Date()

    func step(toward target: Float, at now: Date) -> Float {
        let dt = Float(min(max(now.timeIntervalSince(last), 0), 0.1))
        last = now
        let rate: Float = target > value ? 14 : 3.5
        value += (target - value) * min(1, rate * dt)
        return value
    }
}

/// A glass sphere of moving fire, after the iOS Siri orb in ember colours.
/// Drawn by the `emberOrb` Metal shader; always flowing, faster and more
/// turbulent with the voice.
struct EmberOrb: View {
    var level: Float
    var active: Bool
    var reduceMotion = false

    @State private var smoother = LevelSmoother()
    @State private var start = Date()

    var body: some View {
        TimelineView(.animation(paused: reduceMotion)) { context in
            let lvl = smoother.step(toward: active ? level : 0, at: context.date)
            let t = Float(context.date.timeIntervalSince(start))
            GeometryReader { g in
                Rectangle()
                    .fill(.white)
                    .colorEffect(ShaderLibrary.emberOrb(.float2(g.size), .float(t), .float(lvl)))
            }
        }
        .accessibilityHidden(true)
    }
}

/// The glow around the screen's edge while recording, after Siri's: drawn by the
/// `edgeGlow` shader to follow the display's own corner radius.
struct EdgeGlow: View {
    var level: Float
    var reduceMotion = false

    @State private var smoother = LevelSmoother()
    @State private var start = Date()

    var body: some View {
        TimelineView(.animation(paused: reduceMotion)) { context in
            let lvl = smoother.step(toward: level, at: context.date)
            let t = Float(context.date.timeIntervalSince(start))
            GeometryReader { g in
                Rectangle()
                    .fill(.white)
                    .colorEffect(ShaderLibrary.edgeGlow(.float2(g.size), .float(Float(Self.cornerRadius)), .float(t), .float(lvl)))
            }
        }
        .ignoresSafeArea()
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }

    /// The display's corner radius, so the glow hugs the glass. UIKit keeps it
    /// behind a private key; fall back to a value close to current iPhones.
    static let cornerRadius: CGFloat = {
        if let r = UIScreen.main.value(forKey: "_displayCornerRadius") as? CGFloat, r > 0 { return r }
        return 55
    }()
}
