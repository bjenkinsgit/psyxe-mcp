import EventKit
import Foundation

enum DeleteList {
    static func run(_ input: [String: Any]) async throws -> Never {
        guard let name = input["name"] as? String, !name.isEmpty else {
            writeError("Missing required 'name' field")
            exit(1)
        }

        guard let calendar = EventKitStore.findCalendar(named: name) else {
            writeError("Reminder list '\(name)' not found")
            exit(1)
        }

        try EventKitStore.store.removeCalendar(calendar, commit: true)
        writeOutput(["success": true, "message": "Deleted reminder list '\(name)'"])
    }
}
