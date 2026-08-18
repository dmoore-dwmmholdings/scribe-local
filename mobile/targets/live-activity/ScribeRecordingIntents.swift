import AppIntents
import Foundation

// ⚠️ THIS FILE IS DUPLICATED.
// An identical copy lives at:
//   modules/scribe-live-activity/ios/ScribeRecordingIntents.swift
//
// Same reason as ScribeActivityAttributes: the widget needs these types to
// build `Button(intent:)`, and the app needs them because a LiveActivityIntent
// executes in the APP's process, not the extension's. Apple's guidance is one
// file in two target memberships; @bacons/apple-targets compiles each target
// only from its own folder, so we keep two copies. Keep them identical.

/// Bridges an intent invocation to whatever is listening in the host process.
///
/// In the app this is observed by ScribeLiveActivityModule, which forwards it
/// to JS. Compiled into the widget too, where nothing observes it — harmless,
/// because a LiveActivityIntent's `perform()` runs in the app.
enum ScribeLiveActivityBridge {
  static let actionNotification = Notification.Name("ScribeLiveActivityAction")

  static func emit(_ action: String) {
    NotificationCenter.default.post(
      name: actionNotification,
      object: nil,
      userInfo: ["action": action]
    )
  }
}

/// Pause or resume from the Lock Screen / Dynamic Island.
///
/// Deliberately a toggle rather than separate intents: the button reflects the
/// activity's current state, and a single intent cannot get out of step with it
/// the way a stale "pause" button could after an update is missed.
@available(iOS 17.0, *)
struct ToggleRecordingPauseIntent: LiveActivityIntent {
  static var title: LocalizedStringResource = "Pause or resume recording"
  static var description = IntentDescription("Pauses or resumes the Scribe recording.")
  /// Not something to surface in Shortcuts — it only makes sense from the
  /// Live Activity while a recording is running.
  static var isDiscoverable: Bool = false

  func perform() async throws -> some IntentResult {
    ScribeLiveActivityBridge.emit("togglePause")
    return .result()
  }
}

/// Stop the recording from the Lock Screen / Dynamic Island.
@available(iOS 17.0, *)
struct StopRecordingIntent: LiveActivityIntent {
  static var title: LocalizedStringResource = "Stop recording"
  static var description = IntentDescription("Stops the Scribe recording and finalises it.")
  static var isDiscoverable: Bool = false

  func perform() async throws -> some IntentResult {
    ScribeLiveActivityBridge.emit("stop")
    return .result()
  }
}
