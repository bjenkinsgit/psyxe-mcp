import EventKit
import Foundation

enum DeleteReminder {
    static func run(_ input: [String: Any]) async throws -> Never {
        guard let name = input["name"] as? String, !name.isEmpty else {
            writeError("Missing required 'name' field")
            exit(1)
        }

        let listName = input["list"] as? String

        guard let reminder = try await EventKitStore.findReminder(named: name, inList: listName) else {
            writeError("Reminder '\(name)' not found")
            exit(1)
        }

        try EventKitStore.store.remove(reminder, commit: true)
        writeOutput(["success": true, "message": "Deleted reminder '\(name)'"])
    }
}
