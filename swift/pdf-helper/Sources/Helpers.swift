import Foundation

/// Read stdin as JSON, returning a dictionary. Empty stdin -> empty dict.
func readInput() -> [String: Any] {
    let data = FileHandle.standardInput.readDataToEndOfFile()
    if data.isEmpty { return [:] }
    guard let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
        writeError("Invalid JSON input")
        exit(1)
    }
    return json
}

/// Write a JSON-serializable value to stdout and exit 0.
func writeOutput(_ value: Any) -> Never {
    let data = try! JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write("\n".data(using: .utf8)!)
    exit(0)
}

/// Write an error JSON to stderr and exit with the given code.
func writeError(_ message: String, exitCode: Int32 = 1) {
    let json: [String: Any] = ["error": true, "message": message]
    if let data = try? JSONSerialization.data(withJSONObject: json) {
        FileHandle.standardError.write(data)
        FileHandle.standardError.write("\n".data(using: .utf8)!)
    }
}
