import AVFoundation
import ExpoModulesCore

/**
 Reports AVAudioSession interruptions to JS.

 Without this, an interruption is silent and terminal: the OS stops the
 recorder, nothing tells JS, and the recording appears to continue while
 capturing nothing. SegmentedRecorder has always had a handleInterruptionEnd()
 for this, but nothing was subscribed to call it.

 Two notifications matter:

 - `interruptionNotification` — a phone call, Siri, or another app taking
   audio focus. On `.ended` the `shouldResume` option says whether the system
   expects us to start again.
 - `mediaServicesWereResetNotification` — audiod restarted. Every session and
   recorder object is invalid afterwards and must be rebuilt from scratch;
   treating it as an interruption that ended is the closest correct behaviour.
 */
public class ScribeAudioSessionModule: Module {
  private var observers: [NSObjectProtocol] = []

  public func definition() -> ModuleDefinition {
    Name("ScribeAudioSession")

    /// `{ type: "began" | "ended", shouldResume: boolean, reason: string }`
    Events("onInterruption")

    OnStartObserving {
      let center = NotificationCenter.default

      self.observers.append(
        center.addObserver(
          forName: AVAudioSession.interruptionNotification,
          object: nil,
          queue: .main
        ) { [weak self] note in
          guard
            let raw = note.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt,
            let type = AVAudioSession.InterruptionType(rawValue: raw)
          else { return }

          switch type {
          case .began:
            self?.sendEvent("onInterruption", [
              "type": "began",
              "shouldResume": false,
              "reason": "interruption",
            ])
          case .ended:
            let optsRaw =
              note.userInfo?[AVAudioSessionInterruptionOptionKey] as? UInt ?? 0
            let shouldResume =
              AVAudioSession.InterruptionOptions(rawValue: optsRaw).contains(.shouldResume)
            self?.sendEvent("onInterruption", [
              "type": "ended",
              "shouldResume": shouldResume,
              "reason": "interruption",
            ])
          @unknown default:
            break
          }
        }
      )

      self.observers.append(
        center.addObserver(
          forName: AVAudioSession.mediaServicesWereResetNotification,
          object: nil,
          queue: .main
        ) { [weak self] _ in
          // Everything audio-related is now invalid; the recorder has to be
          // rebuilt, so report it as an ended interruption worth resuming.
          self?.sendEvent("onInterruption", [
            "type": "ended",
            "shouldResume": true,
            "reason": "mediaServicesReset",
          ])
        }
      )
    }

    OnStopObserving {
      self.observers.forEach { NotificationCenter.default.removeObserver($0) }
      self.observers.removeAll()
    }

    OnDestroy {
      self.observers.forEach { NotificationCenter.default.removeObserver($0) }
      self.observers.removeAll()
    }
  }
}
