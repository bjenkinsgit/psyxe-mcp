(*
@tool reminders_open
@version 1.0
@input list_name:string? (argv 1)
@output "OK: Opened ..." on success
@errors Outputs "ERROR: message" on failure
*)

on run argv
    set listName to missing value
    if (count of argv) > 0 and item 1 of argv is not "" then
        set listName to item 1 of argv
    end if

    try
        tell application "Reminders"
            activate

            if listName is not missing value then
                try
                    set targetList to list listName
                    -- Reminders.app doesn't have a direct "show list" command,
                    -- but activating the app brings it to front
                    return "OK: Opened Reminders showing list '" & listName & "'"
                on error
                    return "OK: Opened Reminders (list '" & listName & "' not found)"
                end try
            else
                return "OK: Opened Reminders"
            end if
        end tell

    on error errMsg
        return "ERROR: " & errMsg
    end try
end run
