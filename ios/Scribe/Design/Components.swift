import SwiftUI

// The building blocks of the old app's look, so every screen shares it: large
// bold screen titles, small letter-spaced monospaced labels in the accent,
// warm cards with a hairline border.

extension Font {
    /// Menlo, as the old app used for labels, counts and timestamps.
    static func mono(_ size: CGFloat, weight: Font.Weight = .semibold) -> Font {
        .system(size: size, weight: weight, design: .monospaced)
    }
}

/// The big title at the top of a tab, with an optional accessory on the right.
struct ScreenTitle<Accessory: View>: View {
    let title: String
    let accessory: Accessory

    init(_ title: String, @ViewBuilder accessory: () -> Accessory = { EmptyView() }) {
        self.title = title
        self.accessory = accessory()
    }

    var body: some View {
        HStack(alignment: .firstTextBaseline) {
            Text(title)
                .font(.system(size: 27, weight: .bold))
                .tracking(-0.5)
                .foregroundStyle(Theme.textPrimary)
            Spacer()
            accessory
        }
        .padding(.horizontal, 22)
        .padding(.top, 6)
        .padding(.bottom, 10)
    }
}

/// `TRANSCRIPT`, `SUMMARY` — small, spaced, monospaced, in the accent.
struct SectionLabel: View {
    let text: String
    var color: Color = Theme.accent
    init(_ text: String, color: Color = Theme.accent) {
        self.text = text
        self.color = color
    }
    var body: some View {
        Text(text.uppercased())
            .font(.mono(11, weight: .bold))
            .tracking(2)
            .foregroundStyle(color)
            .padding(.leading, 6)
    }
}

/// A raised surface with the hairline border the old app's cards had.
struct Card<Content: View>: View {
    var padding: CGFloat = 16
    @ViewBuilder let content: Content

    var body: some View {
        content
            .padding(padding)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Theme.surface, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).stroke(Theme.border, lineWidth: 1))
    }
}

/// A full-bleed screen on the warm background, scrolling, with the title.
struct TabScreen<Accessory: View, Content: View>: View {
    let title: String
    let accessory: Accessory
    let content: Content

    init(_ title: String, @ViewBuilder accessory: () -> Accessory = { EmptyView() }, @ViewBuilder content: () -> Content) {
        self.title = title
        self.accessory = accessory()
        self.content = content()
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                ScreenTitle(title) { accessory }
                content.padding(.horizontal, 16)
            }
            .padding(.bottom, 24)
        }
        .scrollDismissesKeyboard(.interactively)
        .background(Theme.bg.ignoresSafeArea())
        .toolbar(.hidden, for: .navigationBar)
    }
}

/// A tappable row inside a card: icon, label, optional trailing text, chevron.
struct CardRow: View {
    let icon: String
    let label: String
    var trailing: String?
    var tint: Color = Theme.accent

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: icon).foregroundStyle(tint).frame(width: 22)
            Text(label).foregroundStyle(Theme.textPrimary)
            Spacer()
            if let trailing { Text(trailing).foregroundStyle(Theme.textMuted).font(.footnote) }
            Image(systemName: "chevron.right").font(.footnote.weight(.semibold)).foregroundStyle(Theme.textDim)
        }
        .contentShape(Rectangle())
        .padding(.vertical, 12)
    }
}

struct Hairline: View {
    var body: some View { Rectangle().fill(Theme.border).frame(height: 1) }
}

/// The input style: dark field, hairline border, small caption above.
struct FieldBox<Content: View>: View {
    let label: String
    @ViewBuilder let content: Content
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(label.uppercased()).font(.mono(10, weight: .bold)).tracking(1.5).foregroundStyle(Theme.textMuted)
            content
                .padding(.horizontal, 12).padding(.vertical, 11)
                .background(Theme.bg, in: RoundedRectangle(cornerRadius: 13, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 13, style: .continuous).stroke(Theme.border, lineWidth: 1))
                .foregroundStyle(Theme.textPrimary)
        }
    }
}

/// A full-width accent button, or a quieter outlined one.
struct PillButton: View {
    let title: String
    var icon: String?
    var prominent = true
    var busy = false
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 8) {
                if busy { ProgressView().tint(prominent ? .white : Theme.accent) }
                else if let icon { Image(systemName: icon) }
                Text(title).fontWeight(.semibold)
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 13)
            .foregroundStyle(prominent ? Color.white : Theme.accent)
            .background {
                if prominent { Capsule().fill(Theme.accentGradient) }
                else { Capsule().stroke(Theme.accent.opacity(0.45), lineWidth: 1) }
            }
        }
        .buttonStyle(.plain)
        .disabled(busy)
        .accessibilityLabel(title)
    }
}

extension View {
    /// Liquid Glass where the system has it, a material otherwise.
    @ViewBuilder func glassBackground(cornerRadius: CGFloat = 22) -> some View {
        if #available(iOS 26.0, *) {
            self.glassEffect(.regular.tint(Theme.accent.opacity(0.06)), in: RoundedRectangle(cornerRadius: cornerRadius, style: .continuous))
        } else {
            self.background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: cornerRadius, style: .continuous))
        }
    }
}
