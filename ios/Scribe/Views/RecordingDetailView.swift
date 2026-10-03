import SwiftUI

/// Placeholder until the Recording detail item is ported.
struct RecordingDetailView: View {
    let recordingId: String
    let initial: Recording?

    var body: some View {
        ZStack {
            Theme.bg.ignoresSafeArea()
            Text(initial?.title ?? "Recording").foregroundStyle(Theme.textPrimary)
        }
        .navigationTitle(initial?.title ?? "Recording")
        .navigationBarTitleDisplayMode(.inline)
    }
}
