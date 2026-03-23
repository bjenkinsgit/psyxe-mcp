(*
@tool reminders_list
@version 1.0
@input list_name:string (argv 1), show_completed:string? (argv 2, default "false")
@output RECORD_START/RECORD_END delimited records
@fields id, name, list, due_date, completed, priority, notes, snippet
@errors Outputs "ERROR: message" on failure
*)

on run argv
    if (count of argv) < 1 then
        return "ERROR: Missing list name argument"
    end if

    set listName to item 1 of argv
    set showCompleted to "false"
    if (count of argv) > 1 then
        set showCompleted to item 2 of argv
    end if

    set outputText to ""

    try
        tell application "Reminders"
            try
                set targetList to list listName
            on error
                return "ERROR: List not found: " & listName
            end try

            if showCompleted is "true" then
                set allReminders to every reminder of targetList
            else
                set allReminders to (every reminder of targetList whose completed is false)
            end if

            repeat with r in allReminders
                set outputText to outputText & "RECORD_START" & linefeed
                set outputText to outputText & "id: " & (id of r) & linefeed
                set outputText to outputText & "name: " & (name of r) & linefeed
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

                -- Priority
                set outputText to outputText & "priority: " & (priority of r as string) & linefeed

                -- Full notes
                set noteBody to ""
                try
                    set noteBody to body of r
                    if noteBody is missing value then set noteBody to ""
                end try
                set cleanNotes to my replaceText(noteBody, linefeed, " ")
                set cleanNotes to my replaceText(cleanNotes, return, " ")
                set outputText to outputText & "notes: " & cleanNotes & linefeed

                -- Snippet (truncated preview)
                if length of cleanNotes > 200 then
                    set outputText to outputText & "snippet: " & (text 1 thru 200 of cleanNotes) & linefeed
                else
                    set outputText to outputText & "snippet: " & cleanNotes & linefeed
                end if

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
