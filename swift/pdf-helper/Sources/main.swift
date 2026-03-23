import Foundation

let args = CommandLine.arguments
guard args.count >= 2, args[1] == "generate" else {
    writeError("Usage: pdf-helper generate < input.json")
    exit(1)
}

let input = readInput()

do {
    try await PDFGenerator.run(input)
} catch {
    writeError(error.localizedDescription)
    exit(1)
}
