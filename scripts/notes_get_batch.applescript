(*
@tool notes_get_batch
@version 1.0
@input ids_file:string (argv 1) - path to file containing note IDs, one per line
@output Multiple notes with RECORD_START/RECORD_END delimiters
@errors Outputs "ERROR: message" on failure, or per-note errors inline
*)

on run argv
    -- Validate arguments
    if (count of argv) < 1 then
        return "ERROR: Missing IDs file path argument"
    end if

    set idsFilePath to item 1 of argv
    set outputText to ""

    try
        -- Read note IDs from file
        set idsContent to read POSIX file idsFilePath as «class utf8»
        set noteIds to paragraphs of idsContent

        -- Filter empty lines
        set filteredIds to {}
        repeat with anId in noteIds
            if length of anId > 0 then
                set end of filteredIds to anId as string
            end if
        end repeat

        -- Build a lookup dictionary of all notes (single pass)
        tell application "Notes"
            set allNotes to every note
            set noteCount to count of allNotes

            -- Create parallel lists for fast lookup
            set noteIdList to {}
            set noteRefList to {}

            repeat with i from 1 to noteCount
                set n to item i of allNotes
                set end of noteIdList to (id of n) as string
                set end of noteRefList to n
            end repeat
        end tell

        -- Process each requested ID
        repeat with requestedId in filteredIds
            set requestedIdStr to requestedId as string
            set foundNote to missing value

            -- Find in our cached list
            repeat with i from 1 to (count of noteIdList)
                if (item i of noteIdList) = requestedIdStr then
                    set foundNote to item i of noteRefList
                    exit repeat
                end if
            end repeat

            if foundNote is missing value then
                -- Output error record for this note
                set outputText to outputText & "RECORD_START" & linefeed
                set outputText to outputText & "error: Note not found with ID: " & requestedIdStr & linefeed
                set outputText to outputText & "RECORD_END" & linefeed
            else
                -- Output note details
                set outputText to outputText & "RECORD_START" & linefeed

                tell application "Notes"
                    set outputText to outputText & "id: " & (id of foundNote) & linefeed
                    set outputText to outputText & "title: " & (name of foundNote) & linefeed

                    -- Get folder name safely
                    try
                        set folderName to name of container of foundNote
                    on error
                        set folderName to "Notes"
                    end try
                    set outputText to outputText & "folder: " & folderName & linefeed

                    -- Get modification date as ISO 8601
                    set modDate to modification date of foundNote
                    set outputText to outputText & "modified: " & my formatDateISO(modDate) & linefeed

                    -- Get full body content (plaintext to strip HTML)
                    set noteBody to plaintext of foundNote
                    set outputText to outputText & "BODY_START" & linefeed
                    set outputText to outputText & noteBody & linefeed
                    set outputText to outputText & "BODY_END" & linefeed
                end tell

                set outputText to outputText & "RECORD_END" & linefeed
            end if
        end repeat

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
