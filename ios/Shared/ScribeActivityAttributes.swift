import ActivityKit
import Foundation

/// The recording Live Activity's data, shared by the app (which starts and
/// updates it) and the widget extension (which draws it).
struct ScribeActivityAttributes: ActivityAttributes {
    struct ContentState: Codable, Hashable {
        /// When recording started; with `pausedMs` it gives a self-updating
        /// timer, so the app does not need to wake to tick it.
        var startedAt: Date
        var pausedMs: Double
        var isPaused: Bool
        var segmentsUploaded: Int
    }

    var title: String
}
