import SwiftUI

/// The enrolled speaker library: rename someone everywhere, or forget them.
struct SpeakersView: View {
    @State private var speakers: [EnrolledSpeaker] = []
    @State private var loading = true
    @State private var error: String?
    @State private var renaming: EnrolledSpeaker?
    @State private var newName = ""
    @State private var forgetting: EnrolledSpeaker?

    var body: some View {
        List {
            Section {
                Text("Names you give a speaker are kept here. The ones with a voiceprint are matched automatically in recordings you upload later.")
                    .font(.footnote).foregroundStyle(Theme.textMuted)
                    .listRowBackground(Color.clear)
            }
            if let error {
                Label(error, systemImage: "exclamationmark.triangle").foregroundStyle(Theme.amber)
            }
            if !loading && speakers.isEmpty && error == nil {
                Text("No one yet. Tag a speaker from a recording's transcript.").foregroundStyle(Theme.textMuted)
            }
            ForEach(speakers) { s in
                HStack {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(s.displayName).foregroundStyle(Theme.textPrimary)
                        Text(meta(s)).font(.caption).foregroundStyle(Theme.textMuted)
                    }
                    Spacer()
                    if s.hasVoiceprint == true { Image(systemName: "waveform").foregroundStyle(Theme.ready) }
                }
                .listRowBackground(Theme.surface)
                .swipeActions {
                    Button("Forget", role: .destructive) { forgetting = s }
                    Button("Rename") { newName = s.displayName; renaming = s }.tint(Theme.accent)
                }
            }
        }
        .scrollContentBackground(.hidden)
        .background(Theme.bg)
        .navigationTitle("Speakers")
        .overlay { if loading { ProgressView() } }
        .task { await load() }
        .refreshable { await load() }
        .alert("Rename speaker", isPresented: Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } })) {
            TextField("Name", text: $newName)
            Button("Cancel", role: .cancel) { renaming = nil }
            Button("Save") {
                guard let s = renaming else { return }
                let n = newName.trimmingCharacters(in: .whitespaces)
                renaming = nil
                guard !n.isEmpty else { return }
                Task {
                    do { _ = try await APIClient.shared.renameSpeaker(s.id, name: n); await load() }
                    catch { self.error = "Could not rename: \(error.localizedDescription)" }
                }
            }
        } message: {
            Text("The new name replaces the old one in every recording.")
        }
        .confirmationDialog("Forget this speaker?", isPresented: Binding(get: { forgetting != nil }, set: { if !$0 { forgetting = nil } }),
                            titleVisibility: .visible, presenting: forgetting) { s in
            Button("Forget \(s.displayName)", role: .destructive) {
                Task {
                    do { try await APIClient.shared.deleteSpeaker(s.id); await load() }
                    catch { self.error = "Could not delete: \(error.localizedDescription)" }
                }
            }
        } message: { s in
            let n = s.recordingCount ?? 0
            Text("Their name and voiceprint are deleted." + (n > 0 ? " \(n) recording\(n == 1 ? "" : "s") will go back to “Speaker N”." : ""))
        }
    }

    private func meta(_ s: EnrolledSpeaker) -> String {
        let n = s.recordingCount ?? 0
        let voice = s.hasVoiceprint == true ? "voiceprint" : "name only"
        return "\(n) recording\(n == 1 ? "" : "s") · \(voice)"
    }

    private func load() async {
        do {
            speakers = try await APIClient.shared.speakers().sorted { $0.displayName < $1.displayName }
            error = nil
        } catch {
            self.error = error.localizedDescription
        }
        loading = false
    }
}
