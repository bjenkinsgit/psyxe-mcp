(*
@tool reminders_create
@version 1.0
@input title:string (argv 1), list_name:string? (argv 2), due_date:string? (argv 3, YYYY-MM-DD), notes:string? (argv 4), priority:string? (argv 5, 0/1/5/9)
@output "OK: Created ..." on success
@errors Outputs "ERROR: message" on failure
*)

on run argv
    if (count of argv) < 1 then
        return "ERROR: Missing title argument"
    end if

    set reminderTitle to item 1 of argv
    set listName to missing value
    set dueStr to missing value
    set noteText to missing value
    set priorityVal to 0

    if (count of argv) > 1 and item 2 of argv is not "" then
        set listName to item 2 of argv
    end if
    if (count of argv) > 2 and item 3 of argv is not "" then
        set dueStr to item 3 of argv
    end if
    if (count of argv) > 3 and item 4 of argv is not "" then
        set noteText to item 4 of argv
    end if
    if (count of argv) > 4 and item 5 of argv is not "" then
        set priorityVal to (item 5 of argv) as integer
    end if

    try
        tell application "Reminders"
            -- Get target list
            set targetList to missing value
            if listName is not missing value then
                try
                    set targetList to list listName
                on error
                    return "ERROR: List not found: " & listName
                end try
            else
                set targetList to default list
            end if

            -- Create reminder properties
            set reminderProps to {name:reminderTitle, priority:priorityVal}

            -- Create the reminder
            set newReminder to make new reminder at targetList with properties reminderProps

            -- Set due date if provided (YYYY-MM-DD)
            if dueStr is not missing value then
                set theYear to (text 1 thru 4 of dueStr) as integer
                set theMonth to (text 6 thru 7 of dueStr) as integer
                set theDay to (text 9 thru 10 of dueStr) as integer
                set dueDate to current date
                set year of dueDate to theYear
                set month of dueDate to theMonth
                set day of dueDate to theDay
                set hours of dueDate to 9
                set minutes of dueDate to 0
                set seconds of dueDate to 0
                set due date of newReminder to dueDate
            end if

            -- Set notes if provided
            if noteText is not missing value then
                set body of newReminder to noteText
            end if

            return "OK: Created reminder '" & reminderTitle & "' in list '" & (name of targetList) & "'"
        end tell

    on error errMsg
        return "ERROR: " & errMsg
    end try
end run
