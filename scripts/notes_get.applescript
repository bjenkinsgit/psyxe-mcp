(*
@tool notes_get
@version 1.0
@input note_id:string (argv 1)
@output Single note with fields: id, title, folder, modified, body (wrapped in BODY_START/BODY_END)
@errors Outputs "ERROR: message" on failure
*)

on run argv
    -- Validate arguments
    if (count of argv) < 1 then
        return "ERROR: Missing note ID argument"
    end if

    set noteId to item 1 of argv
    set outputText to ""

    try
        tell application "Notes"
            -- Find note by ID
            set targetNote to missing value

            repeat with n in every note
                if (id of n) = noteId then
                    set targetNote to n
                    exit repeat
                end if
            end repeat

            if targetNote is missing value then
                return "ERROR: Note not found with ID: " & noteId
            end if

            -- Output note details
            set outputText to outputText & "id: " & (id of targetNote) & linefeed
            set outputText to outputText & "title: " & (name of targetNote) & linefeed

            -- Get folder name safely
            try
                set folderName to name of container of targetNote
            on error
                set folderName to "Notes"
            end try
            set outputText to outputText & "folder: " & folderName & linefeed

            -- Get modification date as ISO 8601
            set modDate to modification date of targetNote
            set outputText to outputText & "modified: " & my formatDateISO(modDate) & linefeed

            -- Get full body content (plaintext to strip HTML)
            set noteBody to plaintext of targetNote
            set outputText to outputText & "BODY_START" & linefeed
            set outputText to outputText & noteBody & linefeed
            set outputText to outputText & "BODY_END" & linefeed
        end tell

    on error errMsg
        return "ERROR: " & errMsg
    end try

    return outputText
end run

-- Format date as ISO 8601 (YYYY-MM-DDTHH:MM:SSZ)
on formatDateISO(theDate)
    set y to year of theDate as string
    set m to my padZero(month of theDate as integer)
    set d to my padZero(day of theDate)
    set h to my padZero(hours of theDate)
    set min to my padZero(minutes of theDate)
    set s to my padZero(seconds of theDate)
    return y & "-" & m & "-" & d & "T" & h & ":" & min & ":" & s & "Z"
end formatDateISO

-- Pad single digit numbers with leading zero
on padZero(n)
    if n < 10 then
        return "0" & (n as string)
    else
        return n as string
    end if
end padZero
