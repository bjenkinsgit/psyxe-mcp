import EventKit
import Foundation

enum ListLists {
    static func run(_ input: [String: Any]) async throws -> Never {
        let calendars = EventKitStore.store.calendars(for: .reminder)

        // If EventKit returned data, use it directly
        if !calendars.isEmpty {
            let sorted = calendars.sorted { $0.title.localizedCaseInsensitiveCompare($1.title) == .orderedAscending }
            let lists: [[String: Any]] = sorted.map { cal in
                ["name": cal.title, "id": cal.calendarIdentifier]
            }
            writeOutput(["count": lists.count, "lists": lists])
        }

        // EventKit returned 0 calendars (sandbox blocks XPC to remindd).
        // Fall back to NSAppleScript in-process — runs within the inherited
        // sandbox profile, so the parent app's Apple Events entitlements apply.
        diag("EventKit returned 0 calendars, trying in-process AppleScript fallback")

        let source = """
        tell application "Reminders"
            set listNames to name of every list
            set listIDs to id of every list
            set output to ""
            repeat with i from 1 to count of listNames
                set output to output & item i of listNames & "||" & item i of listIDs & linefeed
            end repeat
            return output
        end tell
        """

        let script = NSAppleScript(source: source)
        var errorInfo: NSDictionary?
        let result = script?.executeAndReturnError(&errorInfo)

        if let error = errorInfo {
            let msg = (error[NSAppleScript.errorMessage] as? String) ?? "Unknown AppleScript error"
            diag("AppleScript fallback failed: \(msg)")
            // Return empty rather than failing — caller can try osascript fallback
            writeOutput(["count": 0, "lists": [], "error": "applescript_failed", "message": msg] as [String: Any])
        }

        guard let output = result?.stringValue else {
            writeOutput(["count": 0, "lists": []] as [String: Any])
        }

        var lists: [[String: Any]] = []
        for line in output.components(separatedBy: "\n") where !line.isEmpty {
            let parts = line.components(separatedBy: "||")
            if parts.count >= 2 {
                lists.append(["name": parts[0], "id": parts[1]])
            }
        }
        lists.sort { ($0["name"] as? String ?? "").localizedCaseInsensitiveCompare($1["name"] as? String ?? "") == .orderedAscending }

        diag("AppleScript fallback returned \(lists.count) lists")
        writeOutput(["count": lists.count, "lists": lists])
    }
}
