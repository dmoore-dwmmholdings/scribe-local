import SwiftUI

struct RootView: View {
    var body: some View {
        TabView {
            RecordView().tabItem { Label("Record", systemImage: "mic.fill") }
            LibraryView().tabItem { Label("Library", systemImage: "books.vertical") }
            placeholder("Search").tabItem { Label("Search", systemImage: "magnifyingglass") }
            placeholder("Ask").tabItem { Label("Ask", systemImage: "bubble.left.and.text.bubble.right") }
            SettingsView().tabItem { Label("Settings", systemImage: "gearshape") }
        }
    }

    private func placeholder(_ title: String) -> some View {
        ZStack {
            Theme.bg.ignoresSafeArea()
            Text(title).font(.title2.weight(.semibold)).foregroundStyle(Theme.textPrimary)
        }
    }
}
