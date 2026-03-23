import Foundation

let args = CommandLine.arguments
guard args.count >= 2 else {
    writeError("Usage: reminders-helper <subcommand> [< input.json]")
    exit(1)
}

let subcommand = args[1]

do {
    try await EventKitStore.authorize()
} catch {
    writeError("Failed to authorize Reminders access: \(error.localizedDescription)", exitCode: 2)
    exit(2)
}

let input = readInput()

do {
    switch subcommand {
    case "list-lists":
        try await ListLists.run(input)
    case "search":
        try await Search.run(input)
    case "list":
        try await ListReminders.run(input)
    case "get":
        try await GetReminder.run(input)
    case "create":
        try await CreateReminder.run(input)
    case "create-batch":
        try await CreateBatch.run(input)
    case "complete":
        try await CompleteReminder.run(input)
    case "delete":
        try await DeleteReminder.run(input)
    case "create-list":
        try await CreateList.run(input)
    case "delete-list":
        try await DeleteList.run(input)
    case "edit":
        try await EditReminder.run(input)
    case "edit-batch":
        try await EditBatch.run(input)
    case "set-url":
        try await SetURL.run(input)
    case "open":
        try await OpenReminders.run(input)
    default:
        writeError("Unknown subcommand: \(subcommand)")
        exit(1)
    }
} catch {
    writeError(error.localizedDescription)
    exit(1)
}
