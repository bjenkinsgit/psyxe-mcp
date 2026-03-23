(*
@tool reminders_complete
@version 1.0
@input reminder_name:string (argv 1), list_name:string? (argv 2), completed:string? (argv 3, "true"/"false", default "true")
@output "OK: Marked ..." on success
@errors Outputs "ERROR: message" on failure
*)

on run argv
    if (count of argv) < 1 then
        return "ERROR: Missing reminder name argument"
    end if

    set reminderName to item 1 of argv
    set listFilter to missing value
    set markCompleted to true

    if (count of argv) > 1 and item 2 of argv is not "" then
        set listFilter to item 2 of argv
    end if
    if (count of argv) > 2 then
        if item 3 of argv is "false" then
            set markCompleted to false
        end if
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

            set completed of foundReminder to markCompleted

            if markCompleted then
                return "OK: Marked '" & reminderName & "' as completed"
            else
                return "OK: Marked '" & reminderName & "' as incomplete"
            end if
        end tell

    on error errMsg
        return "ERROR: " & errMsg
    end try
end run
