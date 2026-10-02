import SwiftUI

/// Placeholder screen that proves the AppCore FFI surface works in-app:
/// it creates a game session through the UniFFI Swift bindings and shows
/// the initial board state (FEN) and the starting player rating.
struct ContentView: View {
    @State private var boardState: String?
    @State private var rating: Double?
    @State private var errorMessage: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("MyApp")
                .font(.largeTitle.bold())

            if let boardState, let rating {
                VStack(alignment: .leading, spacing: 8) {
                    Text("Board state (FEN)")
                        .font(.headline)
                    Text(boardState)
                        .font(.system(.body, design: .monospaced))
                        .textSelection(.enabled)

                    HStack(spacing: 8) {
                        Text("Player rating")
                            .font(.headline)
                        Text(String(format: "%.1f", rating))
                            .font(.title2)
                    }
                }
            } else if let errorMessage {
                VStack(alignment: .leading, spacing: 8) {
                    Text("FFI error")
                        .font(.headline)
                        .foregroundStyle(.red)
                    Text(errorMessage)
                }
            } else {
                ProgressView("Creating session…")
            }

            Spacer()
        }
        .padding()
        .onAppear(perform: loadSession)
    }

    private func loadSession() {
        do {
            let session = newGameSession(initialRating: 1500.0)
            boardState = try session.getBoardState()
            rating = try session.getCurrentRating()
        } catch {
            errorMessage = "\(error)"
        }
    }
}
