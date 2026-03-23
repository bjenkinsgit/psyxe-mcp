import EventKit
import Foundation

enum SetURL {
    static func run(_ input: [String: Any]) async throws -> Never {
        guard let name = input["name"] as? String, !name.isEmpty else {
            writeError("Missing required 'name' field")
            exit(1)
        }
        guard let urlStr = input["url"] as? String, !urlStr.isEmpty else {
            writeError("Missing required 'url' field")
            exit(1)
        }

        let listName = input["list"] as? String
        guard let reminder = try await EventKitStore.findReminder(named: name, inList: listName) else {
            let scope = listName.map { " in list '\($0)'" } ?? ""
            writeError("Reminder '\(name)' not found\(scope)")
            exit(1)
        }

        guard let url = URL(string: urlStr) else {
            writeError("Invalid URL: '\(urlStr)'")
            exit(1)
        }

        reminder.url = url
        try EventKitStore.store.save(reminder, commit: true)

        // Verify the URL persisted by re-fetching
        let persisted = reminder.url?.absoluteString ?? ""
        let verified = persisted == urlStr

        writeOutput([
            "success": true,
            "message": "Set URL on reminder '\(reminder.title ?? name)'",
            "url": persisted,
            "verified": verified,
        ])
    }
}
