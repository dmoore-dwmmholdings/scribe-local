import SwiftUI

struct RecordView: View {
    private var session = RecordingSession.shared
    @State private var title = ""
    @State private var participants = Settings.shared.defaultParticipants

    var body: some View {
        NavigationStack {
            ZStack {
                Theme.bg.ignoresSafeArea()
                VStack(spacing: 24) {
                    statusLine
                    Spacer()
                    Text(formatClock(ms: session.elapsedMs))
                        .font(.system(size: 56, weight: .light, design: .rounded).monospacedDigit())
                        .foregroundStyle(session.state == .paused ? Theme.amber : Theme.textPrimary)
                    LevelMeter(level: session.isActive && session.state == .recording ? session.level : 0)
                        .frame(height: 36).padding(.horizontal, 40)
                    if !session.isActive { setup }
                    Spacer()
                    controls
                    if let err = session.error {
                        Text(err).font(.footnote).foregroundStyle(Theme.amber).multilineTextAlignment(.center)
                    }
                    if case .finished = session.state {
                        Text("Saved. It uploads in the background and appears in the Library once processed.")
                            .font(.footnote).foregroundStyle(Theme.textMuted).multilineTextAlignment(.center)
                    }
                }
                .padding(24)
            }
            .navigationTitle("Record")
            .navigationBarTitleDisplayMode(.inline)
        }
    }

    private var statusLine: some View {
        HStack(spacing: 8) {
            switch session.state {
            case .recording:
                Circle().fill(Theme.accentDeep).frame(width: 8, height: 8)
                Text("REC · 16 kHz AAC").foregroundStyle(Theme.accent)
            case .paused:
                Text("❚❚ PAUSED").foregroundStyle(Theme.amber)
            case .interrupted:
                Text("INTERRUPTED — resumes after the call").foregroundStyle(Theme.amber)
            case .finished:
                Text("✓ SAVED").foregroundStyle(Theme.ready)
            case .idle:
                Text(Settings.shared.isConfigured ? "READY · 16 kHz AAC" : "READY · no server set up — recordings wait on the phone")
                    .foregroundStyle(Theme.textMuted)
            }
            if !session.marks.isEmpty {
                Text("⚑ \(session.marks.count)").foregroundStyle(Theme.amber)
            }
        }
        .font(.caption.weight(.semibold))
    }

    private var setup: some View {
        VStack(spacing: 12) {
            TextField("Title (optional)", text: $title)
                .textFieldStyle(.roundedBorder)
            Stepper(value: $participants, in: 1...20) {
                Text("\(participants) \(participants == 1 ? "person" : "people") speaking")
                    .foregroundStyle(Theme.textPrimary)
            }
            Text("Saying how many people will speak helps tell them apart.")
                .font(.caption).foregroundStyle(Theme.textMuted)
        }
        .padding(.horizontal, 8)
    }

    @ViewBuilder private var controls: some View {
        if session.isActive {
            HStack(spacing: 36) {
                control("flag.fill", "Mark", tint: Theme.amber) { session.mark() }
                if session.state == .recording {
                    control("pause.fill", "Pause", tint: Theme.textPrimary) { session.pause() }
                } else {
                    control("play.fill", "Resume", tint: Theme.accent) { session.resume() }
                }
                control("stop.fill", "Stop", tint: Theme.accentDeep, big: true) {
                    session.stop()
                    title = ""
                }
            }
        } else {
            Button {
                if case .finished = session.state { session.reset() }
                Task { await session.start(title: title, participants: participants) }
            } label: {
                ZStack {
                    Circle().fill(Theme.accentGradient).frame(width: 96, height: 96)
                        .shadow(color: Theme.accent.opacity(0.4), radius: 18)
                    Image(systemName: "mic.fill").font(.system(size: 34, weight: .semibold)).foregroundStyle(.white)
                }
            }
            .accessibilityLabel("Start recording")
        }
    }

    private func control(_ icon: String, _ label: String, tint: Color, big: Bool = false, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            VStack(spacing: 6) {
                ZStack {
                    Circle().fill(Theme.surface).frame(width: big ? 76 : 60, height: big ? 76 : 60)
                    Image(systemName: icon).font(.system(size: big ? 28 : 22)).foregroundStyle(tint)
                }
                Text(label).font(.caption).foregroundStyle(Theme.textMuted)
            }
        }
        .accessibilityLabel(label)
    }
}

/// A row of bars that rise with the input level.
struct LevelMeter: View {
    let level: Float
    private let bars = 28

    var body: some View {
        GeometryReader { g in
            HStack(alignment: .center, spacing: 3) {
                ForEach(0..<bars, id: \.self) { i in
                    let center = abs(Double(i) - Double(bars - 1) / 2) / (Double(bars) / 2)
                    let h = max(3, g.size.height * CGFloat(level) * CGFloat(1 - center * 0.7))
                    Capsule().fill(Theme.accent.opacity(0.35 + Double(level) * 0.65))
                        .frame(width: (g.size.width - CGFloat(bars - 1) * 3) / CGFloat(bars), height: h)
                }
            }
            .frame(maxHeight: .infinity)
            .animation(.easeOut(duration: 0.12), value: level)
        }
    }
}
