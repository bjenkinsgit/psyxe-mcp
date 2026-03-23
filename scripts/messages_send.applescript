(*
@tool notify_human
@version 1.1
@input recipient:string (argv 1), message:string (argv 2), service:string (argv 3, optional: "iMessage" or "SMS")
@output "OK: Sent message to ..." on success
@errors Outputs "ERROR: message" on failure
*)

on run argv
    if (count of argv) < 2 then
        return "ERROR: Missing arguments. Usage: recipient message [iMessage|SMS]"
    end if

    set recipientId to item 1 of argv
    set messageText to item 2 of argv

    -- Default to iMessage if no service specified
    set preferredService to "iMessage"
    if (count of argv) ≥ 3 then
        set preferredService to item 3 of argv
    end if

    -- Try preferred service first, then fall back
    if preferredService is "SMS" then
        -- SMS preferred: try SMS first, then iMessage, then unspecified
        try
            tell application "Messages"
                set targetBuddy to buddy recipientId of (service 1 whose service type is SMS)
                send messageText to targetBuddy
            end tell
            return "OK: Sent message to " & recipientId & " (SMS)"
        on error
            try
                tell application "Messages"
                    set targetBuddy to buddy recipientId of (service 1 whose service type is iMessage)
                    send messageText to targetBuddy
                end tell
                return "OK: Sent message to " & recipientId & " (iMessage fallback)"
            on error
                try
                    tell application "Messages"
                        send messageText to buddy recipientId
                    end tell
                    return "OK: Sent message to " & recipientId
                on error errMsg
                    return "ERROR: " & errMsg
                end try
            end try
        end try
    else
        -- iMessage preferred: try iMessage first, then SMS, then unspecified
        try
            tell application "Messages"
                set targetBuddy to buddy recipientId of (service 1 whose service type is iMessage)
                send messageText to targetBuddy
            end tell
            return "OK: Sent message to " & recipientId & " (iMessage)"
        on error
            try
                tell application "Messages"
                    set targetBuddy to buddy recipientId of (service 1 whose service type is SMS)
                    send messageText to targetBuddy
                end tell
                return "OK: Sent message to " & recipientId & " (SMS fallback)"
            on error
                try
                    tell application "Messages"
                        send messageText to buddy recipientId
                    end tell
                    return "OK: Sent message to " & recipientId
                on error errMsg
                    return "ERROR: " & errMsg
                end try
            end try
        end try
    end if
end run
