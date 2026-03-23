import EventKit
import Foundation

enum EditReminder {
    static func run(_ input: [String: Any]) async throws -> Never {
        guard let name = input["name"] as? String, !name.isEmpty else {
            writeError("Missing required 'name' field")
            exit(1)
        }

        let listName = input["list"] as? String
        guard let reminder = try await EventKitStore.findReminder(named: name, inList: listName) else {
            let scope = listName.map { " in list '\($0)'" } ?? ""
            writeError("Reminder '\(name)' not found\(scope)")
            exit(1)
        }

        // Update title
        if let newTitle = input["title"] as? String, !newTitle.isEmpty {
            reminder.title = newTitle
        }

        // Update due date (empty string = clear)
        if let dueDateStr = input["due_date"] as? String {
            if dueDateStr.isEmpty {
                reminder.dueDateComponents = nil
            } else if let dc = EventKitStore.parseDateComponents(dueDateStr) {
                reminder.dueDateComponents = dc
            }
        }

        // Update notes (append to existing content if present)
        if let notes = input["notes"] as? String {
            if notes.isEmpty {
                reminder.notes = nil
            } else if let existing = reminder.notes, !existing.isEmpty {
                // Avoid duplicating content already present
                if !existing.contains(notes) {
                    reminder.notes = existing + "\n" + notes
                }
            } else {
                reminder.notes = notes
            }
        }

        // Update priority
        if let priorityStr = input["priority"] as? String {
            reminder.priority = EventKitStore.mapPriorityString(priorityStr)
        } else if let priorityNum = input["priority"] as? Int {
            reminder.priority = priorityNum
        }

        // Move to a different list
        if let newListName = input["new_list"] as? String, !newListName.isEmpty {
            guard let newCal = EventKitStore.findCalendar(named: newListName) else {
                writeError("Target list '\(newListName)' not found")
                exit(1)
            }
            reminder.calendar = newCal
        }

        // Extended fields (url, location, start_date, alarms, recurrence)
        EventKitStore.applyCommonFields(reminder, from: input)

        try EventKitStore.store.save(reminder, commit: true)

        let displayName = reminder.title ?? name
        let calTitle = reminder.calendar.title
        writeOutput(["success": true, "message": "Updated reminder '\(displayName)'", "list": calTitle])
    }
}
