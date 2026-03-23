(*
@tool notes_create
@version 1.0
@input title:string (argv 1), body:string (argv 2), folder:string (argv 3, optional — defaults to "Notes")
@output Created note details: id, title, folder
@errors Outputs "ERROR: message" on failure
*)

on run argv
    -- Validate arguments
    if (count of argv) < 2 then
        return "ERROR: Missing arguments. Usage: notes_create <title> <body> [folder]"
    end if

    set noteTitle to item 1 of argv
    set noteBody to item 2 of argv

    -- Optional folder (default: "Notes")
    if (count of argv) ≥ 3 then
        set folderName to item 3 of argv
    else
        set folderName to "Notes"
    end if

    try
        tell application "Notes"
            -- Find or default to the target folder
            set targetFolder to missing value
            repeat with f in every folder
                if (name of f) = folderName then
                    set targetFolder to f
                    exit repeat
                end if
            end repeat

            if targetFolder is missing value then
                -- Fall back to default account's default folder
                set targetFolder to default folder of default account
            end if

            -- Create the note with HTML body
            set newNote to make new note at targetFolder with properties {name:noteTitle, body:noteBody}

            -- Return details
            set noteId to id of newNote
            set noteFolder to name of container of newNote

            set output to "RECORD_START" & linefeed
            set output to output & "id: " & noteId & linefeed
            set output to output & "title: " & noteTitle & linefeed
            set output to output & "folder: " & noteFolder & linefeed
            set output to output & "RECORD_END"

            return output
        end tell
    on error errMsg
        return "ERROR: " & errMsg
    end try
end run
