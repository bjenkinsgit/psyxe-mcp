import Contacts
import Foundation

/// Shared CNContactStore wrapper with authorization handling and I/O helpers.
enum ContactStore {
    // Re-created after authorization to pick up freshly granted TCC permissions
    static var store = CNContactStore()

    /// Request access to Contacts. Exits with code 2 on denial.
    static func authorize() async throws {
        let status = CNContactStore.authorizationStatus(for: .contacts)
        diag("auth_status_before=\(status.rawValue) (\(statusName(status)))")

        let granted: Bool
        if #available(macOS 14.0, *) {
            granted = try await store.requestAccess(for: .contacts)
        } else {
            granted = try await withCheckedThrowingContinuation { cont in
                store.requestAccess(for: .contacts) { granted, error in
                    if let error {
                        cont.resume(throwing: error)
                    } else {
                        cont.resume(returning: granted)
                    }
                }
            }
        }
        guard granted else {
            writeError("Contacts access not granted. Open System Settings > Privacy & Security > Contacts and enable this app.", exitCode: 2)
            exit(2)
        }

        let statusAfter = CNContactStore.authorizationStatus(for: .contacts)
        diag("auth_status_after=\(statusAfter.rawValue) (\(statusName(statusAfter)))")

        // After first authorization grant, the store's data sources may be stale.
        // Re-create to force a fresh connection to the Contacts database.
        store = CNContactStore()
    }

    /// Human-readable authorization status name.
    private static func statusName(_ s: CNAuthorizationStatus) -> String {
        switch s {
        case .notDetermined: return "notDetermined"
        case .restricted: return "restricted"
        case .denied: return "denied"
        case .authorized: return "authorized"
        @unknown default: return "unknown(\(s.rawValue))"
        }
    }

    /// Keys to fetch for list/search results (summary).
    static let summaryKeys: [CNKeyDescriptor] = [
        CNContactIdentifierKey as CNKeyDescriptor,
        CNContactGivenNameKey as CNKeyDescriptor,
        CNContactFamilyNameKey as CNKeyDescriptor,
        CNContactOrganizationNameKey as CNKeyDescriptor,
        CNContactPhoneNumbersKey as CNKeyDescriptor,
        CNContactEmailAddressesKey as CNKeyDescriptor,
    ]

    /// Keys to fetch for full detail.
    /// Note: CNContactNoteKey is excluded — on macOS Sonoma+ it requires Full Contacts Access
    /// and causes Cocoa error 134092 if the app only has basic access.
    static let detailKeys: [CNKeyDescriptor] = [
        CNContactIdentifierKey as CNKeyDescriptor,
        CNContactGivenNameKey as CNKeyDescriptor,
        CNContactMiddleNameKey as CNKeyDescriptor,
        CNContactFamilyNameKey as CNKeyDescriptor,
        CNContactNicknameKey as CNKeyDescriptor,
        CNContactOrganizationNameKey as CNKeyDescriptor,
        CNContactJobTitleKey as CNKeyDescriptor,
        CNContactDepartmentNameKey as CNKeyDescriptor,
        CNContactPhoneNumbersKey as CNKeyDescriptor,
        CNContactEmailAddressesKey as CNKeyDescriptor,
        CNContactPostalAddressesKey as CNKeyDescriptor,
        CNContactBirthdayKey as CNKeyDescriptor,
        CNContactUrlAddressesKey as CNKeyDescriptor,
        CNContactSocialProfilesKey as CNKeyDescriptor,
        CNContactInstantMessageAddressesKey as CNKeyDescriptor,
        CNContactImageDataAvailableKey as CNKeyDescriptor,
        CNContactImageDataKey as CNKeyDescriptor,
    ]

    /// Format a contact as a summary dictionary.
    static func summaryDict(_ contact: CNContact) -> [String: Any] {
        let fullName = [contact.givenName, contact.familyName]
            .filter { !$0.isEmpty }
            .joined(separator: " ")
        return [
            "id": contact.identifier,
            "name": fullName,
            "organization": contact.organizationName,
            "phone": contact.phoneNumbers.first.map { $0.value.stringValue } ?? "",
            "email": (contact.emailAddresses.first?.value as String?) ?? "",
        ]
    }

    /// Format a contact as a full detail dictionary.
    static func detailDict(_ contact: CNContact) -> [String: Any] {
        let fullName = [contact.givenName, contact.familyName]
            .filter { !$0.isEmpty }
            .joined(separator: " ")

        let phones: [[String: String]] = contact.phoneNumbers.map { lv in
            [
                "label": lv.label.map { CNLabeledValue<NSString>.localizedString(forLabel: $0) } ?? "other",
                "value": lv.value.stringValue,
            ]
        }

        let emails: [[String: String]] = contact.emailAddresses.map { lv in
            [
                "label": lv.label.map { CNLabeledValue<NSString>.localizedString(forLabel: $0) } ?? "other",
                "value": lv.value as String,
            ]
        }

        let addresses: [[String: String]] = contact.postalAddresses.map { lv in
            let formatter = CNPostalAddressFormatter()
            return [
                "label": lv.label.map { CNLabeledValue<NSString>.localizedString(forLabel: $0) } ?? "other",
                "value": formatter.string(from: lv.value),
            ]
        }

        let urls: [[String: String]] = contact.urlAddresses.map { lv in
            [
                "label": lv.label.map { CNLabeledValue<NSString>.localizedString(forLabel: $0) } ?? "other",
                "value": lv.value as String,
            ]
        }

        let socials: [[String: String]] = contact.socialProfiles.map { lv in
            [
                "service": lv.value.service,
                "username": lv.value.username,
                "url": lv.value.urlString,
            ]
        }

        let instantMessages: [[String: String]] = contact.instantMessageAddresses.map { lv in
            [
                "service": lv.value.service,
                "username": lv.value.username,
                "label": lv.label.map { CNLabeledValue<NSString>.localizedString(forLabel: $0) } ?? "other",
            ]
        }

        var dict: [String: Any] = [
            "id": contact.identifier,
            "given_name": contact.givenName,
            "middle_name": contact.middleName,
            "family_name": contact.familyName,
            "nickname": contact.nickname,
            "name": fullName,
            "organization": contact.organizationName,
            "job_title": contact.jobTitle,
            "department": contact.departmentName,
            "phones": phones,
            "emails": emails,
            "addresses": addresses,
            "urls": urls,
            "social_profiles": socials,
            "instant_messages": instantMessages,
            "has_image": contact.imageDataAvailable,
        ]

        // Note requires Full Contacts Access on macOS Sonoma+ — only include if fetchable
        if contact.isKeyAvailable(CNContactNoteKey) {
            dict["note"] = contact.note
        }

        if let birthday = contact.birthday {
            var parts: [String] = []
            if let year = birthday.year { parts.append(String(format: "%04d", year)) }
            else { parts.append("----") }
            if let month = birthday.month { parts.append(String(format: "%02d", month)) }
            if let day = birthday.day { parts.append(String(format: "%02d", day)) }
            dict["birthday"] = parts.joined(separator: "-")
        }

        return dict
    }

    /// Find a contact by identifier.
    static func findContact(byId identifier: String) -> CNContact? {
        try? store.unifiedContact(withIdentifier: identifier, keysToFetch: detailKeys)
    }

    /// Find a contact by name (first match, case-insensitive).
    static func findContact(byName name: String) -> CNContact? {
        let predicate = CNContact.predicateForContacts(matchingName: name)
        let contacts = try? store.unifiedContacts(matching: predicate, keysToFetch: detailKeys)
        return contacts?.first
    }
}

// MARK: - Container resolution

/// Resolve the container identifier for a given group.
/// Uses `predicateForContainerOfGroup` first, then falls back to scanning all containers.
func resolveContainerForGroup(_ group: CNGroup) throws -> String {
    // Primary: ask the framework directly
    let predicate = CNContainer.predicateForContainerOfGroup(withIdentifier: group.identifier)
    let containers = try ContactStore.store.containers(matching: predicate)
    if let id = containers.first?.identifier {
        return id
    }

    // Fallback: scan all containers to find which one owns this group
    diag("WARNING: predicateForContainerOfGroup returned no results for '\(group.name)', scanning all containers")
    let allContainers = try ContactStore.store.containers(matching: nil)
    for c in allContainers {
        let groupPredicate = CNGroup.predicateForGroupsInContainer(withIdentifier: c.identifier)
        let groupsInContainer = try ContactStore.store.groups(matching: groupPredicate)
        if groupsInContainer.contains(where: { $0.identifier == group.identifier }) {
            diag("found group '\(group.name)' in container '\(c.name)' (\(c.identifier))")
            return c.identifier
        }
    }

    // Last resort: use default container (may cause 134092 if group is in a different container)
    diag("WARNING: could not resolve container for group '\(group.name)', using default")
    return ContactStore.store.defaultContainerIdentifier()
}

// MARK: - Type coercion helpers

/// Safely coerce Any? to String? — handles String, NSString, NSNumber, and nil.
/// Use this instead of `as? String` when JSON values may not all be strings.
func str(_ value: Any?) -> String? {
    if let s = value as? String { return s }
    if let n = value as? NSNumber { return n.stringValue }
    return nil
}

// MARK: - JSON I/O helpers

/// Read stdin as JSON, returning a dictionary. Empty stdin -> empty dict.
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
    FileHandle.standardError.write("[contacts-helper] \(message)\n".data(using: .utf8)!)
}

/// Write an error JSON to stderr and exit with the given code.
func writeError(_ message: String, exitCode: Int32 = 1) {
    let json: [String: Any] = ["error": true, "message": message]
    if let data = try? JSONSerialization.data(withJSONObject: json) {
        FileHandle.standardError.write(data)
        FileHandle.standardError.write("\n".data(using: .utf8)!)
    }
}
