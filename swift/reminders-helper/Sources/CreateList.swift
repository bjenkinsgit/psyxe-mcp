import EventKit
import Foundation

enum CreateList {
    static func run(_ input: [String: Any]) async throws -> Never {
        guard let name = input["name"] as? String, !name.isEmpty else {
            writeError("Missing required 'name' field")
            exit(1)
        }

        // Check if list already exists
        if EventKitStore.findCalendar(named: name) != nil {
            writeError("Reminder list '\(name)' already exists")
            exit(1)
        }

        let calendar = EKCalendar(for: .reminder, eventStore: EventKitStore.store)
        calendar.title = name

        // Use the default source for reminders
        if let defaultCal = EventKitStore.store.defaultCalendarForNewReminders() {
            calendar.source = defaultCal.source
        } else {
            // Fallback: find a local or iCloud source
            let sources = EventKitStore.store.sources
            if let local = sources.first(where: { $0.sourceType == .local }) {
                calendar.source = local
            } else if let icloud = sources.first(where: { $0.sourceType == .calDAV }) {
                calendar.source = icloud
            } else {
                writeError("No suitable source found for creating reminder lists")
                exit(1)
            }
        }

        try EventKitStore.store.saveCalendar(calendar, commit: true)
        writeOutput(["success": true, "message": "Created reminder list '\(name)'"])
    }
}
