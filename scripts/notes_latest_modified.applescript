(*
@tool notes_latest_modified
@version 1.0
@input none
@output Single line: "LATEST: <ISO 8601 timestamp>"
@errors Outputs "ERROR: message" on failure
@description Get the most recent modification date across all notes for staleness check
*)

on run argv
    try
        tell application "Notes"
            set allDates to modification date of every note
        end tell

        -- Find the maximum date
        set latestDate to item 1 of allDates
        repeat with d in allDates
            if d > latestDate then
                set latestDate to d
            end if
        end repeat

        return "LATEST: " & my formatDateISO(latestDate)
    on error errMsg
        return "ERROR: " & errMsg
    end try
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
