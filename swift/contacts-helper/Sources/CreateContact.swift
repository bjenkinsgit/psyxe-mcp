import Contacts
import Foundation

enum CreateContact {
    static func run(_ input: [String: Any]) throws -> Never {
        // Log received fields for diagnostics
        let receivedKeys = input.keys.sorted().joined(separator: ", ")
        diag("create: received fields: [\(receivedKeys)]")

        guard let givenName = input["given_name"] as? String, !givenName.isEmpty else {
            writeError("Missing required 'given_name' field")
            exit(1)
        }

        let contact = CNMutableContact()
        contact.givenName = givenName

        let familyName = input["family_name"] as? String ?? ""
        contact.familyName = familyName

        if let org = input["organization"] as? String {
            contact.organizationName = org
            // Mark as organization contact when there's no family name —
            // this makes Contacts.app display it as a company card
            if familyName.isEmpty {
                contact.contactType = .organization
            }
        }
        if let middleName = input["middle_name"] as? String {
            contact.middleName = middleName
        }
        if let nickname = input["nickname"] as? String {
            contact.nickname = nickname
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

        // Birthday (accepts YYYY-MM-DD, --MM-DD, or MM-DD)
        if let birthdayStr = input["birthday"] as? String, !birthdayStr.isEmpty {
            contact.birthday = parseBirthday(birthdayStr)
        }

        // Profile image (base64-encoded image data)
        if let imageBase64 = input["image_base64"] as? String, !imageBase64.isEmpty,
           let imageData = Data(base64Encoded: imageBase64) {
            contact.imageData = imageData
        }
        // Profile image from file path
        if let imagePath = input["image_path"] as? String, !imagePath.isEmpty {
            let url = URL(fileURLWithPath: imagePath)
            if let imageData = try? Data(contentsOf: url) {
                contact.imageData = imageData
            }
        }

        // Phone numbers
        if let phone = input["phone"] as? String, !phone.isEmpty {
            contact.phoneNumbers = [CNLabeledValue(label: CNLabelPhoneNumberMain, value: CNPhoneNumber(stringValue: phone))]
        }
        if let phones = input["phones"] as? [[String: Any]] {
            contact.phoneNumbers = phones.map { p in
                let label = mapPhoneLabel(str(p["label"]) ?? "main")
                return CNLabeledValue(label: label, value: CNPhoneNumber(stringValue: str(p["value"]) ?? ""))
            }
        }

        // Email addresses
        if let email = input["email"] as? String, !email.isEmpty {
            contact.emailAddresses = [CNLabeledValue(label: CNLabelHome, value: email as NSString)]
        }
        if let emails = input["emails"] as? [[String: Any]] {
            contact.emailAddresses = emails.map { e in
                let label = mapLabel(str(e["label"]) ?? "home")
                return CNLabeledValue(label: label, value: (str(e["value"]) ?? "") as NSString)
            }
        }

        // Postal addresses
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

        // URL addresses
        if let urls = input["urls"] as? [[String: Any]] {
            contact.urlAddresses = urls.compactMap { u in
                guard let value = str(u["value"]), !value.isEmpty else { return nil }
                let label = mapLabel(str(u["label"]) ?? "home")
                return CNLabeledValue(label: label, value: value as NSString)
            }
        }

        // Instant message addresses
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

        // Log what actually got set on the contact
        var setFields: [String] = []
        if !contact.givenName.isEmpty { setFields.append("givenName") }
        if !contact.middleName.isEmpty { setFields.append("middleName") }
        if !contact.familyName.isEmpty { setFields.append("familyName") }
        if !contact.nickname.isEmpty { setFields.append("nickname") }
        if !contact.organizationName.isEmpty { setFields.append("organization") }
        if !contact.jobTitle.isEmpty { setFields.append("jobTitle") }
        if !contact.note.isEmpty { setFields.append("note") }
        if !contact.phoneNumbers.isEmpty { setFields.append("phones(\(contact.phoneNumbers.count))") }
        if !contact.emailAddresses.isEmpty { setFields.append("emails(\(contact.emailAddresses.count))") }
        if !contact.postalAddresses.isEmpty { setFields.append("addresses(\(contact.postalAddresses.count))") }
        if !contact.urlAddresses.isEmpty { setFields.append("urls(\(contact.urlAddresses.count))") }
        if !contact.instantMessageAddresses.isEmpty { setFields.append("ims(\(contact.instantMessageAddresses.count))") }
        if contact.imageData != nil { setFields.append("image") }
        if contact.birthday != nil { setFields.append("birthday") }
        diag("create: populated fields: [\(setFields.joined(separator: ", "))]")

        // Save to specified group or default container
        let saveRequest = CNSaveRequest()

        if let groupName = input["group"] as? String, !groupName.isEmpty {
            let groups = try ContactStore.store.groups(matching: nil)
            diag("create: looking for group '\(groupName)' among \(groups.count) groups: [\(groups.map { $0.name }.joined(separator: ", "))]")
            guard let group = groups.first(where: { $0.name.caseInsensitiveCompare(groupName) == .orderedSame }) else {
                writeError("Group '\(groupName)' not found. Available groups: \(groups.map { $0.name }.joined(separator: ", "))")
                exit(1)
            }
            // Resolve the container that owns this group. Saving to the wrong container
            // causes Cocoa error 134092 (cross-container membership validation failure).
            let containerId = try resolveContainerForGroup(group)
            diag("group='\(group.name)' groupId=\(group.identifier) containerId=\(containerId)")
            saveRequest.add(contact, toContainerWithIdentifier: containerId)
            saveRequest.addMember(contact, to: group)
        } else if let containerName = input["container"] as? String, !containerName.isEmpty {
            let containers = try ContactStore.store.containers(matching: nil)
            guard let container = containers.first(where: { $0.name.caseInsensitiveCompare(containerName) == .orderedSame }) else {
                writeError("Container '\(containerName)' not found")
                exit(1)
            }
            diag("container='\(container.name)' containerId=\(container.identifier)")
            saveRequest.add(contact, toContainerWithIdentifier: container.identifier)
        } else {
            // No group or container specified — prefer iCloud over the default container.
            // On machines with both "On My Mac" and "iCloud", the default is often the local
            // container which can fail with Cocoa error 134092.
            let allContainers = try ContactStore.store.containers(matching: nil)
            let iCloudContainer = allContainers.first(where: { $0.name.caseInsensitiveCompare("iCloud") == .orderedSame })
            if let iCloud = iCloudContainer {
                diag("no group/container specified, using iCloud container (\(iCloud.identifier))")
                saveRequest.add(contact, toContainerWithIdentifier: iCloud.identifier)
            } else {
                diag("no group/container specified, no iCloud found, using default container")
                saveRequest.add(contact, toContainerWithIdentifier: nil)
            }
        }

        var noteDropped = false
        do {
            try ContactStore.store.execute(saveRequest)
        } catch {
            let nsError = error as NSError
            // Cocoa error 134092: on macOS Sonoma+, writing CNContactNoteKey requires
            // Full Contacts Access. If note was set, retry without it.
            if nsError.code == 134092 && !contact.note.isEmpty {
                diag("create: error 134092 with note set — retrying without note field")
                contact.note = ""
                noteDropped = true
                let retryRequest = CNSaveRequest()
                // Re-add to the same container/group as the original request
                // (contact object is already configured, just need a fresh save request)
                if let groupName = input["group"] as? String, !groupName.isEmpty {
                    let groups = try ContactStore.store.groups(matching: nil)
                    if let group = groups.first(where: { $0.name.caseInsensitiveCompare(groupName) == .orderedSame }) {
                        let containerId = try resolveContainerForGroup(group)
                        retryRequest.add(contact, toContainerWithIdentifier: containerId)
                        retryRequest.addMember(contact, to: group)
                    } else {
                        retryRequest.add(contact, toContainerWithIdentifier: nil)
                    }
                } else if let containerName = input["container"] as? String, !containerName.isEmpty {
                    let containers = try ContactStore.store.containers(matching: nil)
                    if let container = containers.first(where: { $0.name.caseInsensitiveCompare(containerName) == .orderedSame }) {
                        retryRequest.add(contact, toContainerWithIdentifier: container.identifier)
                    } else {
                        retryRequest.add(contact, toContainerWithIdentifier: nil)
                    }
                } else {
                    let allContainers = try ContactStore.store.containers(matching: nil)
                    let iCloud = allContainers.first(where: { $0.name.caseInsensitiveCompare("iCloud") == .orderedSame })
                    retryRequest.add(contact, toContainerWithIdentifier: iCloud?.identifier)
                }
                do {
                    try ContactStore.store.execute(retryRequest)
                } catch {
                    let retryError = error as NSError
                    writeError("Save failed on retry (without note): \(retryError.localizedDescription) (domain=\(retryError.domain) code=\(retryError.code))")
                    exit(1)
                }
            } else {
                writeError("Save failed: \(nsError.localizedDescription) (domain=\(nsError.domain) code=\(nsError.code))")
                exit(1)
            }
        }

        let fullName = [contact.givenName, contact.familyName]
            .filter { !$0.isEmpty }
            .joined(separator: " ")

        var message = "Created contact '\(fullName)'"
        if noteDropped {
            message += " (note field was skipped — macOS requires Full Contacts Access to write notes. Grant in System Settings > Privacy & Security > Contacts.)"
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

    /// Parse a birthday string into DateComponents.
    /// Accepts: "YYYY-MM-DD", "----MM-DD" (no year), "MM-DD" (no year)
    static func parseBirthday(_ str: String) -> DateComponents {
        var components = DateComponents()
        let cleaned = str.replacingOccurrences(of: "----", with: "")
        let parts = cleaned.split(separator: "-").compactMap { Int($0) }
        switch parts.count {
        case 3:
            components.year = parts[0]
            components.month = parts[1]
            components.day = parts[2]
        case 2:
            components.month = parts[0]
            components.day = parts[1]
        default:
            break
        }
        return components
    }
}
