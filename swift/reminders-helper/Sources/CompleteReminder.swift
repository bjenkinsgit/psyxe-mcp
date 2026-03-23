import EventKit
import Foundation

enum CompleteReminder {
    static func run(_ input: [String: Any]) async throws -> Never {
        guard let name = input["name"] as? String, !name.isEmpty else {
            writeError("Missing required 'name' field")
            exit(1)
        }

        let listName = input["list"] as? String
        let completed = input["completed"] as? Bool ?? true

        guard let reminder = try await EventKitStore.findReminder(named: name, inList: listName) else {
            writeError("Reminder '\(name)' not found")
            exit(1)
        }

        reminder.isCompleted = completed
        try EventKitStore.store.save(reminder, commit: true)

        let action = completed ? "Completed" : "Marked incomplete"
        writeOutput(["success": true, "message": "\(action) reminder '\(name)'"])
    }
}
