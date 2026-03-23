import Contacts
import Foundation

enum ListContacts {
    /// Maximum results to return.
    private static let maxResults = 500

    static func run(_ input: [String: Any]) throws -> Never {
        let containerName = input["container"] as? String
        let groupName = input["group"] as? String

        var predicate: NSPredicate? = nil

        if let groupName, !groupName.isEmpty {
            let groups = try ContactStore.store.groups(matching: nil)
            guard let group = groups.first(where: { $0.name.caseInsensitiveCompare(groupName) == .orderedSame }) else {
                writeError("Group '\(groupName)' not found")
                exit(1)
            }
            predicate = CNContact.predicateForContactsInGroup(withIdentifier: group.identifier)
        } else if let containerName, !containerName.isEmpty {
            let containers = try ContactStore.store.containers(matching: nil)
            guard let container = containers.first(where: { $0.name.caseInsensitiveCompare(containerName) == .orderedSame }) else {
                writeError("Container '\(containerName)' not found")
                exit(1)
            }
            predicate = CNContact.predicateForContactsInContainer(withIdentifier: container.identifier)
        }

        var contacts: [CNContact] = []
        if let predicate {
            contacts = try ContactStore.store.unifiedContacts(
                matching: predicate,
                keysToFetch: ContactStore.summaryKeys
            )
        } else {
            // No scope — enumerate all contacts
            let fetchRequest = CNContactFetchRequest(keysToFetch: ContactStore.summaryKeys)
            fetchRequest.sortOrder = .givenName
            try ContactStore.store.enumerateContacts(with: fetchRequest) { contact, stop in
                contacts.append(contact)
                if contacts.count >= maxResults {
                    stop.pointee = true
                }
            }
        }

        let truncated = contacts.count > maxResults
        let results = contacts.prefix(maxResults).map { ContactStore.summaryDict($0) }

        var output: [String: Any] = [
            "count": results.count,
            "contacts": Array(results),
        ]
        if truncated {
            output["truncated"] = true
        }
        writeOutput(output)
    }
}
