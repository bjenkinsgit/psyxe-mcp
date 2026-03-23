(*
@tool reminders_search
@version 1.0
@input query:string (argv 1), list_name:string? (argv 2)
@output RECORD_START/RECORD_END delimited records
@fields id, name, list, due_date, completed, priority, snippet
@errors Outputs "ERROR: message" on failure
*)

on run argv
    if (count of argv) < 1 then
        return "ERROR: Missing search query argument"
    end if

    set searchQuery to item 1 of argv
    set listFilter to missing value
    if (count of argv) > 1 then
        set listFilter to item 2 of argv
    end if

    set outputText to ""

    try
        tell application "Reminders"
            if listFilter is not missing value then
                try
                    set targetList to list listFilter
                    set matchingReminders to (every reminder of targetList whose name contains searchQuery)
                on error
                    return "ERROR: List not found: " & listFilter
                end try
            else
                set matchingReminders to (every reminder whose name contains searchQuery)
            end if

            repeat with r in matchingReminders
                set outputText to outputText & "RECORD_START" & linefeed
                set outputText to outputText & "id: " & (id of r) & linefeed
                set outputText to outputText & "name: " & (name of r) & linefeed

                -- Get list name
                try
                    set listName to name of container of r
                on error
                    set listName to "Reminders"
                end try
                set outputText to outputText & "list: " & listName & linefeed

                -- Due date
                if due date of r is not missing value then
                    set outputText to outputText & "due_date: " & my formatDateISO(due date of r) & linefeed
                else
                    set outputText to outputText & "due_date: " & linefeed
                end if

                -- Completed
                if completed of r then
                    set outputText to outputText & "completed: true" & linefeed
                else
                    set outputText to outputText & "completed: false" & linefeed
                end if

                -- Priority (0=none, 1=high, 5=medium, 9=low)
                set outputText to outputText & "priority: " & (priority of r as string) & linefeed

                -- Snippet from body/notes
                set noteBody to ""
                try
                    set noteBody to body of r
                    if noteBody is missing value then set noteBody to ""
                end try
                if length of noteBody > 200 then
                    set noteBody to text 1 thru 200 of noteBody
                end if
                set noteBody to my replaceText(noteBody, linefeed, " ")
                set noteBody to my replaceText(noteBody, return, " ")
                set outputText to outputText & "snippet: " & noteBody & linefeed

                set outputText to outputText & "RECORD_END" & linefeed
            end repeat
        end tell

    on error errMsg
        return "ERROR: " & errMsg
    end try

    return outputText
end run

on replaceText(theText, searchStr, replaceStr)
    set AppleScript's text item delimiters to searchStr
    set theItems to text items of theText
    set AppleScript's text item delimiters to replaceStr
    set theText to theItems as text
    set AppleScript's text item delimiters to ""
    return theText
end replaceText

on formatDateISO(theDate)
    set y to year of theDate as string
    set m to my padZero(month of theDate as integer)
    set d to my padZero(day of theDate)
    set h to my padZero(hours of theDate)
    set min to my padZero(minutes of theDate)
    set s to my padZero(seconds of theDate)
    return y & "-" & m & "-" & d & "T" & h & ":" & min & ":" & s & "Z"
end formatDateISO

on padZero(n)
    if n < 10 then
        return "0" & (n as string)
    else
        return n as string
    end if
end padZero
