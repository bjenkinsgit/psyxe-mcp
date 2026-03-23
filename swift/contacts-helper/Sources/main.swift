import Foundation

let args = CommandLine.arguments
guard args.count >= 2 else {
    writeError("Usage: contacts-helper <subcommand> [< input.json]")
    exit(1)
}

let subcommand = args[1]

do {
    try await ContactStore.authorize()
} catch {
    writeError("Failed to authorize Contacts access: \(error.localizedDescription)", exitCode: 2)
    exit(2)
}

let input = readInput()

do {
    switch subcommand {
    case "list-groups":
        try ListGroups.run(input)
    case "search":
        try SearchContacts.run(input)
    case "list":
        try ListContacts.run(input)
    case "get":
        try GetContact.run(input)
    case "create":
        try CreateContact.run(input)
    case "edit":
        try EditContact.run(input)
    case "delete":
        try DeleteContact.run(input)
    default:
        writeError("Unknown subcommand: \(subcommand)")
        exit(1)
    }
} catch {
    writeError(error.localizedDescription)
    exit(1)
}
