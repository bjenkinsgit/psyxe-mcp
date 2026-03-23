import EventKit
import Foundation

enum GetReminder {
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

        var detail: [String: Any] = [
            "name": reminder.title ?? "",
            "list": reminder.calendar?.title ?? "",
            "due_date": EventKitStore.formatDueDate(reminder),
            "completed": reminder.isCompleted,
            "priority": EventKitStore.mapPriority(reminder.priority),
            "notes": reminder.notes ?? "",
            "created": EventKitStore.formatDate(reminder.creationDate),
            "modified": EventKitStore.formatDate(reminder.lastModifiedDate),
            "url": reminder.url?.absoluteString ?? "",
            "location": reminder.location ?? "",
            "start_date": EventKitStore.formatStartDate(reminder),
            "completion_date": EventKitStore.formatDate(reminder.completionDate),
            "alarms": EventKitStore.formatAlarms(reminder),
        ]
        if let recurrence = EventKitStore.formatRecurrence(reminder) {
            detail["recurrence"] = recurrence
        }
        let attachments = EventKitStore.fetchAttachments(forExternalId: reminder.calendarItemExternalIdentifier)
        if !attachments.isEmpty {
            detail["attachments"] = attachments
        }

        writeOutput(["success": true, "reminder": detail])
    }
}
