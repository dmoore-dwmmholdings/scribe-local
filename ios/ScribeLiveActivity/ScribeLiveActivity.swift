import ActivityKit
import AppIntents
import SwiftUI
import WidgetKit

/// The timer counts from the effective start — the real start pushed forward by
/// time spent paused — so it stays right without the app pushing updates.
private func timerRange(_ s: ScribeActivityAttributes.ContentState) -> ClosedRange<Date> {
    let start = s.startedAt.addingTimeInterval(s.pausedMs / 1000)
    return start...start.addingTimeInterval(60 * 60 * 24)
}

/// A small ember disc standing in for the orb (widgets cannot run shaders).
private struct EmberDot: View {
    var paused: Bool
    var size: CGFloat = 12
    var body: some View {
        Circle()
            .fill(paused
                  ? AnyShapeStyle(Theme.textMuted)
                  : AnyShapeStyle(RadialGradient(colors: [Color(hex: 0xFFD68C), Theme.accent, Color(hex: 0xFF4840)],
                                                 center: .init(x: 0.35, y: 0.3), startRadius: 0, endRadius: size)))
            .frame(width: size, height: size)
            .shadow(color: paused ? .clear : Theme.accent.opacity(0.6), radius: size / 3)
    }
}

private struct Elapsed: View {
    var state: ScribeActivityAttributes.ContentState
    var font: Font = .system(.title2, design: .rounded).weight(.medium)
    var body: some View {
        if state.isPaused {
            // A paused activity must not use a live timer, or it would keep counting.
            Text("Paused").font(font).foregroundStyle(Theme.amber)
        } else {
            // A live timer takes all the width it is offered; right-align it so
            // it sits at the edge instead of mid-row.
            Text(timerInterval: timerRange(state), countsDown: false)
                .font(font).monospacedDigit().foregroundStyle(Theme.textPrimary)
                .multilineTextAlignment(.trailing)
        }
    }
}

private struct Controls: View {
    var paused: Bool
    var compact = false
    var body: some View {
        HStack(spacing: compact ? 8 : 10) {
            Button(intent: ToggleRecordingPauseIntent()) {
                Image(systemName: paused ? "play.fill" : "pause.fill")
                    .font(.system(size: compact ? 13 : 15, weight: .bold))
                    .foregroundStyle(Theme.textPrimary)
                    .frame(width: compact ? 32 : 40, height: compact ? 32 : 40)
                    .background(Circle().fill(.white.opacity(0.14)))
            }
            .buttonStyle(.plain)
            .accessibilityLabel(paused ? "Resume recording" : "Pause recording")

            Button(intent: StopRecordingIntent()) {
                Image(systemName: "stop.fill")
                    .font(.system(size: compact ? 13 : 15, weight: .bold))
                    .foregroundStyle(.white)
                    .frame(width: compact ? 32 : 40, height: compact ? 32 : 40)
                    .background(Circle().fill(Theme.accentGradient))
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Stop recording")
        }
    }
}

private func subtitle(_ s: ScribeActivityAttributes.ContentState) -> String {
    s.segmentsUploaded == 1 ? "1 segment uploaded" : "\(s.segmentsUploaded) segments uploaded"
}

private struct LockScreen: View {
    var context: ActivityViewContext<ScribeActivityAttributes>
    var body: some View {
        VStack(spacing: 10) {
            HStack(spacing: 10) {
                EmberDot(paused: context.state.isPaused, size: 14)
                Text(context.attributes.title.isEmpty ? "Recording" : context.attributes.title)
                    .font(.headline).foregroundStyle(Theme.textPrimary).lineLimit(1)
                Spacer(minLength: 8)
                Elapsed(state: context.state)
                    .frame(width: 96, alignment: .trailing)
            }
            HStack {
                Text(subtitle(context.state))
                    .font(.system(.caption, design: .monospaced)).foregroundStyle(Theme.textMuted)
                    .lineLimit(1)
                Spacer(minLength: 8)
                Controls(paused: context.state.isPaused)
            }
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 14)
        .activityBackgroundTint(Theme.bg)
        .activitySystemActionForegroundColor(Theme.textPrimary)
    }
}

struct ScribeLiveActivityWidget: Widget {
    var body: some WidgetConfiguration {
        ActivityConfiguration(for: ScribeActivityAttributes.self) { context in
            LockScreen(context: context)
        } dynamicIsland: { context in
            DynamicIsland {
                DynamicIslandExpandedRegion(.leading) {
                    HStack(spacing: 8) {
                        EmberDot(paused: context.state.isPaused)
                        Text(context.state.isPaused ? "Paused" : "Recording")
                            .font(.caption).foregroundStyle(Theme.textMuted)
                    }
                    .padding(.leading, 4)
                }
                DynamicIslandExpandedRegion(.trailing) {
                    Elapsed(state: context.state, font: .system(.body, design: .rounded).weight(.medium))
                        .padding(.trailing, 4)
                }
                DynamicIslandExpandedRegion(.bottom) {
                    HStack {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(context.attributes.title.isEmpty ? "Scribe" : context.attributes.title)
                                .font(.headline).foregroundStyle(Theme.textPrimary).lineLimit(1)
                            Text(subtitle(context.state))
                                .font(.system(.caption, design: .monospaced)).foregroundStyle(Theme.textMuted)
                        }
                        Spacer(minLength: 8)
                        Controls(paused: context.state.isPaused, compact: true)
                    }
                }
            } compactLeading: {
                EmberDot(paused: context.state.isPaused)
            } compactTrailing: {
                Elapsed(state: context.state, font: .system(.caption, design: .rounded).weight(.semibold))
                    .frame(maxWidth: 52)
            } minimal: {
                EmberDot(paused: context.state.isPaused)
            }
            .widgetURL(URL(string: "scribe://record"))
            .keylineTint(Theme.accent)
        }
    }
}

@main
struct ScribeWidgetBundle: WidgetBundle {
    var body: some Widget { ScribeLiveActivityWidget() }
}
