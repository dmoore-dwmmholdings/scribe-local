import ExpoModulesCore

/**
 A repeating timer that keeps firing while the app is backgrounded and the
 screen is locked.

 Why this exists: React Native drives `setTimeout`/`setInterval` from
 `RCTTiming`, which is backed by a `CADisplayLink`. A display link is tied to
 screen refresh, so it stops when the screen turns off. During a locked
 recording the audio session keeps capturing (the app is running, not
 suspended), but JS timers simply never fire — so segment rotation stalls and
 nothing is uploaded until the user unlocks.

 `DispatchSourceTimer` runs on its own queue and is unaffected by display
 state, so it keeps ticking for as long as the process is alive. The app stays
 alive here because `UIBackgroundModes: audio` plus an active recording keeps
 it in the background-running state rather than suspended.

 This is deliberately a plain interval timer with no knowledge of recording:
 the rotation policy stays in JS where it is testable.
 */
public class ScribeBgTimerModule: Module {
  /// Active timers by caller-supplied id, guarded by `lock`.
  private var timers: [String: DispatchSourceTimer] = [:]
  private let lock = NSLock()
  private let queue = DispatchQueue(label: "com.dwmmholdings.scribe.bgtimer", qos: .userInitiated)

  public func definition() -> ModuleDefinition {
    Name("ScribeBgTimer")

    Events("onTick")

    /// Start (or restart) a repeating timer. Emits `onTick` with `{ id, count }`.
    /// Starting an existing id cancels the previous timer first, so callers
    /// cannot accidentally leak overlapping timers.
    Function("start") { (id: String, intervalMs: Double) in
      self.startTimer(id: id, intervalMs: intervalMs)
    }

    /// Stop a timer. Safe to call for an unknown or already-stopped id.
    Function("stop") { (id: String) in
      self.stopTimer(id: id)
    }

    /// Stop every timer — used on teardown so a reloaded JS bundle does not
    /// leave orphaned native timers firing into a dead runtime.
    Function("stopAll") {
      self.stopAllTimers()
    }

    OnDestroy {
      self.stopAllTimers()
    }
  }

  private func startTimer(id: String, intervalMs: Double) {
    // Guard against a zero/negative interval spinning the CPU.
    let interval = max(0.25, intervalMs / 1000.0)

    lock.lock()
    timers[id]?.cancel()

    var count = 0
    let timer = DispatchSource.makeTimerSource(queue: queue)
    timer.schedule(deadline: .now() + interval, repeating: interval, leeway: .milliseconds(50))
    timer.setEventHandler { [weak self] in
      guard let self else { return }
      count += 1
      self.sendEvent("onTick", ["id": id, "count": count])
    }
    timers[id] = timer
    lock.unlock()

    timer.resume()
  }

  private func stopTimer(id: String) {
    lock.lock()
    let timer = timers.removeValue(forKey: id)
    lock.unlock()
    timer?.cancel()
  }

  private func stopAllTimers() {
    lock.lock()
    let all = timers.values
    timers.removeAll()
    lock.unlock()
    all.forEach { $0.cancel() }
  }
}
