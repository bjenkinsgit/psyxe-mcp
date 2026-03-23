import Contacts
import Foundation

enum DeleteContact {
    static func run(_ input: [String: Any]) throws -> Never {
        guard let id = input["id"] as? String, !id.isEmpty else {
            writeError("Missing required 'id' field")
            exit(1)
        }

        guard let immutableContact = ContactStore.findContact(byId: id) else {
            writeError("Contact with id '\(id)' not found")
            exit(1)
        }

        let contact = immutableContact.mutableCopy() as! CNMutableContact
        let saveRequest = CNSaveRequest()
        saveRequest.delete(contact)
        try ContactStore.store.execute(saveRequest)

        let fullName = [immutableContact.givenName, immutableContact.familyName]
            .filter { !$0.isEmpty }
            .joined(separator: " ")

        writeOutput(["success": true, "message": "Deleted contact '\(fullName)'"])
    }
}
