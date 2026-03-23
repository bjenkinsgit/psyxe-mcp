import AppKit
import Foundation

enum OpenReminders {
    static func run(_ input: [String: Any]) async throws -> Never {
        let listName = input["list"] as? String

        if let listName, !listName.isEmpty {
            // Open Reminders app with a specific list via URL scheme
            let encoded = listName.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? listName
            if let url = URL(string: "x-apple-reminderkit://REMCDList/\(encoded)") {
                NSWorkspace.shared.open(url)
            } else {
                // Fallback: just open the app
                NSWorkspace.shared.open(URL(string: "x-apple-reminderkit://")!)
            }
        } else {
            NSWorkspace.shared.open(URL(string: "x-apple-reminderkit://")!)
        }

        writeOutput(["success": true, "message": "Opened Reminders app"])
    }
}
