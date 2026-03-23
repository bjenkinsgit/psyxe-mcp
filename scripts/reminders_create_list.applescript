(*
@tool reminders_create_list
@version 1.1
@input list_name:string (argv 1)
@output "OK: Created list ..." or "OK: List already exists ..." on success
@errors Outputs "ERROR: message" on failure
*)

on run argv
    if (count of argv) < 1 then
        return "ERROR: Missing list name argument"
    end if

    set listName to item 1 of argv

    try
        tell application "Reminders"
            -- Check if list already exists
            try
                set existingList to list listName
                return "OK: List already exists '" & listName & "'"
            end try

            -- Create new list
            make new list with properties {name:listName}
            return "OK: Created list '" & listName & "'"
        end tell

    on error errMsg
        return "ERROR: " & errMsg
    end try
end run
