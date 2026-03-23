(*
@tool reminders_create_batch
@version 1.0
@input list_name:string (argv 1), items:string (argv 2, "|||"-delimited, each item "title:::notes")
@output "OK: Created N reminders in list ..."
@errors Outputs "ERROR: message" on failure

Batch creates multiple reminders in a single Reminders.app transaction.
Items are delimited by "|||" and each item is "title:::notes" (notes optional).
*)

on run argv
    if (count of argv) < 2 then
        return "ERROR: Missing arguments (list_name, items)"
    end if

    set listName to item 1 of argv
    set itemsStr to item 2 of argv

    -- Split items by "|||"
    set AppleScript's text item delimiters to "|||"
    set itemParts to text items of itemsStr
    set AppleScript's text item delimiters to ""

    set createdCount to 0

    try
        tell application "Reminders"
            -- Get target list
            set targetList to missing value
            try
                set targetList to list listName
            on error
                return "ERROR: List not found: " & listName
            end try

            repeat with anItem in itemParts
                set itemText to anItem as text

                -- Skip empty items
                if itemText is not "" then
                    -- Split by ":::" for title:::notes
                    set AppleScript's text item delimiters to ":::"
                    set parts to text items of itemText
                    set AppleScript's text item delimiters to ""

                    set itemTitle to item 1 of parts
                    set itemNotes to ""
                    if (count of parts) > 1 then
                        set itemNotes to item 2 of parts
                    end if

                    -- Create reminder
                    set newReminder to make new reminder at targetList with properties {name:itemTitle}
                    if itemNotes is not "" then
                        set body of newReminder to itemNotes
                    end if

                    set createdCount to createdCount + 1
                end if
            end repeat
        end tell

        return "OK: Created " & createdCount & " reminders in list '" & listName & "'"

    on error errMsg
        return "ERROR: " & errMsg & " (created " & createdCount & " before failure)"
    end try
end run
