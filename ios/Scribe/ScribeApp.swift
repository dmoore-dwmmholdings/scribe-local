import SwiftUI

@main
struct ScribeApp: App {
    @State private var pairing: PairingPayload?

    var body: some Scene {
        WindowGroup {
            RootView()
                .preferredColorScheme(.dark)
                .tint(Theme.accent)
                // `scribe://pair?url=…&key=…`, from the QR code the installer
                // prints. Confirmed before it replaces a saved server.
                .onOpenURL { url in pairing = PairingPayload(url: url) }
                .alert("Pair with this server?", isPresented: Binding(
                    get: { pairing != nil }, set: { if !$0 { pairing = nil } }
                ), presenting: pairing) { p in
                    Button("Pair") {
                        Settings.shared.baseURL = p.baseURL
                        Settings.shared.deviceKey = p.deviceKey
                        pairing = nil
                    }
                    Button("Cancel", role: .cancel) { pairing = nil }
                } message: { p in
                    Text(p.summary)
                }
        }
    }
}
