import SwiftUI

/// Name the diarized speaker behind a transcript line: with someone already in
/// the library, or a new name. Keeping the voiceprint ("recognise later") is
/// what carries the name into recordings uploaded afterwards.
struct SpeakerTagSheet: View {
    @Environment(\.dismiss) private var dismiss
    let currentLabel: String
    let isNamed: Bool
    /// Tag with a request; returns an error message or a notice to show.
    let onTag: (NameSpeakerRequest) async -> TagOutcome
    let onUntag: () async -> String?
    let onNotParticipant: () async -> String?

    enum TagOutcome { case done, notice(String), failed(String) }

    @State private var known: [EnrolledSpeaker] = []
    @State private var name = ""
    @State private var enroll = true
    @State private var saving = false
    @State private var message: AlertItem?
    @State private var relearn: EnrolledSpeaker?
    @State private var confirmRemove = false

    private var matches: [EnrolledSpeaker] {
        let q = name.trimmingCharacters(in: .whitespaces).lowercased()
        guard !q.isEmpty else { return known }
        return known.filter { $0.displayName.lowercased().contains(q) }
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("Name, e.g. Dawson", text: $name)
                        .textInputAutocapitalization(.words)
                        .onSubmit(submitName)
                    Toggle("Recognise this voice later", isOn: $enroll)
                    Button("Tag as “\(name.trimmingCharacters(in: .whitespaces))”", action: submitName)
                        .disabled(name.trimmingCharacters(in: .whitespaces).isEmpty || saving)
                } header: {
                    Text("Someone new")
                } footer: {
                    Text("With recognition on, recordings you upload after this get this name automatically.")
                }

                Section("Known speakers") {
                    if known.isEmpty {
                        Text("No one yet.").foregroundStyle(Theme.textMuted)
                    } else if matches.isEmpty {
                        Text("No one matches “\(name.trimmingCharacters(in: .whitespaces))”.").foregroundStyle(Theme.textMuted)
                    }
                    ForEach(matches) { s in
                        Button { tag(NameSpeakerRequest(speakerId: s.id, enroll: enroll)) } label: {
                            HStack {
                                Text(s.displayName).foregroundStyle(Theme.textPrimary)
                                Spacer()
                                if s.hasVoiceprint == true {
                                    Image(systemName: "waveform").foregroundStyle(Theme.ready).accessibilityLabel("Has a voiceprint")
                                }
                                Text("\(s.recordingCount ?? 0)").font(.caption).foregroundStyle(Theme.textMuted)
                            }
                        }
                        .swipeActions {
                            Button("Re-learn voice") { relearn = s }.tint(Theme.amber)
                        }
                        .contextMenu {
                            Button { relearn = s } label: { Label("Re-learn \(s.displayName)'s voice from this recording", systemImage: "waveform.badge.plus") }
                        }
                    }
                }

                Section {
                    if isNamed {
                        Button("Remove name") { run { await onUntag() } }
                    }
                    Button("Not a participant", role: .destructive) { confirmRemove = true }
                } footer: {
                    Text("Not a participant removes this voice and every line it said — a television, a passer-by — and rebuilds the summary without it. Reprocessing brings it back.")
                }
            }
            .navigationTitle("Tag speaker")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .principal) {
                    VStack {
                        Text("Tag speaker").font(.headline)
                        Text("Currently \(currentLabel)").font(.caption).foregroundStyle(Theme.textMuted)
                    }
                }
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
            }
            .disabled(saving)
            .task { known = (try? await APIClient.shared.speakers()) ?? [] }
            .confirmationDialog("Re-learn this voice?", isPresented: Binding(get: { relearn != nil }, set: { if !$0 { relearn = nil } }),
                                titleVisibility: .visible, presenting: relearn) { s in
                Button("Re-learn \(s.displayName)") {
                    tag(NameSpeakerRequest(speakerId: s.id, enroll: true, replaceVoiceprint: true))
                }
            } message: { s in
                Text("Replace \(s.displayName)'s saved voice with this speaker's, from this recording. Recordings uploaded afterwards are matched against the new one.")
            }
            .confirmationDialog("Not a participant?", isPresented: $confirmRemove, titleVisibility: .visible) {
                Button("Remove \(currentLabel) and their lines", role: .destructive) { run { await onNotParticipant() } }
            }
            .alert(item: $message) { m in
                Alert(title: Text(m.title), message: Text(m.message), dismissButton: .default(Text("OK")) {
                    if m.title == "Tagged" { dismiss() }
                })
            }
        }
    }

    private func submitName() {
        let n = name.trimmingCharacters(in: .whitespaces)
        guard !n.isEmpty else { return }
        tag(NameSpeakerRequest(name: n, enroll: enroll))
    }

    private func tag(_ req: NameSpeakerRequest) {
        saving = true
        Task {
            let outcome = await onTag(req)
            saving = false
            switch outcome {
            case .done: dismiss()
            case .notice(let n): message = AlertItem(title: "Tagged", message: n)
            case .failed(let e): message = AlertItem(title: "Could not tag speaker", message: e)
            }
        }
    }

    private func run(_ work: @escaping () async -> String?) {
        saving = true
        Task {
            let err = await work()
            saving = false
            if let err { message = AlertItem(title: "That did not work", message: err) } else { dismiss() }
        }
    }
}
