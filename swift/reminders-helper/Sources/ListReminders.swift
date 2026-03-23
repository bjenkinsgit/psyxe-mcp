import EventKit
import Foundation

enum ListReminders {
    static func run(_ input: [String: Any]) async throws -> Never {
        guard let listName = input["list"] as? String, !listName.isEmpty else {
            writeError("Missing required 'list' field")
            exit(1)
        }

        let showCompleted = input["show_completed"] as? Bool ?? false

        // Try EventKit first
        if let cal = EventKitStore.findCalendar(named: listName) {
            let reminders = try await EventKitStore.fetchAll(in: [cal])
            let filtered = showCompleted ? reminders : reminders.filter { !$0.isCompleted }

            let results: [[String: Any]] = filtered.map { r in
                var entry: [String: Any] = [
                    "id": r.calendarItemExternalIdentifier ?? "",
                    "name": r.title ?? "",
                    "list": r.calendar?.title ?? "",
                    "due_date": EventKitStore.formatDueDate(r),
                    "completed": r.isCompleted,
                    "priority": EventKitStore.mapPriority(r.priority),
                    "notes": r.notes ?? "",
                    "snippet": EventKitStore.snippet(from: r.notes),
                    "url": r.url?.absoluteString ?? "",
                    "location": r.location ?? "",
                    "start_date": EventKitStore.formatStartDate(r),
                ]
                let attachments = EventKitStore.fetchAttachments(forExternalId: r.calendarItemExternalIdentifier)
                if !attachments.isEmpty {
                    entry["attachments"] = attachments
                }
                return entry
            }
            writeOutput(["list": cal.title, "count": results.count, "reminders": results])
        }

        // EventKit blocked by sandbox — try in-process AppleScript
        guard eventKitBlocked else {
            writeError("Reminder list '\(listName)' not found")
            exit(1)
        }

        diag("EventKit blocked (0 sources), trying in-process AppleScript for list '\(listName)'")

        let completedFilter = showCompleted ? "" : "whose completed is false"
        let source = """
        tell application "Reminders"
            try
                set theList to list "\(listName)"
            on error
                return "ERROR:List not found"
            end try
            set theReminders to every reminder in theList \(completedFilter)
            set output to ""
            repeat with r in theReminders
                set rName to name of r
                set rNotes to ""
                try
                    set rNotes to body of r
                end try
                set rDue to ""
                try
                    set rDue to (due date of r) as «class isot» as string
                end try
                set rCompleted to completed of r
                set rPriority to priority of r
                set output to output & rName & "||" & rNotes & "||" & rDue & "||" & rCompleted & "||" & rPriority & linefeed
            end repeat
            return output
        end tell
        """

        guard let output = runInProcessAppleScript(source) else {
            writeError("Reminder list '\(listName)' not found (AppleScript fallback failed)")
            exit(1)
        }

        if output.hasPrefix("ERROR:") {
            let msg = output.dropFirst(6)
            writeError(String(msg))
            exit(1)
        }

        var results: [[String: Any]] = []
        for line in output.components(separatedBy: "\n") where !line.isEmpty {
            let parts = line.components(separatedBy: "||")
            if parts.count >= 5 {
                results.append([
                    "id": "",
                    "name": parts[0],
                    "list": listName,
                    "due_date": parts[2],
                    "completed": parts[3] == "true",
                    "priority": Int(parts[4]) ?? 0,
                    "notes": parts[1],
                    "snippet": String(parts[1].prefix(200)),
                    "url": "",
                    "location": "",
                    "start_date": "",
                ])
            }
        }

        diag("AppleScript fallback returned \(results.count) reminders")
        writeOutput(["list": listName, "count": results.count, "reminders": results])
    }
}
