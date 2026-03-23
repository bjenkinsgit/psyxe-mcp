import Contacts
import Foundation

enum SearchContacts {
    /// Maximum results to return per search.
    private static let maxResults = 500

    static func run(_ input: [String: Any]) throws -> Never {
        guard let query = input["query"] as? String, !query.isEmpty else {
            writeError("Missing required 'query' field")
            exit(1)
        }

        // Build scope set: when a group/container is specified, collect all
        // contact identifiers that belong to it so we can filter global results.
        var scopeIds: Set<String>? = nil
        var scopePredicate: NSPredicate? = nil

        if let groupName = input["group"] as? String, !groupName.isEmpty {
            let groups = try ContactStore.store.groups(matching: nil)
            if let group = groups.first(where: { $0.name.caseInsensitiveCompare(groupName) == .orderedSame }) {
                scopePredicate = CNContact.predicateForContactsInGroup(withIdentifier: group.identifier)
            }
        } else if let containerName = input["container"] as? String, !containerName.isEmpty {
            let containers = try ContactStore.store.containers(matching: nil)
            if let container = containers.first(where: { $0.name.caseInsensitiveCompare(containerName) == .orderedSame }) {
                scopePredicate = CNContact.predicateForContactsInContainer(withIdentifier: container.identifier)
            }
        }

        // Collect the IDs of contacts in scope (for filtering the name search)
        if let pred = scopePredicate {
            let scopeContacts = try ContactStore.store.unifiedContacts(
                matching: pred,
                keysToFetch: [CNContactIdentifierKey as CNKeyDescriptor]
            )
            scopeIds = Set(scopeContacts.map { $0.identifier })
        }

        var allContacts: [CNContact] = []

        // Search by name using built-in predicate (this is global)
        let namePredicate = CNContact.predicateForContacts(matchingName: query)
        let nameMatches = try ContactStore.store.unifiedContacts(
            matching: namePredicate,
            keysToFetch: ContactStore.summaryKeys
        )

        // Filter name matches to scope if one is active
        for contact in nameMatches {
            if let ids = scopeIds {
                if ids.contains(contact.identifier) {
                    allContacts.append(contact)
                }
            } else {
                allContacts.append(contact)
            }
        }
        let nameIds = Set(allContacts.map { $0.identifier })

        // Also search by email/phone/org via iteration (no built-in predicate for these)
        let queryLower = query.lowercased()
        let fetchRequest = CNContactFetchRequest(keysToFetch: ContactStore.summaryKeys)
        fetchRequest.sortOrder = .givenName

        // Scope to container or group if specified
        if let pred = scopePredicate {
            fetchRequest.predicate = pred
        }

        try ContactStore.store.enumerateContacts(with: fetchRequest) { contact, stop in
            // Skip contacts already found by name search
            if nameIds.contains(contact.identifier) { return }

            let matchesEmail = contact.emailAddresses.contains {
                ($0.value as String).lowercased().contains(queryLower)
            }
            // Normalize phone numbers to digits-only for comparison
            // e.g. "+15714407786" and "(571) 440-7786" both become "15714407786" / "5714407786"
            let queryDigits = query.filter { $0.isNumber }
            let matchesPhone = contact.phoneNumbers.contains { phoneNumber in
                let storedDigits = phoneNumber.value.stringValue.filter { $0.isNumber }
                // Match if either contains the other (handles +1 country code prefix)
                return storedDigits.contains(queryDigits)
                    || queryDigits.contains(storedDigits)
                    || phoneNumber.value.stringValue.contains(query)
            }
            let matchesOrg = contact.organizationName.lowercased().contains(queryLower)

            if matchesEmail || matchesPhone || matchesOrg {
                allContacts.append(contact)
            }

            if allContacts.count >= maxResults {
                stop.pointee = true
            }
        }

        let truncated = allContacts.count >= maxResults
        let results = allContacts.prefix(maxResults).map { ContactStore.summaryDict($0) }

        var output: [String: Any] = [
            "count": results.count,
            "results": Array(results),
        ]
        if truncated {
            output["truncated"] = true
        }
        writeOutput(output)
    }
}
