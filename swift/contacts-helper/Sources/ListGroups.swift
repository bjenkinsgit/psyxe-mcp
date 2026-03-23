import Contacts
import Foundation

enum ListGroups {
    static func run(_ input: [String: Any]) throws -> Never {
        var results: [[String: Any]] = []

        // List all containers
        let containers = try ContactStore.store.containers(matching: nil)
        for container in containers {
            // Count contacts in this container
            let predicate = CNContact.predicateForContactsInContainer(withIdentifier: container.identifier)
            let contacts = try ContactStore.store.unifiedContacts(matching: predicate, keysToFetch: [CNContactIdentifierKey as CNKeyDescriptor])
            results.append([
                "name": container.name,
                "id": container.identifier,
                "type": "container",
                "count": contacts.count,
            ])
        }

        // List all groups
        let groups = try ContactStore.store.groups(matching: nil)
        for group in groups {
            // Find which container this group belongs to
            var containerName = ""
            for container in containers {
                let groupPredicate = CNGroup.predicateForGroups(withIdentifiers: [group.identifier])
                let containerGroups = try ContactStore.store.groups(matching: groupPredicate)
                if !containerGroups.isEmpty {
                    // Check if this group is in this container
                    let groupsInContainer = try ContactStore.store.groups(matching: CNGroup.predicateForGroupsInContainer(withIdentifier: container.identifier))
                    if groupsInContainer.contains(where: { $0.identifier == group.identifier }) {
                        containerName = container.name
                        break
                    }
                }
            }

            // Count contacts in this group
            let predicate = CNContact.predicateForContactsInGroup(withIdentifier: group.identifier)
            let contacts = try ContactStore.store.unifiedContacts(matching: predicate, keysToFetch: [CNContactIdentifierKey as CNKeyDescriptor])
            var entry: [String: Any] = [
                "name": group.name,
                "id": group.identifier,
                "type": "group",
                "count": contacts.count,
            ]
            if !containerName.isEmpty {
                entry["container_name"] = containerName
            }
            results.append(entry)
        }

        writeOutput(["count": results.count, "sources": results])
    }
}
