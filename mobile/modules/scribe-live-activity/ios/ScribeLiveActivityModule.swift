import ActivityKit
import ExpoModulesCore

/**
 Starts / updates / ends the recording Live Activity.

 Updates are pushed from this process (no APNs), which matters for two reasons:
 it needs no push entitlement or paid membership, and it works while the phone
 is locked — the app is already alive there because an active audio session
 keeps it background-running.

 Update frequency is deliberately low: the Lock Screen elapsed time is rendered
 by SwiftUI's `Text(timerInterval:)`, so it ticks on its own. We only push an
 update when something the timer cannot derive changes (segment count, pause
 state), which keeps us clear of ActivityKit's update budget.
 */
public class ScribeLiveActivityModule: Module {
  /// The in-flight activity. `Any` because the concrete generic type is only
  /// nameable under `@available`, which stored properties cannot carry.
  private var currentActivity: Any?

  public func definition() -> ModuleDefinition {
    Name("ScribeLiveActivity")

    /// Whether Live Activities can actually be shown right now. False on
    /// iOS < 16.2, and false when the user has switched them off for the app —
    /// callers must treat this as "skip", not "error".
    Function("isSupported") { () -> Bool in
      if #available(iOS 16.2, *) {
        return ActivityAuthorizationInfo().areActivitiesEnabled
      }
      return false
    }

    AsyncFunction("start") { (title: String, startedAtMs: Double, promise: Promise) in
      guard #available(iOS 16.2, *) else {
        promise.resolve(false)
        return
      }
      guard ActivityAuthorizationInfo().areActivitiesEnabled else {
        promise.resolve(false)
        return
      }

      self.currentActivity = nil

      Task {
        // Sweep ALL existing activities, not just one this process started — a
        // previous run may have crashed and left one stranded on the Lock
        // Screen, where it is unreachable by any handle this process holds.
        for stale in Activity<ScribeActivityAttributes>.activities {
          await stale.end(nil, dismissalPolicy: .immediate)
        }

        let attributes = ScribeActivityAttributes(title: title)
        let state = ScribeActivityAttributes.ContentState(
          startedAt: Date(timeIntervalSince1970: startedAtMs / 1000.0),
          pausedMs: 0,
          isPaused: false,
          segmentsUploaded: 0
        )

        do {
          let activity = try Activity.request(
            attributes: attributes,
            content: ActivityContent(state: state, staleDate: nil),
            pushType: nil
          )
          self.currentActivity = activity
          promise.resolve(true)
        } catch {
          // Most commonly: too many active activities, or the user disabled
          // them between the check above and here. Not fatal to recording.
          promise.reject("ERR_LIVE_ACTIVITY_START", error.localizedDescription)
        }
      }
    }

    AsyncFunction("update") {
      (startedAtMs: Double, pausedMs: Double, isPaused: Bool, segmentsUploaded: Int, promise: Promise) in
      guard #available(iOS 16.2, *),
            let activity = self.currentActivity as? Activity<ScribeActivityAttributes> else {
        promise.resolve(false)
        return
      }

      let state = ScribeActivityAttributes.ContentState(
        startedAt: Date(timeIntervalSince1970: startedAtMs / 1000.0),
        pausedMs: pausedMs,
        isPaused: isPaused,
        segmentsUploaded: segmentsUploaded
      )

      Task {
        await activity.update(ActivityContent(state: state, staleDate: nil))
        promise.resolve(true)
      }
    }

    /// End EVERY activity belonging to this app, including ones this process
    /// did not start.
    ///
    /// ActivityKit activities outlive the app: if the app crashes mid-recording
    /// the Live Activity keeps showing "recording" forever, and because the new
    /// process has no handle to it, neither relaunching nor rebooting clears it.
    /// The only way back is to enumerate the system's activities and end them.
    AsyncFunction("endAll") { (promise: Promise) in
      guard #available(iOS 16.2, *) else {
        promise.resolve(0)
        return
      }
      self.currentActivity = nil
      Task {
        var ended = 0
        for activity in Activity<ScribeActivityAttributes>.activities {
          ended += 1
          await activity.end(nil, dismissalPolicy: .immediate)
        }
        promise.resolve(ended)
      }
    }

    AsyncFunction("end") { (promise: Promise) in
      self.endCurrentActivity(dismissImmediately: true)
      promise.resolve(true)
    }

    OnDestroy {
      // A reloaded JS bundle must not leave a stale "recording" pill on the
      // Lock Screen with nothing driving it.
      self.endCurrentActivity(dismissImmediately: true)
    }
  }

  private func endCurrentActivity(dismissImmediately: Bool) {
    guard #available(iOS 16.2, *),
          let activity = currentActivity as? Activity<ScribeActivityAttributes> else {
      currentActivity = nil
      return
    }
    currentActivity = nil
    Task {
      await activity.end(nil, dismissalPolicy: dismissImmediately ? .immediate : .default)
    }
  }
}
