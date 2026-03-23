import Contacts
import Foundation

enum EditContact {
    static func run(_ input: [String: Any]) throws -> Never {
        guard let id = input["id"] as? String, !id.isEmpty else {
            writeError("Missing required 'id' field")
            exit(1)
        }

        let receivedKeys = input.keys.sorted().joined(separator: ", ")
        diag("edit: received fields: [\(receivedKeys)]")

        guard let immutableContact = ContactStore.findContact(byId: id) else {
            writeError("Contact with id '\(id)' not found")
            exit(1)
        }

        diag("edit: found contact '\(immutableContact.givenName) \(immutableContact.familyName)' id=\(id)")
        let contact = immutableContact.mutableCopy() as! CNMutableContact

        if let givenName = input["given_name"] as? String {
            contact.givenName = givenName
        }
        if let familyName = input["family_name"] as? String {
            contact.familyName = familyName
        }
        if let middleName = input["middle_name"] as? String {
            contact.middleName = middleName
        }
        if let nickname = input["nickname"] as? String {
            contact.nickname = nickname
        }
        if let org = input["organization"] as? String {
            contact.organizationName = org
        }
        if let jobTitle = input["job_title"] as? String {
            contact.jobTitle = jobTitle
        }
        if let department = input["department"] as? String {
            contact.departmentName = department
        }
        if let note = input["note"] as? String {
            contact.note = note
        }

        // Birthday (accepts YYYY-MM-DD, --MM-DD, or MM-DD; empty string clears)
        if let birthdayStr = input["birthday"] as? String {
            if birthdayStr.isEmpty {
                contact.birthday = nil
            } else {
                contact.birthday = CreateContact.parseBirthday(birthdayStr)
            }
        }

        // Profile image (base64-encoded; empty string clears)
        if let imageBase64 = input["image_base64"] as? String {
            if imageBase64.isEmpty {
                contact.imageData = nil
            } else if let imageData = Data(base64Encoded: imageBase64) {
                contact.imageData = imageData
            }
        }
        if let imagePath = input["image_path"] as? String, !imagePath.isEmpty {
            let url = URL(fileURLWithPath: imagePath)
            if let imageData = try? Data(contentsOf: url) {
                contact.imageData = imageData
            }
        }

        // Add phone (appends to existing)
        if let addPhone = input["add_phone"] as? String, !addPhone.isEmpty {
            let label = input["add_phone_label"] as? String ?? "main"
            let phoneLabel = mapPhoneLabel(label)
            contact.phoneNumbers.append(
                CNLabeledValue(label: phoneLabel, value: CNPhoneNumber(stringValue: addPhone))
            )
        }

        // Add email (appends to existing)
        if let addEmail = input["add_email"] as? String, !addEmail.isEmpty {
            let label = input["add_email_label"] as? String ?? "home"
            let emailLabel = mapLabel(label)
            contact.emailAddresses.append(
                CNLabeledValue(label: emailLabel, value: addEmail as NSString)
            )
        }

        // Replace all phones
        if let phones = input["phones"] as? [[String: Any]] {
            contact.phoneNumbers = phones.map { p in
                let label = mapPhoneLabel(str(p["label"]) ?? "main")
                return CNLabeledValue(label: label, value: CNPhoneNumber(stringValue: str(p["value"]) ?? ""))
            }
        }

        // Replace all emails
        if let emails = input["emails"] as? [[String: Any]] {
            contact.emailAddresses = emails.map { e in
                let label = mapLabel(str(e["label"]) ?? "home")
                return CNLabeledValue(label: label, value: (str(e["value"]) ?? "") as NSString)
            }
        }

        // Replace all addresses
        if let addresses = input["addresses"] as? [[String: Any]] {
            contact.postalAddresses = addresses.map { a in
                let addr = CNMutablePostalAddress()
                addr.street = str(a["street"]) ?? ""
                addr.city = str(a["city"]) ?? ""
                addr.state = str(a["state"]) ?? ""
                addr.postalCode = str(a["postal_code"]) ?? ""
                addr.country = str(a["country"]) ?? ""
                let label = mapLabel(str(a["label"]) ?? "home")
                return CNLabeledValue(label: label, value: addr)
            }
        }

        // Replace all URLs
        if let urls = input["urls"] as? [[String: Any]] {
            contact.urlAddresses = urls.compactMap { u in
                guard let value = str(u["value"]), !value.isEmpty else { return nil }
                let label = mapLabel(str(u["label"]) ?? "home")
                return CNLabeledValue(label: label, value: value as NSString)
            }
        }

        // Replace all instant messages
        if let ims = input["instant_messages"] as? [[String: Any]] {
            contact.instantMessageAddresses = ims.map { im in
                let service = CNInstantMessageAddress(
                    username: str(im["username"]) ?? "",
                    service: str(im["service"]) ?? ""
                )
                let label = mapLabel(str(im["label"]) ?? "home")
                return CNLabeledValue(label: label, value: service)
            }
        }

        let saveRequest = CNSaveRequest()
        saveRequest.update(contact)

        var noteDropped = false
        do {
            try ContactStore.store.execute(saveRequest)
        } catch {
            let nsError = error as NSError
            // Cocoa error 134092: note field requires Full Contacts Access on macOS Sonoma+
            if nsError.code == 134092 && input["note"] != nil {
                diag("edit: error 134092 with note — retrying without note field")
                // Re-fetch and re-apply all fields except note
                guard let fresh = ContactStore.findContact(byId: id) else {
                    writeError("Contact disappeared during retry")
                    exit(1)
                }
                let retryContact = fresh.mutableCopy() as! CNMutableContact
                // Re-apply all non-note fields from input
                if let v = input["given_name"] as? String { retryContact.givenName = v }
                if let v = input["family_name"] as? String { retryContact.familyName = v }
                if let v = input["middle_name"] as? String { retryContact.middleName = v }
                if let v = input["nickname"] as? String { retryContact.nickname = v }
                if let v = input["organization"] as? String { retryContact.organizationName = v }
                if let v = input["job_title"] as? String { retryContact.jobTitle = v }
                if let v = input["department"] as? String { retryContact.departmentName = v }
                if let birthdayStr = input["birthday"] as? String {
                    if birthdayStr.isEmpty { retryContact.birthday = nil }
                    else { retryContact.birthday = CreateContact.parseBirthday(birthdayStr) }
                }
                if let phones = input["phones"] as? [[String: Any]] {
                    retryContact.phoneNumbers = phones.map { p in
                        let label = mapPhoneLabel(str(p["label"]) ?? "main")
                        return CNLabeledValue(label: label, value: CNPhoneNumber(stringValue: str(p["value"]) ?? ""))
                    }
                }
                if let emails = input["emails"] as? [[String: Any]] {
                    retryContact.emailAddresses = emails.map { e in
                        let label = mapLabel(str(e["label"]) ?? "home")
                        return CNLabeledValue(label: label, value: (str(e["value"]) ?? "") as NSString)
                    }
                }
                if let addresses = input["addresses"] as? [[String: Any]] {
                    retryContact.postalAddresses = addresses.map { a in
                        let addr = CNMutablePostalAddress()
                        addr.street = str(a["street"]) ?? ""
                        addr.city = str(a["city"]) ?? ""
                        addr.state = str(a["state"]) ?? ""
                        addr.postalCode = str(a["postal_code"]) ?? ""
                        addr.country = str(a["country"]) ?? ""
                        let label = mapLabel(str(a["label"]) ?? "home")
                        return CNLabeledValue(label: label, value: addr)
                    }
                }
                if let urls = input["urls"] as? [[String: Any]] {
                    retryContact.urlAddresses = urls.compactMap { u in
                        guard let value = str(u["value"]), !value.isEmpty else { return nil }
                        let label = mapLabel(str(u["label"]) ?? "home")
                        return CNLabeledValue(label: label, value: value as NSString)
                    }
                }
                let retryRequest = CNSaveRequest()
                retryRequest.update(retryContact)
                noteDropped = true
                do {
                    try ContactStore.store.execute(retryRequest)
                } catch {
                    let retryError = error as NSError
                    writeError("Edit save failed on retry (without note): \(retryError.localizedDescription) (domain=\(retryError.domain) code=\(retryError.code))")
                    exit(1)
                }
            } else {
                writeError("Edit save failed: \(nsError.localizedDescription) (domain=\(nsError.domain) code=\(nsError.code))")
                exit(1)
            }
        }

        let fullName = [contact.givenName, contact.familyName]
            .filter { !$0.isEmpty }
            .joined(separator: " ")

        var message = "Updated contact '\(fullName)'"
        if noteDropped {
            message += " (note field was skipped — macOS requires Full Contacts Access to write notes)"
        }

        writeOutput([
            "success": true,
            "id": contact.identifier,
            "name": fullName,
            "message": message,
            "note_dropped": noteDropped,
        ])
    }

    private static func mapPhoneLabel(_ label: String) -> String {
        switch label.lowercased() {
        case "main": return CNLabelPhoneNumberMain
        case "home": return CNLabelHome
        case "work": return CNLabelWork
        case "mobile", "cell": return CNLabelPhoneNumberMobile
        case "iphone": return CNLabelPhoneNumberiPhone
        default: return CNLabelOther
        }
    }

    private static func mapLabel(_ label: String) -> String {
        switch label.lowercased() {
        case "home": return CNLabelHome
        case "work": return CNLabelWork
        case "school": return CNLabelSchool
        case "home page", "homepage": return CNLabelURLAddressHomePage
        default: return CNLabelOther
        }
    }
}
