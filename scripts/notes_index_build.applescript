(*
@tool notes_index_build
@version 1.0
@input none
@output RECORD_START/RECORD_END delimited records with fields: id, title, folder, modified, tags (comma-separated)
@errors Outputs "ERROR: message" on failure
@description Full scan of all notes to build tag index. Extracts #hashtags from note body.
*)

on run argv
    set outputText to ""

    try
        tell application "Notes"
            set allNotes to every note
            set noteCount to count of allNotes

            -- Output note count first
            set outputText to outputText & "NOTE_COUNT: " & noteCount & linefeed

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

                -- Extract hashtags from plaintext
                set noteBody to plaintext of n
                set tagList to my extractHashtags(noteBody)
                set outputText to outputText & "tags: " & tagList & linefeed

                set outputText to outputText & "RECORD_END" & linefeed
            end repeat
        end tell

    on error errMsg
        return "ERROR: " & errMsg
    end try

    return outputText
end run

-- Extract hashtags from text (returns comma-separated list)
on extractHashtags(theText)
    set tagList to {}
    -- Normalize all whitespace (newlines, tabs, returns) to spaces so tags on their own lines are found
    set AppleScript's text item delimiters to {return, linefeed, return & linefeed, tab}
    set textParts to text items of theText
    set AppleScript's text item delimiters to " "
    set normalizedText to textParts as text
    set AppleScript's text item delimiters to ""
    set wordList to my splitText(normalizedText, " ")

    repeat with w in wordList
        set w to w as text
        -- Check if word starts with # and has valid tag characters
        if w starts with "#" and (length of w) > 1 then
            -- Clean up the tag (remove trailing punctuation)
            set cleanTag to my cleanHashtag(w)
            if cleanTag is not "" and cleanTag is not in tagList then
                set end of tagList to cleanTag
            end if
        end if
    end repeat

    -- Convert list to comma-separated string
    set AppleScript's text item delimiters to ","
    set tagString to tagList as text
    set AppleScript's text item delimiters to ""
    return tagString
end extractHashtags

-- Clean a hashtag (remove trailing punctuation, validate format)
on cleanHashtag(tag)
    -- Remove common trailing punctuation
    set validChars to "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-"
    set cleanedTag to "#"

    -- Skip the # and process rest of tag
    repeat with i from 2 to length of tag
        set c to character i of tag
        if c is in validChars then
            set cleanedTag to cleanedTag & c
        else
            -- Stop at first invalid character
            exit repeat
        end if
    end repeat

    -- Must have at least one character after #, and first char must be a letter
    if (length of cleanedTag) < 2 then
        return ""
    end if

    set firstChar to character 2 of cleanedTag
    if firstChar is not in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ" then
        return ""
    end if

    return cleanedTag
end cleanHashtag

-- Split text by delimiter
on splitText(theText, theDelimiter)
    set AppleScript's text item delimiters to theDelimiter
    set theItems to text items of theText
    set AppleScript's text item delimiters to ""
    return theItems
end splitText

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
