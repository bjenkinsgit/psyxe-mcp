import EventKit
import Foundation

enum Search {
    static func run(_ input: [String: Any]) async throws -> Never {
        guard let query = input["query"] as? String, !query.isEmpty else {
            writeError("Missing required 'query' field")
            exit(1)
        }

        let listName = input["list"] as? String
        var calendars: [EKCalendar]? = nil
        if let listName, !listName.isEmpty {
            guard let cal = EventKitStore.findCalendar(named: listName) else {
                writeOutput(["count": 0, "results": [] as [Any]])
            }
            calendars = [cal]
        }

        let reminders = try await EventKitStore.fetchAll(in: calendars)
        let matches = reminders.filter { r in
            r.title?.localizedCaseInsensitiveContains(query) == true ||
            r.notes?.localizedCaseInsensitiveContains(query) == true ||
            r.location?.localizedCaseInsensitiveContains(query) == true
        }

        let results: [[String: Any]] = matches.map { r in
            var entry: [String: Any] = [
                "id": r.calendarItemExternalIdentifier ?? "",
                "name": r.title ?? "",
                "list": r.calendar?.title ?? "",
                "due_date": EventKitStore.formatDueDate(r),
                "completed": r.isCompleted,
                "priority": EventKitStore.mapPriority(r.priority),
                "snippet": EventKitStore.snippet(from: r.notes),
                "url": r.url?.absoluteString ?? "",
                "location": r.location ?? "",
            ]
            let attachments = EventKitStore.fetchAttachments(forExternalId: r.calendarItemExternalIdentifier)
            if !attachments.isEmpty {
                entry["attachments"] = attachments
            }
            return entry
        }

        writeOutput(["count": results.count, "results": results])
    }
}
