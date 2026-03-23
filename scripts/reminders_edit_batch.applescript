(*
@tool reminders_edit_batch
@version 1.0
@input listName:string (argv 1), itemsStr:string (argv 2, "name:::title:::due_date:::notes:::priority" joined by "|||")
@output "OK: Updated N reminders..." on success
@errors Outputs "ERROR: message" on failure
*)

on run argv
    if (count of argv) < 2 then
        return "ERROR: Missing list name and items arguments"
    end if

    set listName to item 1 of argv
    set itemsStr to item 2 of argv

    try
        tell application "Reminders"
            -- Get target list
            try
                set targetList to list listName
            on error
                return "ERROR: List not found: " & listName
            end try

            -- Split items by "|||"
            set oldDelims to AppleScript's text item delimiters
            set AppleScript's text item delimiters to "|||"
            set itemChunks to text items of itemsStr
            set AppleScript's text item delimiters to oldDelims

            set updatedCount to 0

            repeat with chunk in itemChunks
                -- Split fields by ":::"
                set AppleScript's text item delimiters to ":::"
                set fields to text items of (chunk as text)
                set AppleScript's text item delimiters to oldDelims

                if (count of fields) < 1 then
                    -- skip empty
                else
                    set reminderName to item 1 of fields
                    set newTitle to ""
                    set dueStr to ""
                    set noteText to ""
                    set priorityStr to ""

                    if (count of fields) > 1 then set newTitle to item 2 of fields
                    if (count of fields) > 2 then set dueStr to item 3 of fields
                    if (count of fields) > 3 then set noteText to item 4 of fields
                    if (count of fields) > 4 then set priorityStr to item 5 of fields

                    -- Find the reminder
                    set targetReminder to missing value
                    repeat with r in (every reminder of targetList whose completed is false)
                        if name of r is reminderName then
                            set targetReminder to r
                            exit repeat
                        end if
                    end repeat

                    if targetReminder is not missing value then
                        if newTitle is not "" then
                            set name of targetReminder to newTitle
                        end if

                        if dueStr is not "" then
                            if dueStr is "clear" then
                                set due date of targetReminder to missing value
                            else
                                set theYear to (text 1 thru 4 of dueStr) as integer
                                set theMonth to (text 6 thru 7 of dueStr) as integer
                                set theDay to (text 9 thru 10 of dueStr) as integer
                                set dueDate to current date
                                set year of dueDate to theYear
                                set month of dueDate to theMonth
                                set day of dueDate to theDay
                                set hours of dueDate to 9
                                set minutes of dueDate to 0
                                set seconds of dueDate to 0
                                set due date of targetReminder to dueDate
                            end if
                        end if

                        if noteText is not "" then
                            set body of targetReminder to noteText
                        end if

                        if priorityStr is not "" then
                            set priority of targetReminder to (priorityStr as integer)
                        end if

                        set updatedCount to updatedCount + 1
                    end if
                end if
            end repeat

            return "OK: Updated " & updatedCount & " reminders in '" & listName & "'"
        end tell

    on error errMsg
        return "ERROR: " & errMsg
    end try
end run
