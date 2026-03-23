(*
@tool reminders_get
@version 1.0
@input reminder_name:string (argv 1), list_name:string? (argv 2)
@output Key-value fields for a single reminder
@fields name, list, due_date, completed, priority, notes, created, modified
@errors Outputs "ERROR: message" on failure
*)

on run argv
    if (count of argv) < 1 then
        return "ERROR: Missing reminder name argument"
    end if

    set reminderName to item 1 of argv
    set listFilter to missing value
    if (count of argv) > 1 then
        set listFilter to item 2 of argv
    end if

    set outputText to ""

    try
        tell application "Reminders"
            set foundReminder to missing value

            if listFilter is not missing value then
                try
                    set targetList to list listFilter
                    set matches to (every reminder of targetList whose name is reminderName)
                    if (count of matches) > 0 then
                        set foundReminder to item 1 of matches
                    end if
                on error
                    return "ERROR: List not found: " & listFilter
                end try
            else
                set matches to (every reminder whose name is reminderName)
                if (count of matches) > 0 then
                    set foundReminder to item 1 of matches
                end if
            end if

            if foundReminder is missing value then
                return "ERROR: Reminder not found: " & reminderName
            end if

            set outputText to outputText & "name: " & (name of foundReminder) & linefeed

            -- List name
            try
                set listName to name of container of foundReminder
            on error
                set listName to "Reminders"
            end try
            set outputText to outputText & "list: " & listName & linefeed

            -- Due date
            if due date of foundReminder is not missing value then
                set outputText to outputText & "due_date: " & my formatDateISO(due date of foundReminder) & linefeed
            else
                set outputText to outputText & "due_date: " & linefeed
            end if

            -- Completed
            if completed of foundReminder then
                set outputText to outputText & "completed: true" & linefeed
            else
                set outputText to outputText & "completed: false" & linefeed
            end if

            -- Priority
            set outputText to outputText & "priority: " & (priority of foundReminder as string) & linefeed

            -- Notes/body
            set noteBody to ""
            try
                set noteBody to body of foundReminder
                if noteBody is missing value then set noteBody to ""
            end try
            set outputText to outputText & "notes: " & noteBody & linefeed

            -- Creation date
            set outputText to outputText & "created: " & my formatDateISO(creation date of foundReminder) & linefeed

            -- Modification date
            set outputText to outputText & "modified: " & my formatDateISO(modification date of foundReminder) & linefeed

        end tell

    on error errMsg
        return "ERROR: " & errMsg
    end try

    return outputText
end run

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
