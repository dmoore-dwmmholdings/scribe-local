import ActivityKit
import Foundation

// ⚠️ THIS FILE IS DUPLICATED.
// An identical copy lives at:
//   modules/scribe-live-activity/ios/ScribeActivityAttributes.swift
//
// ActivityKit matches the app's `Activity<Attributes>.request(...)` to the
// widget's `ActivityConfiguration(for:)` by the attributes type NAME, not by
// module identity — which is why Apple's own guidance is to add one source file
// to two target memberships. @bacons/apple-targets compiles each target only
// from its own directory, so we keep two copies instead. They must stay
// byte-for-byte equivalent in shape: renaming or reordering fields in one
// without the other silently breaks the Live Activity (it starts, but the
// system finds no matching configuration and renders nothing).

struct ScribeActivityAttributes: ActivityAttributes {
  /// Values that change while the recording runs.
  public struct ContentState: Codable, Hashable {
    /// When capture began. Drives the live-counting timer in the UI, so the
    /// elapsed time keeps ticking on the Lock Screen with no further updates —
    /// important, because updates only happen on segment boundaries.
    var startedAt: Date
    /// Wall-clock time accumulated while paused, excluded from the timer.
    var pausedMs: Double
    var isPaused: Bool
    /// Segments successfully uploaded so far — the visible proof that the
    /// pipeline is still moving while the phone is locked.
    var segmentsUploaded: Int
  }

  /// Fixed for the lifetime of the activity.
  var title: String
}
