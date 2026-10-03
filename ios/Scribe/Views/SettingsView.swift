import SwiftUI

struct SettingsView: View {
    @Bindable private var settings = Settings.shared

    // Drafts, so a half-typed URL is not saved until Save or Test.
    @State private var baseURL = ""
    @State private var deviceKey = ""
    @State private var updateToken = ""
    @State private var participants = 2

    @State private var testing = false
    @State private var scanning = false
    @State private var testResult: TestResult?
    @State private var found: [DiscoveredServer] = []
    @State private var showFound = false
    @State private var alert: AlertItem?
    @State private var keyVisible = false

    enum TestResult: Equatable {
        case ok(String), failed(String), info(String)
    }

    var body: some View {
        NavigationStack {
            Form {
                connectionSection
                recordingSection
                deviceSection
            }
            .scrollContentBackground(.hidden)
            .background(Theme.bg)
            .navigationTitle("Settings")
            .onAppear(perform: loadDrafts)
            .confirmationDialog("Choose a server", isPresented: $showFound, titleVisibility: .visible) {
                ForEach(found) { server in
                    Button("\(server.name) — \(server.url)") { choose(server) }
                }
            }
            .alert(item: $alert) { a in Alert(title: Text(a.title), message: Text(a.message)) }
        }
    }

    // MARK: Sections

    private var connectionSection: some View {
        Section {
            LabeledField("Server URL") {
                TextField("https://scribe.example.ts.net", text: $baseURL)
                    .keyboardType(.URL).textInputAutocapitalization(.never).autocorrectionDisabled()
            }
            LabeledField("Device key") {
                HStack {
                    Group {
                        if keyVisible { TextField("Not needed on your tailnet", text: $deviceKey) }
                        else { SecureField("Not needed on your tailnet", text: $deviceKey) }
                    }
                    .textInputAutocapitalization(.never).autocorrectionDisabled()
                    Button { keyVisible.toggle() } label: {
                        Image(systemName: keyVisible ? "eye.slash" : "eye").foregroundStyle(Theme.textMuted)
                    }.buttonStyle(.plain)
                }
            }
            HStack {
                Button { Task { await testConnection() } } label: {
                    Label(testing ? "Testing…" : "Test connection", systemImage: "antenna.radiowaves.left.and.right")
                }.disabled(testing)
                Spacer()
                Button { Task { await findServer() } } label: {
                    Label(scanning ? "Searching…" : "Find server", systemImage: "wifi")
                }.disabled(scanning)
            }
            .buttonStyle(.borderless)
            if let r = testResult { testResultView(r) }
            Button("Save", action: save).bold()
        } header: {
            Text("Connection")
        } footer: {
            Text("Find server looks for your Scribe server on this Wi-Fi network and fills in its tailnet address. Scanning the QR code the installer prints does the same, with the key.")
        }
    }

    private var recordingSection: some View {
        Section {
            Picker("Audio quality", selection: $settings.audioQuality) {
                Text("Low").tag(Settings.AudioQuality.low)
                Text("Medium").tag(Settings.AudioQuality.medium)
                Text("High").tag(Settings.AudioQuality.high)
            }
            Stepper(value: $participants, in: 1...20) {
                HStack {
                    Text("Default participants")
                    Spacer()
                    Text("\(participants)").foregroundStyle(Theme.textMuted).monospacedDigit()
                }
            }
            .onChange(of: participants) { _, v in settings.defaultParticipants = v }
            Toggle("Reduce motion", isOn: $settings.reduceMotion)
        } header: {
            Text("Recording")
        } footer: {
            Text("Every option records 16 kHz mono AAC; only the bitrate changes. The participant count pre-fills the Record screen and helps speaker detection.")
        }
    }

    private var deviceSection: some View {
        Section("This device") {
            NavigationLink { SpeakersView() } label: { Label("Speakers", systemImage: "person.2.wave.2") }
            LabeledField("Update token") {
                SecureField("For server self-update only", text: $updateToken)
                    .textInputAutocapitalization(.never).autocorrectionDisabled()
            }
            HStack {
                Text("Device ID")
                Spacer()
                Text(settings.deviceId).foregroundStyle(Theme.textMuted).font(.footnote.monospaced())
                    .textSelection(.enabled)
            }
            HStack {
                Text("Version")
                Spacer()
                Text(Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "?")
                    .foregroundStyle(Theme.textMuted)
            }
        }
    }

    @ViewBuilder private func testResultView(_ r: TestResult) -> some View {
        switch r {
        case .ok(let s): Label(s, systemImage: "checkmark.circle.fill").foregroundStyle(Theme.ready).font(.footnote)
        case .failed(let s): Label(s, systemImage: "xmark.octagon.fill").foregroundStyle(Theme.accentDeep).font(.footnote)
        case .info(let s): Label(s, systemImage: "info.circle").foregroundStyle(Theme.textMuted).font(.footnote)
        }
    }

    // MARK: Actions

    private func loadDrafts() {
        baseURL = settings.baseURL
        deviceKey = settings.deviceKey
        updateToken = settings.updateToken
        participants = settings.defaultParticipants
    }

    private func save() {
        var url = baseURL.trimmingCharacters(in: .whitespacesAndNewlines)
        while url.hasSuffix("/") { url.removeLast() }
        settings.baseURL = url
        settings.deviceKey = deviceKey.trimmingCharacters(in: .whitespacesAndNewlines)
        settings.updateToken = updateToken.trimmingCharacters(in: .whitespacesAndNewlines)
        baseURL = url
    }

    /// Health first, which needs no credential, then an authenticated call:
    /// without the second, a wrong key reports "connected" and then fails on
    /// every real request.
    private func testConnection() async {
        guard !baseURL.trimmingCharacters(in: .whitespaces).isEmpty else {
            alert = AlertItem(title: "No URL", message: "Enter a server URL first.")
            return
        }
        save() // test what is in the box, not a stale saved value
        testing = true
        defer { testing = false }
        do {
            let health = try await APIClient.shared.health()
            _ = try await APIClient.shared.listRecordings(limit: 1)
            testResult = .ok("Connected · v\(health.version) · DB \(health.db) · access OK")
        } catch let e as APIError where e.isUnauthorized {
            testResult = .failed(deviceKey.isEmpty
                ? "No device key, and the server did not admit this phone by its tailnet identity. Paste a key from the server, or sign in to Tailscale as the server's owner."
                : "The server rejected this device key. Copy the one the installer printed.")
        } catch let e as URLError {
            testResult = .failed("Could not reach the server (\(e.localizedDescription)). Is Tailscale connected on this phone?")
        } catch {
            testResult = .failed(error.localizedDescription)
        }
    }

    private func findServer() async {
        scanning = true
        testResult = nil
        defer { scanning = false }
        do {
            let servers = try await Discovery.browse()
            switch servers.count {
            case 0:
                alert = AlertItem(title: "No servers found",
                                  message: "Nothing answered on this network. Check the phone is on the same Wi-Fi as the server, and that the server was installed with LAN discovery on.")
            case 1: choose(servers[0])
            default: found = servers; showFound = true
            }
        } catch {
            alert = AlertItem(title: "Search failed", message: error.localizedDescription)
        }
    }

    private func choose(_ server: DiscoveredServer) {
        baseURL = server.url
        // A server that admits by tailnet identity wants no key; a stale one
        // left in the field would send a credential it never asked for.
        if !server.needsKey { deviceKey = "" }
        save()
        testResult = .info(server.needsKey
            ? "Found \(server.name) — paste its device key, then Save."
            : "Found \(server.name) — no device key needed. Test the connection.")
    }
}

/// A field with a small caption above it.
struct LabeledField<Content: View>: View {
    let label: String
    @ViewBuilder let content: Content
    init(_ label: String, @ViewBuilder content: () -> Content) {
        self.label = label
        self.content = content()
    }
    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(label.uppercased()).font(.caption2.weight(.semibold)).foregroundStyle(Theme.textMuted)
            content
        }
    }
}

struct AlertItem: Identifiable {
    let id = UUID()
    let title: String
    let message: String
}
