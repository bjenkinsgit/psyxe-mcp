import EventKit
import Foundation

enum CreateReminder {
    static func run(_ input: [String: Any]) async throws -> Never {
        guard let title = input["title"] as? String, !title.isEmpty else {
            writeError("Missing required 'title' field")
            exit(1)
        }

        let reminder = EKReminder(eventStore: EventKitStore.store)
        reminder.title = title

        // List
        if let listName = input["list"] as? String, !listName.isEmpty {
            guard let cal = EventKitStore.findCalendar(named: listName) else {
                writeError("Reminder list '\(listName)' not found")
                exit(1)
            }
            reminder.calendar = cal
        } else {
            reminder.calendar = EventKitStore.store.defaultCalendarForNewReminders()
        }

        // Due date
        if let dueDateStr = input["due_date"] as? String, !dueDateStr.isEmpty {
            if let dc = EventKitStore.parseDateComponents(dueDateStr) {
                reminder.dueDateComponents = dc
            }
        }

        // Notes
        if let notes = input["notes"] as? String, !notes.isEmpty {
            reminder.notes = notes
        }

        // Priority (accept both string and number)
        if let priorityStr = input["priority"] as? String {
            reminder.priority = EventKitStore.mapPriorityString(priorityStr)
        } else if let priorityNum = input["priority"] as? Int {
            reminder.priority = priorityNum
        }

        // Extended fields (url, location, start_date, alarms, recurrence)
        EventKitStore.applyCommonFields(reminder, from: input)

        try EventKitStore.store.save(reminder, commit: true)
        writeOutput(["success": true, "message": "Created reminder '\(title)' in \(reminder.calendar.title)"])
    }
}
