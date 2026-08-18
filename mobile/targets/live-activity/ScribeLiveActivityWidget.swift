import ActivityKit
import AppIntents
import SwiftUI
import WidgetKit

// Kiln palette (mobile/src/theme.ts) — hardcoded rather than shared, since the
// extension cannot import the app's JS theme.
private let kilnBackground = Color(red: 0.063, green: 0.047, blue: 0.043) // #100c0b
private let kilnEmber = Color(red: 0.898, green: 0.361, blue: 0.196)      // #e55c32
private let kilnText = Color(red: 0.949, green: 0.929, blue: 0.914)       // #f2ede9
private let kilnMuted = Color(red: 0.608, green: 0.573, blue: 0.553)      // #9b928d

/// The range the Lock Screen timer counts over. Starting from the effective
/// start (real start pushed forward by paused time) makes SwiftUI render a
/// self-updating clock, so elapsed time stays correct without the app waking
/// to push updates.
private func timerRange(_ state: ScribeActivityAttributes.ContentState) -> ClosedRange<Date> {
  let effectiveStart = state.startedAt.addingTimeInterval(state.pausedMs / 1000.0)
  return effectiveStart...effectiveStart.addingTimeInterval(60 * 60 * 24)
}

private struct RecordingDot: View {
  var isPaused: Bool
  var body: some View {
    Circle()
      .fill(isPaused ? kilnMuted : kilnEmber)
      .frame(width: 10, height: 10)
  }
}

private struct ElapsedText: View {
  var state: ScribeActivityAttributes.ContentState
  var font: Font = .system(.title2, design: .monospaced)

  var body: some View {
    if state.isPaused {
      // A paused activity must NOT use a live timer, or it would keep counting.
      Text("Paused")
        .font(font)
        .foregroundStyle(kilnMuted)
    } else {
      Text(timerInterval: timerRange(state), countsDown: false)
        .font(font)
        .monospacedDigit()
        .foregroundStyle(kilnText)
    }
  }
}


/// Pause/resume + stop controls.
///
/// Interactive Live Activities need iOS 17 (`Button(intent:)` with a
/// LiveActivityIntent). The widget deploys to 16.2, so on 16.x these simply do
/// not render and the activity stays a read-only status display.
@available(iOS 17.0, *)
private struct RecordingControls: View {
  var isPaused: Bool
  var compact: Bool = false

  var body: some View {
    HStack(spacing: compact ? 8 : 10) {
      Button(intent: ToggleRecordingPauseIntent()) {
        Label(
          isPaused ? "Resume" : "Pause",
          systemImage: isPaused ? "play.fill" : "pause.fill"
        )
        .labelStyle(.iconOnly)
        .font(.system(size: compact ? 13 : 15, weight: .bold))
        .foregroundStyle(kilnText)
        .frame(width: compact ? 30 : 38, height: compact ? 30 : 38)
        .background(Circle().fill(.white.opacity(0.14)))
      }
      .buttonStyle(.plain)
      .accessibilityLabel(isPaused ? "Resume recording" : "Pause recording")

      Button(intent: StopRecordingIntent()) {
        Label("Stop", systemImage: "stop.fill")
          .labelStyle(.iconOnly)
          .font(.system(size: compact ? 13 : 15, weight: .bold))
          .foregroundStyle(kilnText)
          .frame(width: compact ? 30 : 38, height: compact ? 30 : 38)
          .background(Circle().fill(kilnEmber))
      }
      .buttonStyle(.plain)
      .accessibilityLabel("Stop recording")
    }
  }
}

/// Lock Screen / Notification Center presentation.
private struct LockScreenView: View {
  var context: ActivityViewContext<ScribeActivityAttributes>

  var body: some View {
    HStack(alignment: .center, spacing: 14) {
      RecordingDot(isPaused: context.state.isPaused)

      VStack(alignment: .leading, spacing: 3) {
        Text(context.attributes.title.isEmpty ? "Recording" : context.attributes.title)
          .font(.headline)
          .foregroundStyle(kilnText)
          .lineLimit(1)
        Text("\(context.state.segmentsUploaded) segments uploaded")
          .font(.caption)
          .foregroundStyle(kilnMuted)
      }

      Spacer(minLength: 8)

      VStack(alignment: .trailing, spacing: 8) {
        ElapsedText(state: context.state)
        if #available(iOS 17.0, *) {
          RecordingControls(isPaused: context.state.isPaused)
        }
      }
    }
    .padding(.horizontal, 18)
    .padding(.vertical, 14)
    .activityBackgroundTint(kilnBackground)
    .activitySystemActionForegroundColor(kilnText)
  }
}

struct ScribeLiveActivityWidget: Widget {
  var body: some WidgetConfiguration {
    ActivityConfiguration(for: ScribeActivityAttributes.self) { context in
      LockScreenView(context: context)
    } dynamicIsland: { context in
      DynamicIsland {
        DynamicIslandExpandedRegion(.leading) {
          HStack(spacing: 8) {
            RecordingDot(isPaused: context.state.isPaused)
            Text(context.state.isPaused ? "Paused" : "Recording")
              .font(.caption)
              .foregroundStyle(kilnMuted)
          }
          .padding(.leading, 4)
        }
        DynamicIslandExpandedRegion(.trailing) {
          ElapsedText(state: context.state, font: .system(.body, design: .monospaced))
            .padding(.trailing, 4)
        }
        DynamicIslandExpandedRegion(.bottom) {
          HStack(alignment: .center) {
            VStack(alignment: .leading, spacing: 2) {
              Text(context.attributes.title.isEmpty ? "Scribe" : context.attributes.title)
                .font(.headline)
                .foregroundStyle(kilnText)
                .lineLimit(1)
              Text("\(context.state.segmentsUploaded) segments uploaded")
                .font(.caption)
                .foregroundStyle(kilnMuted)
            }
            Spacer(minLength: 8)
            if #available(iOS 17.0, *) {
              RecordingControls(isPaused: context.state.isPaused, compact: true)
            }
          }
        }
      } compactLeading: {
        RecordingDot(isPaused: context.state.isPaused)
      } compactTrailing: {
        ElapsedText(state: context.state, font: .system(.caption, design: .monospaced))
      } minimal: {
        RecordingDot(isPaused: context.state.isPaused)
      }
      .widgetURL(URL(string: "scribe://"))
      .keylineTint(kilnEmber)
    }
  }
}

@main
struct ScribeWidgetBundle: WidgetBundle {
  var body: some Widget {
    ScribeLiveActivityWidget()
  }
}
