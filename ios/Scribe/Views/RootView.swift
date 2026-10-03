import SwiftUI

struct RootView: View {
    var body: some View {
        TabView {
            RecordView().tabItem { Label("Record", systemImage: "mic.fill") }
            LibraryView().tabItem { Label("Library", systemImage: "books.vertical") }
            SearchView().tabItem { Label("Search", systemImage: "magnifyingglass") }
            AskView().tabItem { Label("Ask", systemImage: "bubble.left.and.text.bubble.right") }
            SettingsView().tabItem { Label("Settings", systemImage: "gearshape") }
        }
    }
}
