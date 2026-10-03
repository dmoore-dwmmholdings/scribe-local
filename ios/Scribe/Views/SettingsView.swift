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
            TabScreen("Settings") {
                VStack(alignment: .leading, spacing: 10) {
                    SectionLabel("Connection")
                    connectionCard
                    Text("Find server looks for your Scribe server on this Wi-Fi network and fills in its tailnet address. Scanning the QR code the installer prints does the same, with the key.")
                        .font(.caption).foregroundStyle(Theme.textDim).padding(.horizontal, 6)

                    SectionLabel("Recording").padding(.top, 14)
                    recordingCard

                    SectionLabel("This device").padding(.top, 14)
                    deviceCard
                }
            }
            .confirmationDialog("Choose a server", isPresented: $showFound, titleVisibility: .visible) {
                ForEach(found) { server in
                    Button("\(server.name) — \(server.url)") { choose(server) }
                }
            }
            .alert(item: $alert) { a in Alert(title: Text(a.title), message: Text(a.message)) }
            .onAppear(perform: loadDrafts)
        }
    }

    // MARK: Cards

    private var connectionCard: some View {
        Card {
            VStack(alignment: .leading, spacing: 14) {
                FieldBox(label: "Server URL") {
                    TextField("", text: $baseURL, prompt: Text("https://scribe.example.ts.net").foregroundColor(Theme.textDim))
                        .keyboardType(.URL).textInputAutocapitalization(.never).autocorrectionDisabled()
                }
                FieldBox(label: "Device key") {
                    HStack {
                        Group {
                            if keyVisible { TextField("", text: $deviceKey, prompt: Text("Not needed on your tailnet").foregroundColor(Theme.textDim)) }
                            else { SecureField("", text: $deviceKey, prompt: Text("Not needed on your tailnet").foregroundColor(Theme.textDim)) }
                        }
                        .textInputAutocapitalization(.never).autocorrectionDisabled()
                        Button { keyVisible.toggle() } label: {
                            Image(systemName: keyVisible ? "eye.slash" : "eye").foregroundStyle(Theme.textMuted)
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel(keyVisible ? "Hide key" : "Show key")
                    }
                }
                HStack(spacing: 10) {
                    PillButton(title: testing ? "Testing…" : "Test connection", icon: "antenna.radiowaves.left.and.right",
                               prominent: false, busy: testing) { Task { await testConnection() } }
                    PillButton(title: scanning ? "Searching…" : "Find server", icon: "wifi",
                               prominent: false, busy: scanning) { Task { await findServer() } }
                }
                if let r = testResult { testResultView(r) }
                PillButton(title: "Save", icon: "checkmark", action: save)
            }
        }
    }

    private var recordingCard: some View {
        Card(padding: 14) {
            VStack(spacing: 0) {
                HStack {
                    Text("Audio quality").foregroundStyle(Theme.textPrimary)
                    Spacer()
                    Picker("Audio quality", selection: $settings.audioQuality) {
                        Text("Low").tag(Settings.AudioQuality.low)
                        Text("Medium").tag(Settings.AudioQuality.medium)
                        Text("High").tag(Settings.AudioQuality.high)
                    }
                    .pickerStyle(.segmented)
                    .frame(width: 190)
                }
                .padding(.vertical, 8)
                Hairline()
                HStack {
                    Text("Default participants").foregroundStyle(Theme.textPrimary)
                    Spacer()
                    Text("\(participants)").font(.mono(15)).foregroundStyle(Theme.textMuted)
                    Stepper("", value: $participants, in: 1...20).labelsHidden()
                }
                .padding(.vertical, 8)
                .onChange(of: participants) { _, v in settings.defaultParticipants = v }
                Hairline()
                Toggle(isOn: $settings.reduceMotion) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("Reduce motion").foregroundStyle(Theme.textPrimary)
                        Text("Still the orb and the edge glow").font(.caption).foregroundStyle(Theme.textMuted)
                    }
                }
                .tint(Theme.accent)
                .padding(.vertical, 8)
            }
        }
    }

    private var deviceCard: some View {
        Card(padding: 14) {
            VStack(alignment: .leading, spacing: 0) {
                NavigationLink { SpeakersView() } label: { CardRow(icon: "person.2.wave.2", label: "Speakers") }
                    .buttonStyle(.plain)
                Hairline()
                FieldBox(label: "Update token") {
                    SecureField("", text: $updateToken, prompt: Text("For server self-update only").foregroundColor(Theme.textDim))
                        .textInputAutocapitalization(.never).autocorrectionDisabled()
                }
                .padding(.vertical, 12)
                Hairline()
                HStack {
                    Text("Device ID").foregroundStyle(Theme.textPrimary)
                    Spacer()
                    Text(settings.deviceId).font(.mono(12)).foregroundStyle(Theme.textMuted).textSelection(.enabled)
                }
                .padding(.vertical, 12)
                Hairline()
                HStack {
                    Text("Version").foregroundStyle(Theme.textPrimary)
                    Spacer()
                    Text(Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "?")
                        .font(.mono(12)).foregroundStyle(Theme.textMuted)
                }
                .padding(.vertical, 12)
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
            testResult = health.dbOK
                ? .ok("Connected · v\(health.version) · database OK · access OK")
                : .failed("The server answered (v\(health.version)) but its database did not. Check Docker on the server.")
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
