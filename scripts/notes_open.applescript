(*
@tool notes_open
@version 1.2
@input note_id:string (argv 1)
@output Opens the note in Notes.app, returns success message or error
@errors Outputs "ERROR: message" on failure
*)

on run argv
    -- Validate arguments
    if (count of argv) < 1 then
        return "ERROR: Missing note ID argument"
    end if

    set noteId to item 1 of argv

    try
        -- Step 1: Ensure Notes is running and has a window open
        tell application "Notes"
            activate
        end tell
        delay 0.5

        -- Ensure Notes window exists and is frontmost
        tell application "System Events"
            tell process "Notes"
                set frontmost to true
                delay 0.3

                -- If no window exists, open one
                if (count of windows) = 0 then
                    tell application "Notes"
                        make new window
                    end tell
                    delay 0.5
                end if
            end tell
        end tell

        -- Step 2: Navigate to "All iCloud" folder via UI
        tell application "System Events"
            tell process "Notes"
                set frontmost to true
                delay 0.2

                -- Use keyboard shortcut to show folders (Cmd+Opt+S toggles sidebar if needed)
                -- Then use Cmd+1 to go to first account's "All" folder (All iCloud)
                keystroke "1" using {command down}
                delay 0.5
            end tell
        end tell

        -- Step 3: Select a random note first to prime the selection
        tell application "Notes"
            set allNotes to every note
            if (count of allNotes) > 0 then
                set firstNote to item 1 of allNotes
                show firstNote
                delay 0.3
            end if
        end tell

        -- Step 4: Now find and open the target note
        tell application "Notes"
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

            show targetNote
            set noteName to name of targetNote
        end tell

        -- Step 5: Press Return to open the selected note in the editor
        delay 0.3
        tell application "System Events"
            tell process "Notes"
                set frontmost to true
                delay 0.2
                key code 36 -- Return key
            end tell
        end tell

        return "OK: Opened note: " & noteName

    on error errMsg
        return "ERROR: " & errMsg
    end try
end run
