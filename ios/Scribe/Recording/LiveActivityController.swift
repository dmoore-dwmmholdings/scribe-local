import ActivityKit
import Foundation

/// Starts, updates and ends the recording Live Activity on the Lock Screen and
/// in the Dynamic Island. Updates are sent only when something the timer cannot
/// work out for itself changes — pause state, the upload count — which keeps
/// within ActivityKit's update budget.
final class LiveActivityController {
    static let shared = LiveActivityController()

    private var activity: Activity<ScribeActivityAttributes>?
    private var state: ScribeActivityAttributes.ContentState?

    func start(title: String?, startedAt: Date) {
        guard ActivityAuthorizationInfo().areActivitiesEnabled else { return }
        // An activity left over from a crash would otherwise sit there forever.
        for old in Activity<ScribeActivityAttributes>.activities {
            Task { await old.end(nil, dismissalPolicy: .immediate) }
        }
        let s = ScribeActivityAttributes.ContentState(startedAt: startedAt, pausedMs: 0, isPaused: false, segmentsUploaded: 0)
        state = s
        activity = try? Activity.request(
            attributes: ScribeActivityAttributes(title: title ?? ""),
            content: ActivityContent(state: s, staleDate: nil),
            pushType: nil)
    }

    func update(_ change: (inout ScribeActivityAttributes.ContentState) -> Void) {
        guard var s = state, let activity else { return }
        change(&s)
        guard s != state else { return }
        state = s
        Task { await activity.update(ActivityContent(state: s, staleDate: nil)) }
    }

    func end() {
        guard let activity else { return }
        self.activity = nil
        state = nil
        Task { await activity.end(nil, dismissalPolicy: .immediate) }
    }
}
