import SwiftUI

struct PlaybackBar: View {
    let player: Player
    let marks: [Int]
    @Binding var follow: Bool
    @State private var scrubbing: Double?

    var body: some View {
        VStack(spacing: 6) {
            if let err = player.error {
                Text(err).font(.caption2).foregroundStyle(Theme.amber).frame(maxWidth: .infinity, alignment: .leading)
            }
            ZStack(alignment: .leading) {
                Slider(value: Binding(
                    get: { scrubbing ?? Double(player.currentMs) },
                    set: { scrubbing = $0 }
                ), in: 0...Double(max(1, player.durationMs))) { editing in
                    if !editing, let s = scrubbing {
                        player.seek(toMs: Int(s))
                        scrubbing = nil
                    }
                }
                .accessibilityLabel("Playback position")
                // Marks captured while recording, as ticks on the track.
                GeometryReader { g in
                    ForEach(marks, id: \.self) { m in
                        Rectangle().fill(Theme.amber).frame(width: 2, height: 10)
                            .offset(x: g.size.width * CGFloat(m) / CGFloat(max(1, player.durationMs)) - 1, y: g.size.height / 2 - 5)
                    }
                }
                .allowsHitTesting(false)
            }
            HStack(spacing: 22) {
                Text(formatClock(ms: Int(scrubbing ?? Double(player.currentMs))))
                    .font(.caption.monospacedDigit()).foregroundStyle(Theme.textMuted).frame(width: 56, alignment: .leading)
                Spacer()
                Button { player.skip(by: -15_000) } label: { Image(systemName: "gobackward.15") }
                    .accessibilityLabel("Back 15 seconds")
                Button { player.toggle() } label: {
                    Image(systemName: player.isPlaying ? "pause.circle.fill" : "play.circle.fill").font(.system(size: 40))
                }
                .accessibilityLabel(player.isPlaying ? "Pause" : "Play")
                Button { player.skip(by: 15_000) } label: { Image(systemName: "goforward.15") }
                    .accessibilityLabel("Forward 15 seconds")
                Spacer()
                HStack(spacing: 10) {
                    Button { player.cycleRate() } label: {
                        Text(rateLabel).font(.caption.weight(.semibold).monospacedDigit())
                    }
                    .accessibilityLabel("Playback speed \(rateLabel)")
                    Button { follow.toggle() } label: {
                        Image(systemName: follow ? "text.line.first.and.arrowtriangle.forward" : "text.alignleft")
                    }
                    .accessibilityLabel(follow ? "Stop following the transcript" : "Follow the transcript")
                }
                .frame(width: 56, alignment: .trailing)
            }
            .font(.title3)
        }
        .padding(.horizontal, 16).padding(.vertical, 10)
        .background(.ultraThinMaterial)
    }

    private var rateLabel: String {
        let r = player.rate
        return r == r.rounded() ? "\(Int(r))×" : "\(r)×"
    }
}

/// Lays out children left to right, wrapping, like words in a paragraph.
struct FlowLayout: Layout {
    var spacing: CGFloat = 4
    var lineSpacing: CGFloat = 2

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let width = proposal.width ?? .infinity
        var x: CGFloat = 0, y: CGFloat = 0, lineHeight: CGFloat = 0, maxX: CGFloat = 0
        for v in subviews {
            let s = v.sizeThatFits(.unspecified)
            if x > 0 && x + s.width > width { x = 0; y += lineHeight + lineSpacing; lineHeight = 0 }
            x += s.width + spacing
            maxX = max(maxX, x - spacing)
            lineHeight = max(lineHeight, s.height)
        }
        return CGSize(width: proposal.width ?? maxX, height: y + lineHeight)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var x = bounds.minX, y = bounds.minY, lineHeight: CGFloat = 0
        for v in subviews {
            let s = v.sizeThatFits(.unspecified)
            if x > bounds.minX && x + s.width > bounds.maxX { x = bounds.minX; y += lineHeight + lineSpacing; lineHeight = 0 }
            v.place(at: CGPoint(x: x, y: y), proposal: ProposedViewSize(s))
            x += s.width + spacing
            lineHeight = max(lineHeight, s.height)
        }
    }
}
