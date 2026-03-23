(*
@tool reminders_list_lists
@version 1.2
@input (none)
@output RECORD_START/RECORD_END delimited records
@fields name, id
@errors Outputs "ERROR: message" on failure
@notes Collects top-level lists AND lists nested inside groups
*)

on run argv
    set allNames to {}
    set allIDs to {}

    try
        tell application "Reminders"
            -- Batch fetch top-level lists
            set topNames to name of every list
            set topIDs to id of every list
            set topLists to every list
        end tell

        -- Add top-level lists
        repeat with i from 1 to count of topNames
            set end of allNames to item i of topNames
            set end of allIDs to item i of topIDs
        end repeat

        -- Check each top-level list for nested children (groups)
        repeat with i from 1 to count of topLists
            try
                tell application "Reminders"
                    set childNames to name of every list of (item i of topLists)
                    set childIDs to id of every list of (item i of topLists)
                end tell
                repeat with j from 1 to count of childNames
                    -- Avoid duplicates: only add if not already in top-level
                    set cID to item j of childIDs
                    if allIDs does not contain cID then
                        set end of allNames to item j of childNames
                        set end of allIDs to cID
                    end if
                end repeat
            end try
        end repeat

        -- Build output
        set outputText to ""
        repeat with i from 1 to count of allNames
            set outputText to outputText & "RECORD_START" & linefeed
            set outputText to outputText & "name: " & (item i of allNames) & linefeed
            set outputText to outputText & "id: " & (item i of allIDs) & linefeed
            set outputText to outputText & "RECORD_END" & linefeed
        end repeat

        return outputText

    on error errMsg
        return "ERROR: " & errMsg
    end try
end run
