import Contacts
import Foundation

enum GetContact {
    static func run(_ input: [String: Any]) throws -> Never {
        let id = input["id"] as? String
        let name = input["name"] as? String

        let contact: CNContact?
        if let id, !id.isEmpty {
            contact = ContactStore.findContact(byId: id)
        } else if let name, !name.isEmpty {
            contact = ContactStore.findContact(byName: name)
        } else {
            writeError("Missing required 'id' or 'name' field")
            exit(1)
        }

        guard let contact else {
            let identifier = id ?? name ?? "unknown"
            writeError("Contact '\(identifier)' not found")
            exit(1)
        }

        // Enforce group/container scope: if specified, verify the contact
        // is a member of that scope. Without this, a caller could bypass
        // access restrictions by looking up contacts by name/ID.
        if let groupName = input["group"] as? String, !groupName.isEmpty {
            let groups = try ContactStore.store.groups(matching: nil)
            guard let group = groups.first(where: { $0.name.caseInsensitiveCompare(groupName) == .orderedSame }) else {
                writeError("Group '\(groupName)' not found")
                exit(1)
            }
            let pred = CNContact.predicateForContactsInGroup(withIdentifier: group.identifier)
            let members = try ContactStore.store.unifiedContacts(
                matching: pred,
                keysToFetch: [CNContactIdentifierKey as CNKeyDescriptor]
            )
            let memberIds = Set(members.map { $0.identifier })
            guard memberIds.contains(contact.identifier) else {
                let identifier = name ?? id ?? "unknown"
                writeError("Contact '\(identifier)' not found in group '\(groupName)'")
                exit(1)
            }
        } else if let containerName = input["container"] as? String, !containerName.isEmpty {
            let containers = try ContactStore.store.containers(matching: nil)
            guard let container = containers.first(where: { $0.name.caseInsensitiveCompare(containerName) == .orderedSame }) else {
                writeError("Container '\(containerName)' not found")
                exit(1)
            }
            let pred = CNContact.predicateForContactsInContainer(withIdentifier: container.identifier)
            let members = try ContactStore.store.unifiedContacts(
                matching: pred,
                keysToFetch: [CNContactIdentifierKey as CNKeyDescriptor]
            )
            let memberIds = Set(members.map { $0.identifier })
            guard memberIds.contains(contact.identifier) else {
                let identifier = name ?? id ?? "unknown"
                writeError("Contact '\(identifier)' not found in container '\(containerName)'")
                exit(1)
            }
        }

        writeOutput(["success": true, "contact": ContactStore.detailDict(contact)])
    }
}
