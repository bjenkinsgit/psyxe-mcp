(*
@tool reminders_delete
@version 1.0
@input reminder_name:string (argv 1), list_name:string? (argv 2)
@output "OK: Deleted ..." on success
@errors Outputs "ERROR: message" on failure
*)

on run argv
    if (count of argv) < 1 then
        return "ERROR: Missing reminder name argument"
    end if

    set reminderName to item 1 of argv
    set listFilter to missing value

    if (count of argv) > 1 and item 2 of argv is not "" then
        set listFilter to item 2 of argv
    end if

    try
        tell application "Reminders"
            set foundReminder to missing value

            if listFilter is not missing value then
                try
                    set targetList to list listFilter
                    set matches to (every reminder of targetList whose name is reminderName)
                    if (count of matches) > 0 then
                        set foundReminder to item 1 of matches
                    end if
                on error
                    return "ERROR: List not found: " & listFilter
                end try
            else
                set matches to (every reminder whose name is reminderName)
                if (count of matches) > 0 then
                    set foundReminder to item 1 of matches
                end if
            end if

            if foundReminder is missing value then
                return "ERROR: Reminder not found: " & reminderName
            end if

            delete foundReminder
            return "OK: Deleted reminder '" & reminderName & "'"
        end tell

    on error errMsg
        return "ERROR: " & errMsg
    end try
end run
