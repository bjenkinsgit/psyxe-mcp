import EventKit
import Foundation

enum EditBatch {
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

        // Fetch all reminders in the list once via predicate
        let allReminders = try await EventKitStore.fetchAll(in: [cal])

        var updated = 0
        var errors: [String] = []
        var i = 0

        while i < items.count {
            let end = min(i + chunkSize, items.count)

            try autoreleasepool {
                for j in i..<end {
                    let item = items[j]
                    guard let name = item["name"] as? String, !name.isEmpty else { continue }

                    guard let reminder = allReminders.first(where: {
                        $0.title?.caseInsensitiveCompare(name) == .orderedSame
                    }) else {
                        errors.append("Reminder '\(name)' not found in '\(listName)'")
                        continue
                    }

                    // Update title
                    if let newTitle = item["title"] as? String, !newTitle.isEmpty {
                        reminder.title = newTitle
                    }

                    // Update due date
                    if let dueDateStr = item["due_date"] as? String {
                        if dueDateStr.isEmpty {
                            reminder.dueDateComponents = nil
                        } else if let dc = EventKitStore.parseDateComponents(dueDateStr) {
                            reminder.dueDateComponents = dc
                        }
                    }

                    // Update notes (append to existing content if present)
                    if let notes = item["notes"] as? String {
                        if notes.isEmpty {
                            reminder.notes = nil
                        } else if let existing = reminder.notes, !existing.isEmpty {
                            if !existing.contains(notes) {
                                reminder.notes = existing + "\n" + notes
                            }
                        } else {
                            reminder.notes = notes
                        }
                    }

                    // Update priority
                    if let priorityStr = item["priority"] as? String {
                        reminder.priority = EventKitStore.mapPriorityString(priorityStr)
                    } else if let priorityNum = item["priority"] as? Int {
                        reminder.priority = priorityNum
                    }

                    // Extended fields (url, location, start_date, alarms, recurrence)
                    EventKitStore.applyCommonFields(reminder, from: item)

                    try EventKitStore.store.save(reminder, commit: false)
                    updated += 1
                }

                // Commit once per chunk
                try EventKitStore.store.commit()
            }

            i = end
        }

        var message = "Updated \(updated) reminder\(updated == 1 ? "" : "s") in '\(cal.title)'"
        if !errors.isEmpty {
            message += ". Errors: " + errors.joined(separator: "; ")
        }
        writeOutput(["success": true, "message": message])
    }

}
