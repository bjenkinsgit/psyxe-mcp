import CoreLocation
import EventKit
import Foundation
import SQLite3

/// Shared EKEventStore wrapper with authorization handling.
enum EventKitStore {
    // Re-created after authorization to pick up freshly granted TCC permissions
    static var store = EKEventStore()

    /// Request full access to Reminders. Exits with code 2 on denial.
    static func authorize() async throws {
        let bundleId = Bundle.main.bundleIdentifier ?? "nil"
        let execPath = Bundle.main.executablePath ?? "nil"
        diag("bundle_id=\(bundleId) exe=\(execPath)")

        let status = EKEventStore.authorizationStatus(for: .reminder)
        diag("auth_status_before=\(status.rawValue) (\(statusName(status)))")

        let granted: Bool
        if #available(macOS 14.0, *) {
            granted = try await store.requestFullAccessToReminders()
        } else {
            granted = try await store.requestAccess(to: .reminder)
        }
        guard granted else {
            writeError("Reminders access not granted. Open System Settings > Privacy & Security > Reminders and enable this app.", exitCode: 2)
            exit(2)
        }

        let statusAfter = EKEventStore.authorizationStatus(for: .reminder)
        diag("auth_status_after=\(statusAfter.rawValue) (\(statusName(statusAfter)))")

        // After first authorization grant, the store's data sources may be stale.
        // Re-create to force a fresh connection to the EventKit database.
        store = EKEventStore()

        let sources = store.sources
        let calendars = store.calendars(for: .reminder)
        diag("sources=\(sources.count) calendars=\(calendars.count)")
    }

    /// Human-readable authorization status name.
    private static func statusName(_ s: EKAuthorizationStatus) -> String {
        switch s {
        case .notDetermined: return "notDetermined"
        case .restricted: return "restricted"
        case .denied: return "denied"
        case .fullAccess: return "fullAccess"
        case .writeOnly: return "writeOnly"
        @unknown default: return "unknown(\(s.rawValue))"
        }
    }

    /// Find a calendar (reminder list) by name. Case-insensitive.
    static func findCalendar(named name: String) -> EKCalendar? {
        store.calendars(for: .reminder).first {
            $0.title.caseInsensitiveCompare(name) == .orderedSame
        }
    }

    /// Find a reminder by title, optionally scoped to a list. Case-insensitive exact match.
    static func findReminder(named name: String, inList listName: String? = nil) async throws -> EKReminder? {
        var calendars: [EKCalendar]? = nil
        if let listName, !listName.isEmpty {
            guard let cal = findCalendar(named: listName) else { return nil }
            calendars = [cal]
        }

        let predicate = store.predicateForReminders(in: calendars)
        let reminders = try await withCheckedThrowingContinuation { (cont: CheckedContinuation<[EKReminder], Error>) in
            store.fetchReminders(matching: predicate) { result in
                cont.resume(returning: result ?? [])
            }
        }

        return reminders.first {
            $0.title?.caseInsensitiveCompare(name) == .orderedSame
        }
    }

    /// Fetch all reminders, optionally scoped to calendars.
    static func fetchAll(in calendars: [EKCalendar]? = nil) async throws -> [EKReminder] {
        let predicate = store.predicateForReminders(in: calendars)
        return try await withCheckedThrowingContinuation { (cont: CheckedContinuation<[EKReminder], Error>) in
            store.fetchReminders(matching: predicate) { result in
                cont.resume(returning: result ?? [])
            }
        }
    }

    /// Format a reminder's due date as ISO 8601 string, or empty string.
    static func formatDueDate(_ reminder: EKReminder) -> String {
        guard let components = reminder.dueDateComponents,
              let date = Calendar.current.date(from: components) else {
            return ""
        }
        let fmt = ISO8601DateFormatter()
        fmt.formatOptions = [.withInternetDateTime]
        return fmt.string(from: date)
    }

    /// Format a Date as ISO 8601 string.
    static func formatDate(_ date: Date?) -> String {
        guard let date else { return "" }
        let fmt = ISO8601DateFormatter()
        fmt.formatOptions = [.withInternetDateTime]
        return fmt.string(from: date)
    }

    /// Parse "YYYY-MM-DD" or "YYYY-MM-DD HH:MM" into DateComponents.
    /// Without time, defaults to hour=9. With time, uses the specified hour and minute.
    static func parseDateComponents(_ str: String) -> DateComponents? {
        let trimmed = str.trimmingCharacters(in: .whitespaces)
        let dateAndTime = trimmed.split(separator: " ", maxSplits: 1)

        let datePart = String(dateAndTime[0])
        let datePieces = datePart.split(separator: "-")
        guard datePieces.count == 3,
              let year = Int(datePieces[0]),
              let month = Int(datePieces[1]),
              let day = Int(datePieces[2]) else {
            return nil
        }

        var dc = DateComponents()
        dc.year = year
        dc.month = month
        dc.day = day

        if dateAndTime.count == 2 {
            let timePart = String(dateAndTime[1])
            let timePieces = timePart.split(separator: ":")
            if timePieces.count >= 2,
               let hour = Int(timePieces[0]),
               let minute = Int(timePieces[1]) {
                dc.hour = hour
                dc.minute = minute
            } else {
                dc.hour = 9
            }
        } else {
            dc.hour = 9
        }

        return dc
    }

    /// Build a snippet from notes: first 200 chars, newlines replaced with spaces.
    static func snippet(from notes: String?) -> String {
        guard let notes, !notes.isEmpty else { return "" }
        let clean = notes.replacingOccurrences(of: "\n", with: " ")
            .replacingOccurrences(of: "\r", with: " ")
        if clean.count > 200 {
            return String(clean.prefix(200))
        }
        return clean
    }

    /// Map EKReminder priority (0-9) to display priority.
    /// EK uses: 0=none, 1-4=high, 5=medium, 6-9=low.
    /// We output: 0=none, 1=high, 5=medium, 9=low (matching Apple's native values).
    static func mapPriority(_ p: Int) -> Int { p }

    /// Map priority string ("high", "medium", "low", "none") to EK int value.
    static func mapPriorityString(_ s: String) -> Int {
        switch s.lowercased() {
        case "high", "1": return 1
        case "medium", "med", "5": return 5
        case "low", "9": return 9
        case "none", "0", "": return 0
        default: return Int(s) ?? 0
        }
    }

    /// Format a reminder's start date as ISO 8601 string, or empty string.
    static func formatStartDate(_ reminder: EKReminder) -> String {
        guard let components = reminder.startDateComponents,
              let date = Calendar.current.date(from: components) else {
            return ""
        }
        let fmt = ISO8601DateFormatter()
        fmt.formatOptions = [.withInternetDateTime]
        return fmt.string(from: date)
    }

    /// Format a reminder's alarms as an array of dictionaries.
    static func formatAlarms(_ reminder: EKReminder) -> [[String: Any]] {
        guard let alarms = reminder.alarms, !alarms.isEmpty else { return [] }
        return alarms.compactMap { alarm in
            if let structuredLoc = alarm.structuredLocation,
               let geoLoc = structuredLoc.geoLocation {
                var dict: [String: Any] = [
                    "type": "location",
                    "title": structuredLoc.title ?? "",
                    "latitude": geoLoc.coordinate.latitude,
                    "longitude": geoLoc.coordinate.longitude,
                    "radius": structuredLoc.radius,
                ]
                switch alarm.proximity {
                case .enter: dict["proximity"] = "enter"
                case .leave: dict["proximity"] = "leave"
                default: dict["proximity"] = "none"
                }
                return dict
            } else {
                // Time-based alarm: offset is negative seconds before due date
                let offsetMinutes = Int(-alarm.relativeOffset / 60)
                return [
                    "type": "time",
                    "offset_minutes": offsetMinutes,
                ]
            }
        }
    }

    /// Format a reminder's first recurrence rule as a dictionary, or nil.
    static func formatRecurrence(_ reminder: EKReminder) -> [String: Any]? {
        guard let rules = reminder.recurrenceRules, let rule = rules.first else { return nil }
        var dict: [String: Any] = [
            "interval": rule.interval,
        ]
        switch rule.frequency {
        case .daily: dict["frequency"] = "daily"
        case .weekly: dict["frequency"] = "weekly"
        case .monthly: dict["frequency"] = "monthly"
        case .yearly: dict["frequency"] = "yearly"
        @unknown default: dict["frequency"] = "unknown"
        }
        if let end = rule.recurrenceEnd {
            if let endDate = end.endDate {
                let fmt = ISO8601DateFormatter()
                fmt.formatOptions = [.withInternetDateTime]
                dict["end_date"] = fmt.string(from: endDate)
            } else if end.occurrenceCount > 0 {
                dict["occurrence_count"] = end.occurrenceCount
            }
        }
        return dict
    }

    /// Apply all extended fields (url, location, start_date, location_alarm, time_alarm, recurrence)
    /// to a reminder from the input dictionary.
    static func applyCommonFields(_ reminder: EKReminder, from input: [String: Any]) {
        // URL (known Apple bug: may not persist for reminders)
        if let urlStr = input["url"] as? String {
            if urlStr.isEmpty {
                reminder.url = nil
            } else if let url = URL(string: urlStr) {
                reminder.url = url
            }
        }

        // Location (plain text stored in EKReminder.location)
        if let location = input["location"] as? String {
            reminder.location = location.isEmpty ? nil : location
        }

        // Start date
        if let startStr = input["start_date"] as? String {
            if startStr.isEmpty {
                reminder.startDateComponents = nil
            } else if let dc = parseDateComponents(startStr) {
                reminder.startDateComponents = dc
            }
        }

        // Location alarm
        if input["location_alarm"] is NSNull {
            // Remove all location-based alarms
            if let alarms = reminder.alarms {
                for alarm in alarms where alarm.structuredLocation != nil {
                    reminder.removeAlarm(alarm)
                }
            }
        } else if let locAlarm = input["location_alarm"] as? [String: Any],
                  let lat = locAlarm["latitude"] as? Double,
                  let lon = locAlarm["longitude"] as? Double {
            // Remove existing location alarms first
            if let alarms = reminder.alarms {
                for alarm in alarms where alarm.structuredLocation != nil {
                    reminder.removeAlarm(alarm)
                }
            }
            let title = locAlarm["title"] as? String ?? ""
            let radius = locAlarm["radius"] as? Double ?? 100.0
            let proximityStr = locAlarm["proximity"] as? String ?? "enter"

            let structuredLoc = EKStructuredLocation(title: title)
            structuredLoc.geoLocation = CLLocation(latitude: lat, longitude: lon)
            structuredLoc.radius = radius

            let alarm = EKAlarm()
            alarm.structuredLocation = structuredLoc
            alarm.proximity = proximityStr == "leave" ? .leave : .enter
            reminder.addAlarm(alarm)
        }

        // Time alarm
        if input["time_alarm"] is NSNull {
            // Remove all time-based alarms (those without structured location)
            if let alarms = reminder.alarms {
                for alarm in alarms where alarm.structuredLocation == nil {
                    reminder.removeAlarm(alarm)
                }
            }
        } else if let timeAlarm = input["time_alarm"] as? [String: Any],
                  let offsetMinutes = timeAlarm["offset_minutes"] as? Int {
            // Remove existing time alarms first
            if let alarms = reminder.alarms {
                for alarm in alarms where alarm.structuredLocation == nil {
                    reminder.removeAlarm(alarm)
                }
            }
            let alarm = EKAlarm(relativeOffset: TimeInterval(-offsetMinutes * 60))
            reminder.addAlarm(alarm)
        }

        // Recurrence
        if input["recurrence"] is NSNull {
            // Remove all recurrence rules
            if let rules = reminder.recurrenceRules {
                for rule in rules {
                    reminder.removeRecurrenceRule(rule)
                }
            }
        } else if let recInput = input["recurrence"] as? [String: Any],
                  let freqStr = recInput["frequency"] as? String {
            // Remove existing rules first
            if let rules = reminder.recurrenceRules {
                for rule in rules {
                    reminder.removeRecurrenceRule(rule)
                }
            }
            let freq: EKRecurrenceFrequency
            switch freqStr.lowercased() {
            case "daily": freq = .daily
            case "weekly": freq = .weekly
            case "monthly": freq = .monthly
            case "yearly": freq = .yearly
            default: return
            }
            let interval = recInput["interval"] as? Int ?? 1
            var end: EKRecurrenceEnd? = nil
            if let endDateStr = recInput["end_date"] as? String, !endDateStr.isEmpty {
                if let dc = parseDateComponents(endDateStr),
                   let date = Calendar.current.date(from: dc) {
                    end = EKRecurrenceEnd(end: date)
                }
            } else if let count = recInput["occurrence_count"] as? Int, count > 0 {
                end = EKRecurrenceEnd(occurrenceCount: count)
            }
            let rule = EKRecurrenceRule(
                recurrenceWith: freq,
                interval: interval,
                end: end
            )
            reminder.addRecurrenceRule(rule)
        }
    }

    // MARK: - Attachment lookup via Reminders SQLite database

    /// Fetch attachments for a reminder by its EventKit calendarItemExternalIdentifier.
    /// Queries the Reminders CoreData SQLite stores directly since EventKit
    /// does not expose attachments through its public API.
    static func fetchAttachments(forExternalId externalId: String) -> [[String: Any]] {
        let storesDir = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Group Containers/group.com.apple.reminders/Container_v1/Stores")

        guard let contents = try? FileManager.default.contentsOfDirectory(
            at: storesDir, includingPropertiesForKeys: nil
        ) else { return [] }

        let sqliteFiles = contents.filter { $0.pathExtension == "sqlite" }

        for dbURL in sqliteFiles {
            let results = queryAttachments(dbPath: dbURL.path, externalId: externalId)
            if !results.isEmpty { return results }
        }
        return []
    }

    private static func queryAttachments(dbPath: String, externalId: String) -> [[String: Any]] {
        var db: OpaquePointer?
        guard sqlite3_open_v2(dbPath, &db, SQLITE_OPEN_READONLY, nil) == SQLITE_OK else { return [] }
        defer { sqlite3_close(db) }

        // Join ZREMCDOBJECT (attachment) → ZREMCDREMINDER via ZREMINDER2,
        // filter by ZDACALENDARITEMUNIQUEIDENTIFIER and attachment entity types:
        //   22=REMCDAttachment, 23=REMCDFileAttachment, 24=REMCDAudioAttachment,
        //   25=REMCDImageAttachment, 26=REMCDURLAttachment
        let sql = """
            SELECT o.Z_ENT, o.ZURL, o.ZHOSTURL, o.ZFILENAME, o.ZUTI
            FROM ZREMCDOBJECT o
            JOIN ZREMCDREMINDER r ON o.ZREMINDER2 = r.Z_PK
            WHERE r.ZDACALENDARITEMUNIQUEIDENTIFIER = ?
              AND o.Z_ENT IN (22, 23, 24, 25, 26)
            """

        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return [] }
        defer { sqlite3_finalize(stmt) }

        sqlite3_bind_text(stmt, 1, (externalId as NSString).utf8String, -1, nil)

        var results: [[String: Any]] = []
        while sqlite3_step(stmt) == SQLITE_ROW {
            let entType = sqlite3_column_int(stmt, 0)
            let url = sqlite3_column_text(stmt, 1).flatMap { String(cString: $0) } ?? ""
            let hostUrl = sqlite3_column_text(stmt, 2).flatMap { String(cString: $0) } ?? ""
            let filename = sqlite3_column_text(stmt, 3).flatMap { String(cString: $0) } ?? ""
            let uti = sqlite3_column_text(stmt, 4).flatMap { String(cString: $0) } ?? ""

            let typeName: String
            switch entType {
            case 23: typeName = "file"
            case 24: typeName = "audio"
            case 25: typeName = "image"
            case 26: typeName = "url"
            default: typeName = "attachment"
            }

            var att: [String: Any] = ["type": typeName]
            if !url.isEmpty { att["url"] = url }
            if !hostUrl.isEmpty { att["host_url"] = hostUrl }
            if !filename.isEmpty { att["filename"] = filename }
            if !uti.isEmpty { att["uti"] = uti }

            results.append(att)
        }
        return results
    }

}

// MARK: - JSON I/O helpers

/// Read stdin as JSON, returning a dictionary. Empty stdin → empty dict.
func readInput() -> [String: Any] {
    let data = FileHandle.standardInput.readDataToEndOfFile()
    if data.isEmpty { return [:] }
    guard let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
        writeError("Invalid JSON input")
        exit(1)
    }
    return json
}

/// Write a JSON-serializable value to stdout and exit 0.
func writeOutput(_ value: Any) -> Never {
    let data = try! JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write("\n".data(using: .utf8)!)
    exit(0)
}

/// Write a diagnostic line to stderr (does not affect JSON protocol on stdout).
func diag(_ message: String) {
    FileHandle.standardError.write("[reminders-helper] \(message)\n".data(using: .utf8)!)
}

/// Check if EventKit is blocked by sandbox (0 data sources).
var eventKitBlocked: Bool {
    EventKitStore.store.sources.isEmpty
}

/// Run an AppleScript in-process via NSAppleScript.
/// Returns the string result, or nil on failure.
/// This runs WITHIN the helper's inherited sandbox, so the parent app's
/// Apple Events entitlements (temporary-exception.apple-events) apply.
func runInProcessAppleScript(_ source: String) -> String? {
    let script = NSAppleScript(source: source)
    var errorInfo: NSDictionary?
    let result = script?.executeAndReturnError(&errorInfo)
    if let error = errorInfo {
        let msg = (error[NSAppleScript.errorMessage] as? String) ?? "Unknown AppleScript error"
        diag("In-process AppleScript failed: \(msg)")
        return nil
    }
    return result?.stringValue
}

/// Write an error JSON to stderr and exit with the given code.
func writeError(_ message: String, exitCode: Int32 = 1) {
    let json: [String: Any] = ["error": true, "message": message]
    if let data = try? JSONSerialization.data(withJSONObject: json) {
        FileHandle.standardError.write(data)
        FileHandle.standardError.write("\n".data(using: .utf8)!)
    }
}
