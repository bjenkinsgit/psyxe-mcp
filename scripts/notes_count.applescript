(*
@tool notes_count
@version 1.0
@input none
@output Single line: "COUNT: <number>"
@errors Outputs "ERROR: message" on failure
@description Quick count of all notes for index staleness check
*)

on run argv
    try
        tell application "Notes"
            set noteCount to count of every note
            return "COUNT: " & noteCount
        end tell
    on error errMsg
        return "ERROR: " & errMsg
    end try
end run
