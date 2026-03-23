(*
@tool reminders_delete_list
@version 1.0
@input list_name:string (argv 1)
@output "OK: Deleted list ..." on success
@errors Outputs "ERROR: message" on failure
*)

on run argv
    if (count of argv) < 1 then
        return "ERROR: Missing list name argument"
    end if

    set listName to item 1 of argv

    try
        tell application "Reminders"
            try
                set targetList to list listName
            on error
                return "ERROR: List not found: " & listName
            end try

            delete targetList
            return "OK: Deleted list '" & listName & "'"
        end tell

    on error errMsg
        return "ERROR: " & errMsg
    end try
end run
