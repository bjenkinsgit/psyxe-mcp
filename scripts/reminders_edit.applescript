(*
@tool reminders_edit
@version 1.0
@input name:string (argv 1), list_name:string? (argv 2), new_title:string? (argv 3), due_date:string? (argv 4, YYYY-MM-DD or "clear"), notes:string? (argv 5), priority:string? (argv 6, 0/1/5/9)
@output "OK: Updated ..." on success
@errors Outputs "ERROR: message" on failure
*)

on run argv
    if (count of argv) < 1 then
        return "ERROR: Missing name argument"
    end if

    set reminderName to item 1 of argv
    set listName to missing value
    set newTitle to missing value
    set dueStr to missing value
    set noteText to missing value
    set priorityVal to missing value

    if (count of argv) > 1 and item 2 of argv is not "" then
        set listName to item 2 of argv
    end if
    if (count of argv) > 2 and item 3 of argv is not "" then
        set newTitle to item 3 of argv
    end if
    if (count of argv) > 3 and item 4 of argv is not "" then
        set dueStr to item 4 of argv
    end if
    if (count of argv) > 4 and item 5 of argv is not "" then
        set noteText to item 5 of argv
    end if
    if (count of argv) > 5 and item 6 of argv is not "" then
        set priorityVal to (item 6 of argv) as integer
    end if

    try
        tell application "Reminders"
            -- Find the reminder
            set targetReminder to missing value

            if listName is not missing value then
                try
                    set targetList to list listName
                on error
                    return "ERROR: List not found: " & listName
                end try
                repeat with r in (every reminder of targetList whose completed is false)
                    if name of r is reminderName then
                        set targetReminder to r
                        exit repeat
                    end if
                end repeat
            else
                repeat with aList in every list
                    repeat with r in (every reminder of aList whose completed is false)
                        if name of r is reminderName then
                            set targetReminder to r
                            exit repeat
                        end if
                    end repeat
                    if targetReminder is not missing value then exit repeat
                end repeat
            end if

            if targetReminder is missing value then
                return "ERROR: Reminder not found: " & reminderName
            end if

            -- Update title
            if newTitle is not missing value then
                set name of targetReminder to newTitle
            end if

            -- Update due date
            if dueStr is not missing value then
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

            -- Update notes (append to existing content if present)
            if noteText is not missing value then
                set existingNotes to body of targetReminder
                if existingNotes is missing value or existingNotes is "" then
                    set body of targetReminder to noteText
                else if existingNotes does not contain noteText then
                    set body of targetReminder to existingNotes & linefeed & noteText
                end if
            end if

            -- Update priority
            if priorityVal is not missing value then
                set priority of targetReminder to priorityVal
            end if

            set displayName to name of targetReminder
            return "OK: Updated reminder '" & displayName & "'"
        end tell

    on error errMsg
        return "ERROR: " & errMsg
    end try
end run
