import AppIntents
import Foundation

// The Live Activity's buttons. A LiveActivityIntent runs in the app's process,
// so these act on the recording session directly; the widget extension only
// needs the types to draw the buttons.

struct ToggleRecordingPauseIntent: LiveActivityIntent {
    static var title: LocalizedStringResource = "Pause or resume recording"
    static var description = IntentDescription("Pauses or resumes the Scribe recording.")
    static var isDiscoverable = false

    func perform() async throws -> some IntentResult {
        #if !WIDGET_EXTENSION
        await MainActor.run {
            let s = RecordingSession.shared
            s.state == .recording ? s.pause() : s.resume()
        }
        #endif
        return .result()
    }
}

struct StopRecordingIntent: LiveActivityIntent {
    static var title: LocalizedStringResource = "Stop recording"
    static var description = IntentDescription("Stops the Scribe recording and saves it.")
    static var isDiscoverable = false

    func perform() async throws -> some IntentResult {
        #if !WIDGET_EXTENSION
        await MainActor.run { RecordingSession.shared.stop() }
        #endif
        return .result()
    }
}
