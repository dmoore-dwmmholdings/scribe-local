import SwiftUI

/// The visual language carried over from the React Native app: warm
/// near-black surfaces, a kiln-fire orange accent, warm off-white text.
enum Theme {
    static let bg = Color(hex: 0x100C0B)
    static let surface = Color(hex: 0x1A1513)
    static let surfaceAlt = Color(hex: 0x150F0D)

    static let accent = Color(hex: 0xFF7A3C)
    static let accentDeep = Color(hex: 0xE8512E)
    static let amber = Color(hex: 0xE2A85F)
    static let ready = Color(hex: 0x93B072)

    static let textPrimary = Color(hex: 0xF0E5D8)
    static let textBody = Color(hex: 0xE7DCC5)
    static let textMuted = Color(hex: 0x9C8A7C)
    static let textDim = Color(hex: 0x6A594D)

    static let border = Color(red: 1, green: 235 / 255, blue: 220 / 255).opacity(0.07)
    static let accentSoft = accent.opacity(0.12)

    static let accentGradient = LinearGradient(
        colors: [accent, accentDeep], startPoint: .topLeading, endPoint: .bottomTrailing)

    /// One colour per diarized speaker, cycling.
    static let speakerColors: [Color] = [
        Color(hex: 0xFF7A3C), Color(hex: 0x6F9FD8), Color(hex: 0x93B072),
        Color(hex: 0xC98BD0), Color(hex: 0xE2A85F), Color(hex: 0x6FD0C0),
    ]

    static func speakerColor(_ localIdx: Int?) -> Color {
        guard let i = localIdx else { return textMuted }
        return speakerColors[((i % speakerColors.count) + speakerColors.count) % speakerColors.count]
    }
}

extension Color {
    init(hex: UInt32, opacity: Double = 1) {
        self.init(
            .sRGB,
            red: Double((hex >> 16) & 0xFF) / 255,
            green: Double((hex >> 8) & 0xFF) / 255,
            blue: Double(hex & 0xFF) / 255,
            opacity: opacity)
    }
}
