import EventKit
import Foundation

enum CreateBatch {
    /// Chunk size for batched saves to control memory pressure.
    private static let chunkSize = 250

    static func run(_ input: [String: Any]) async throws -> Never {
        guard let listName = input["list"] as? String, !listName.isEmpty else {
            writeError("Missing required 'list' field")
            exit(1)
        }

        guard let items = input["items"] as? [[String: Any]], !items.isEmpty else {
            writeError("Missing required 'items' array")
            exit(1)
        }

        guard let cal = EventKitStore.findCalendar(named: listName) else {
            writeError("Reminder list '\(listName)' not found")
            exit(1)
        }

        var created = 0
        var i = 0

        while i < items.count {
            let end = min(i + chunkSize, items.count)

            try autoreleasepool {
                for j in i..<end {
                    let item = items[j]
                    guard let title = item["title"] as? String, !title.isEmpty else { continue }

                    let reminder = EKReminder(eventStore: EventKitStore.store)
                    reminder.title = title
                    reminder.calendar = cal

                    if let notes = item["notes"] as? String, !notes.isEmpty {
                        reminder.notes = notes
                    }

                    if let dueDateStr = item["due_date"] as? String, !dueDateStr.isEmpty {
                        if let dc = EventKitStore.parseDateComponents(dueDateStr) {
                            reminder.dueDateComponents = dc
                        }
                    }

                    if let priorityStr = item["priority"] as? String {
                        reminder.priority = EventKitStore.mapPriorityString(priorityStr)
                    } else if let priorityNum = item["priority"] as? Int {
                        reminder.priority = priorityNum
                    }

                    // Extended fields (url, location, start_date, alarms, recurrence)
                    EventKitStore.applyCommonFields(reminder, from: item)

                    try EventKitStore.store.save(reminder, commit: false)
                    created += 1
                }

                // Commit once per chunk
                try EventKitStore.store.commit()
            }

            i = end
        }

        writeOutput(["success": true, "message": "Created \(created) reminders in '\(cal.title)'"])
    }

}
