(*
@tool notes_list
@version 1.0
@input folder:string? (argv 1)
@output RECORD_START/RECORD_END delimited records
@fields id, title, folder, modified, snippet
@errors Outputs "ERROR: message" on failure
*)

on run argv
    set folderFilter to missing value
    if (count of argv) > 0 then
        set folderFilter to item 1 of argv
    end if

    set outputText to ""

    try
        tell application "Notes"
            -- Get notes based on folder filter
            if folderFilter is not missing value then
                try
                    set targetFolder to folder folderFilter
                    set allNotes to every note of targetFolder
                on error
                    return "ERROR: Folder not found: " & folderFilter
                end try
            else
                set allNotes to every note
            end if

            repeat with n in allNotes
                set outputText to outputText & "RECORD_START" & linefeed
                set outputText to outputText & "id: " & (id of n) & linefeed
                set outputText to outputText & "title: " & (name of n) & linefeed

                -- Get folder name safely
                try
                    set folderName to name of container of n
                on error
                    set folderName to "Notes"
                end try
                set outputText to outputText & "folder: " & folderName & linefeed

                -- Get modification date as ISO 8601
                set modDate to modification date of n
                set outputText to outputText & "modified: " & my formatDateISO(modDate) & linefeed

                -- Get snippet (first 200 chars of plaintext, remove newlines)
                set noteBody to plaintext of n
                if length of noteBody > 200 then
                    set noteBody to text 1 thru 200 of noteBody
                end if
                set noteBody to my replaceText(noteBody, linefeed, " ")
                set noteBody to my replaceText(noteBody, return, " ")
                set noteBody to my replaceText(noteBody, tab, " ")
                set outputText to outputText & "snippet: " & noteBody & linefeed

                set outputText to outputText & "RECORD_END" & linefeed
            end repeat
        end tell

    on error errMsg
        return "ERROR: " & errMsg
    end try

    return outputText
end run

-- Replace all occurrences of a string
on replaceText(theText, searchStr, replaceStr)
    set AppleScript's text item delimiters to searchStr
    set theItems to text items of theText
    set AppleScript's text item delimiters to replaceStr
    set theText to theItems as text
    set AppleScript's text item delimiters to ""
    return theText
end replaceText

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
