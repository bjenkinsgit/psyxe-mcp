//! In-process EventKit FFI for Apple Reminders.
//!
//! Calls EventKit directly from the main Rust process via Objective-C FFI,
//! bypassing helper binaries. This works in sandboxed builds because the main
//! process has the correct bundle ID and TCC authorization, while helper
//! binaries have `bundle_id=nil` which breaks EventKit's XPC attribution.

use serde_json::{json, Value};
use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2::AnyThread;
use objc2_event_kit::*;
use objc2_foundation::*;

// ============================================================================
// Store management
// ============================================================================

/// Create a new EKEventStore and verify authorization.
fn new_authorized_store() -> Result<Retained<EKEventStore>, String> {
    let store = unsafe { EKEventStore::new() };

    let status =
        unsafe { EKEventStore::authorizationStatusForEntityType(EKEntityType::Reminder) };

    if status == EKAuthorizationStatus::FullAccess {
        tracing::debug!("EventKit FFI: authorization status is FullAccess");
        return Ok(store);
    }

    if status == EKAuthorizationStatus::NotDetermined {
        tracing::info!("EventKit FFI: requesting reminders access");
        let (tx, rx) = mpsc::channel();
        let block: RcBlock<dyn Fn(Bool, *mut NSError)> =
            RcBlock::new(move |granted: Bool, _error: *mut NSError| {
                let _ = tx.send(granted.as_bool());
            });
        unsafe {
            store.requestFullAccessToRemindersWithCompletion(
                (&*block as *const block2::DynBlock<dyn Fn(Bool, *mut NSError)>)
                    as *mut block2::DynBlock<dyn Fn(Bool, *mut NSError)>,
            );
        }
        let granted = rx
            .recv_timeout(Duration::from_secs(60))
            .map_err(|e| format!("Authorization timeout: {}", e))?;
        if granted {
            return Ok(store);
        }
        return Err("Reminders access denied by user".to_string());
    }

    Err(format!(
        "Reminders access not available (status={:?})",
        status
    ))
}

// ============================================================================
// Date helpers
// ============================================================================

fn format_date_components(components: &NSDateComponents) -> String {
    let calendar = NSCalendar::currentCalendar();
    match calendar.dateFromComponents(components) {
        Some(date) => format_nsdate(&date),
        None => String::new(),
    }
}

fn format_nsdate(date: &NSDate) -> String {
    let timestamp = date.timeIntervalSince1970();
    chrono::DateTime::from_timestamp(timestamp as i64, 0)
        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_default()
}

fn parse_date_str_to_components(s: &str) -> Option<Retained<NSDateComponents>> {
    let trimmed = s.trim();
    let parts: Vec<&str> = trimmed.splitn(2, ' ').collect();
    let date_pieces: Vec<&str> = parts[0].split('-').collect();
    if date_pieces.len() != 3 {
        return None;
    }

    let year: isize = date_pieces[0].parse().ok()?;
    let month: isize = date_pieces[1].parse().ok()?;
    let day: isize = date_pieces[2].parse().ok()?;

    let dc = NSDateComponents::new();
    dc.setYear(year);
    dc.setMonth(month);
    dc.setDay(day);

    if parts.len() == 2 {
        let time_pieces: Vec<&str> = parts[1].split(':').collect();
        if time_pieces.len() >= 2 {
            if let (Ok(hour), Ok(minute)) = (
                time_pieces[0].parse::<isize>(),
                time_pieces[1].parse::<isize>(),
            ) {
                dc.setHour(hour);
                dc.setMinute(minute);
            } else {
                dc.setHour(9);
            }
        } else {
            dc.setHour(9);
        }
    } else {
        dc.setHour(9);
    }

    Some(dc)
}

// ============================================================================
// Data extraction helpers
// ============================================================================

fn snippet(notes: &str) -> String {
    if notes.is_empty() {
        return String::new();
    }
    let clean = notes.replace('\n', " ").replace('\r', " ");
    if clean.len() > 200 {
        clean[..200].to_string()
    } else {
        clean
    }
}

/// Extract reminder summary fields (for list/search results).
fn extract_reminder_summary(r: &EKReminder) -> Value {
    let title = unsafe { r.title() }.to_string();
    let list = unsafe { r.calendar() }
        .map(|c| unsafe { c.title() }.to_string())
        .unwrap_or_default();
    let notes = unsafe { r.notes() }
        .map(|s| s.to_string())
        .unwrap_or_default();
    let completed = unsafe { r.isCompleted() };
    let priority = unsafe { r.priority() } as u8;
    let due_date = unsafe { r.dueDateComponents() }
        .map(|dc| format_date_components(&dc))
        .unwrap_or_default();
    let start_date = unsafe { r.startDateComponents() }
        .map(|dc| format_date_components(&dc))
        .unwrap_or_default();
    let url = unsafe { r.URL() }
        .and_then(|u| u.absoluteString())
        .map(|s| s.to_string())
        .unwrap_or_default();
    let location = unsafe { r.location() }
        .map(|s| s.to_string())
        .unwrap_or_default();
    let snip = snippet(&notes);

    json!({
        "name": title,
        "list": list,
        "due_date": due_date,
        "completed": completed,
        "priority": priority,
        "notes": notes,
        "snippet": snip,
        "url": url,
        "location": location,
        "start_date": start_date,
    })
}

/// Extract full reminder detail (for get).
fn extract_reminder_detail(r: &EKReminder) -> Value {
    let title = unsafe { r.title() }.to_string();
    let list = unsafe { r.calendar() }
        .map(|c| unsafe { c.title() }.to_string())
        .unwrap_or_default();
    let notes = unsafe { r.notes() }
        .map(|s| s.to_string())
        .unwrap_or_default();
    let completed = unsafe { r.isCompleted() };
    let priority = unsafe { r.priority() } as u8;
    let due_date = unsafe { r.dueDateComponents() }
        .map(|dc| format_date_components(&dc))
        .unwrap_or_default();
    let start_date = unsafe { r.startDateComponents() }
        .map(|dc| format_date_components(&dc))
        .unwrap_or_default();
    let url = unsafe { r.URL() }
        .and_then(|u| u.absoluteString())
        .map(|s| s.to_string())
        .unwrap_or_default();
    let location = unsafe { r.location() }
        .map(|s| s.to_string())
        .unwrap_or_default();
    let created = unsafe { r.creationDate() }
        .map(|d| format_nsdate(&d))
        .unwrap_or_default();
    let modified = unsafe { r.lastModifiedDate() }
        .map(|d| format_nsdate(&d))
        .unwrap_or_default();
    let completion_date = unsafe { r.completionDate() }
        .map(|d| format_nsdate(&d))
        .unwrap_or_default();

    // Alarms
    let alarms = extract_alarms(r);
    // Recurrence
    let recurrence = extract_recurrence(r);

    json!({
        "name": title,
        "list": list,
        "due_date": due_date,
        "completed": completed,
        "priority": priority,
        "notes": notes,
        "created": created,
        "modified": modified,
        "url": url,
        "location": location,
        "start_date": start_date,
        "completion_date": completion_date,
        "alarms": alarms,
        "recurrence": recurrence,
    })
}

fn extract_alarms(r: &EKReminder) -> Vec<Value> {
    let Some(alarms) = (unsafe { r.alarms() }) else {
        return vec![];
    };
    let mut result = Vec::new();
    for alarm in alarms.to_vec() {
        if let Some(loc) = unsafe { alarm.structuredLocation() } {
            if let Some(geo) = unsafe { loc.geoLocation() } {
                let coord = unsafe { geo.coordinate() };
                let proximity = match unsafe { alarm.proximity() } {
                    EKAlarmProximity::Enter => "enter",
                    EKAlarmProximity::Leave => "leave",
                    _ => "none",
                };
                result.push(json!({
                    "type": "location",
                    "title": unsafe { loc.title() }.map(|s| s.to_string()).unwrap_or_default(),
                    "latitude": coord.latitude,
                    "longitude": coord.longitude,
                    "radius": unsafe { loc.radius() },
                    "proximity": proximity,
                }));
            }
        } else {
            let offset_minutes = (unsafe { -alarm.relativeOffset() } / 60.0) as i64;
            result.push(json!({
                "type": "time",
                "offset_minutes": offset_minutes,
            }));
        }
    }
    result
}

fn extract_recurrence(r: &EKReminder) -> Value {
    let Some(rules) = (unsafe { r.recurrenceRules() }) else {
        return Value::Null;
    };
    if rules.is_empty() {
        return Value::Null;
    }
    let rule = &rules.to_vec()[0];
    let freq = match unsafe { rule.frequency() } {
        EKRecurrenceFrequency::Daily => "daily",
        EKRecurrenceFrequency::Weekly => "weekly",
        EKRecurrenceFrequency::Monthly => "monthly",
        EKRecurrenceFrequency::Yearly => "yearly",
        _ => "unknown",
    };
    let interval = unsafe { rule.interval() };
    let mut dict = json!({ "frequency": freq, "interval": interval });
    if let Some(end) = unsafe { rule.recurrenceEnd() } {
        if let Some(end_date) = unsafe { end.endDate() } {
            dict["end_date"] = json!(format_nsdate(&end_date));
        } else {
            let count = unsafe { end.occurrenceCount() };
            if count > 0 {
                dict["occurrence_count"] = json!(count);
            }
        }
    }
    dict
}

// ============================================================================
// Find helpers
// ============================================================================

fn find_calendar_by_name(
    store: &EKEventStore,
    name: &str,
) -> Option<Retained<EKCalendar>> {
    let calendars =
        unsafe { store.calendarsForEntityType(EKEntityType::Reminder) };
    for cal in calendars.to_vec() {
        let title = unsafe { cal.title() }.to_string();
        if title.eq_ignore_ascii_case(name) {
            return Some(cal);
        }
    }
    None
}

/// Fetch reminders matching a predicate, blocking until complete.
fn fetch_reminders_sync(
    store: &EKEventStore,
    predicate: &NSPredicate,
) -> Result<Vec<Retained<EKReminder>>, String> {
    let (tx, rx) = mpsc::channel();
    let block: RcBlock<dyn Fn(*mut NSArray<EKReminder>)> =
        RcBlock::new(move |reminders: *mut NSArray<EKReminder>| {
            let result = if reminders.is_null() {
                Vec::new()
            } else {
                let arr = unsafe { &*reminders };
                arr.to_vec()
            };
            let _ = tx.send(result);
        });

    unsafe {
        store.fetchRemindersMatchingPredicate_completion(predicate, &*block);
    }

    rx.recv_timeout(Duration::from_secs(30))
        .map_err(|e| format!("Timeout fetching reminders: {}", e))
}

/// Find a reminder by title (case-insensitive) in a specific calendar or all calendars.
fn find_reminder(
    store: &EKEventStore,
    name: &str,
    list_name: Option<&str>,
) -> Result<Option<Retained<EKReminder>>, String> {
    let cals = if let Some(ln) = list_name.filter(|s| !s.is_empty()) {
        let cal = find_calendar_by_name(store, ln)
            .ok_or_else(|| format!("Reminder list '{}' not found", ln))?;
        Some(NSArray::from_retained_slice(&[cal]))
    } else {
        None
    };

    let predicate = unsafe {
        store.predicateForRemindersInCalendars(cals.as_deref())
    };
    let reminders = fetch_reminders_sync(store, &predicate)?;
    Ok(reminders.into_iter().find(|r| {
        let title = unsafe { r.title() }.to_string();
        title.eq_ignore_ascii_case(name)
    }))
}

// ============================================================================
// Apply fields helpers (for create/edit)
// ============================================================================

fn apply_common_fields(
    store: &EKEventStore,
    reminder: &EKReminder,
    args: &Value,
) {
    // Title
    if let Some(title) = args.get("title").and_then(|v| v.as_str()) {
        unsafe { reminder.setTitle(Some(&NSString::from_str(title))) };
    }
    // Notes
    if let Some(notes) = args.get("notes").and_then(|v| v.as_str()) {
        if notes.is_empty() {
            unsafe { reminder.setNotes(None) };
        } else {
            unsafe { reminder.setNotes(Some(&NSString::from_str(notes))) };
        }
    }
    // Due date
    if let Some(due) = args.get("due_date").and_then(|v| v.as_str()) {
        if due.is_empty() {
            unsafe { reminder.setDueDateComponents(None) };
        } else if let Some(dc) = parse_date_str_to_components(due) {
            unsafe { reminder.setDueDateComponents(Some(&dc)) };
        }
    }
    // Start date
    if let Some(start) = args.get("start_date").and_then(|v| v.as_str()) {
        if start.is_empty() {
            unsafe { reminder.setStartDateComponents(None) };
        } else if let Some(dc) = parse_date_str_to_components(start) {
            unsafe { reminder.setStartDateComponents(Some(&dc)) };
        }
    }
    // Priority
    if let Some(p) = args.get("priority") {
        let pval = if let Some(s) = p.as_str() {
            match s.to_lowercase().as_str() {
                "high" | "1" => 1usize,
                "medium" | "med" | "5" => 5,
                "low" | "9" => 9,
                "none" | "0" | "" => 0,
                _ => s.parse::<usize>().unwrap_or(0),
            }
        } else {
            p.as_u64().unwrap_or(0) as usize
        };
        unsafe { reminder.setPriority(pval) };
    }
    // Location (plain text)
    if let Some(loc) = args.get("location").and_then(|v| v.as_str()) {
        if loc.is_empty() {
            unsafe { reminder.setLocation(None) };
        } else {
            unsafe { reminder.setLocation(Some(&NSString::from_str(loc))) };
        }
    }
    // URL
    if let Some(url_str) = args.get("url").and_then(|v| v.as_str()) {
        if url_str.is_empty() {
            unsafe { reminder.setURL(None) };
        } else if let Some(url) =
            NSURL::URLWithString(&NSString::from_str(url_str))
        {
            unsafe { reminder.setURL(Some(&url)) };
        }
    }
    // Calendar/list change
    if let Some(new_list) = args
        .get("new_list")
        .or_else(|| args.get("list"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        if let Some(cal) = find_calendar_by_name(store, new_list) {
            unsafe { reminder.setCalendar(Some(&cal)) };
        }
    }
    // Time alarm
    if let Some(time_alarm) = args.get("time_alarm") {
        // Remove existing time alarms
        if let Some(alarms) = unsafe { reminder.alarms() } {
            for alarm in alarms.to_vec() {
                if unsafe { alarm.structuredLocation() }.is_none() {
                    unsafe { reminder.removeAlarm(&alarm) };
                }
            }
        }
        if let Some(offset) = time_alarm.get("offset_minutes").and_then(|v| v.as_i64()) {
            let alarm =
                unsafe { EKAlarm::alarmWithRelativeOffset(-(offset as f64) * 60.0) };
            unsafe { reminder.addAlarm(&alarm) };
        }
    }
    // Recurrence
    if let Some(rec) = args.get("recurrence") {
        // Remove existing rules
        if let Some(rules) = unsafe { reminder.recurrenceRules() } {
            for rule in rules.to_vec() {
                unsafe { reminder.removeRecurrenceRule(&rule) };
            }
        }
        if let Some(freq_str) = rec.get("frequency").and_then(|v| v.as_str()) {
            let freq = match freq_str.to_lowercase().as_str() {
                "daily" => Some(EKRecurrenceFrequency::Daily),
                "weekly" => Some(EKRecurrenceFrequency::Weekly),
                "monthly" => Some(EKRecurrenceFrequency::Monthly),
                "yearly" => Some(EKRecurrenceFrequency::Yearly),
                _ => None,
            };
            if let Some(freq) = freq {
                let interval = rec.get("interval").and_then(|v| v.as_i64()).unwrap_or(1);
                let end = if let Some(end_str) = rec.get("end_date").and_then(|v| v.as_str()) {
                    if !end_str.is_empty() {
                        parse_date_str_to_components(end_str).and_then(|dc| {
                            let cal = NSCalendar::currentCalendar();
                            cal.dateFromComponents(&dc)
                        }).map(|date| unsafe { EKRecurrenceEnd::recurrenceEndWithEndDate(&date) })
                    } else {
                        None
                    }
                } else if let Some(count) = rec.get("occurrence_count").and_then(|v| v.as_u64()) {
                    if count > 0 {
                        Some(unsafe { EKRecurrenceEnd::recurrenceEndWithOccurrenceCount(count as usize) })
                    } else {
                        None
                    }
                } else {
                    None
                };
                let rule = unsafe {
                    EKRecurrenceRule::initRecurrenceWithFrequency_interval_end(
                        EKRecurrenceRule::alloc(),
                        freq,
                        interval as isize,
                        end.as_deref(),
                    )
                };
                unsafe { reminder.addRecurrenceRule(&rule) };
            }
        }
    }
}

// ============================================================================
// Operations
// ============================================================================

fn list_lists(store: &EKEventStore) -> Result<String, String> {
    let calendars =
        unsafe { store.calendarsForEntityType(EKEntityType::Reminder) };

    let lists: Vec<Value> = calendars
        .to_vec()
        .iter()
        .map(|cal| {
            let name = unsafe { cal.title() }.to_string();
            let id = unsafe { cal.calendarIdentifier() }.to_string();
            json!({ "name": name, "id": id })
        })
        .collect();

    let count = lists.len();
    tracing::info!(count, "EventKit FFI: list_lists");
    Ok(serde_json::to_string(&json!({ "lists": lists, "count": count })).unwrap())
}

fn list_reminders(store: &EKEventStore, args: &Value) -> Result<String, String> {
    let list_name = args.get("list").and_then(|v| v.as_str()).unwrap_or("");
    let show_completed = args
        .get("show_completed")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let cals = if !list_name.is_empty() {
        let cal = find_calendar_by_name(store, list_name)
            .ok_or_else(|| format!("Reminder list '{}' not found", list_name))?;
        Some(NSArray::from_retained_slice(&[cal]))
    } else {
        None
    };

    let predicate = unsafe {
        store.predicateForRemindersInCalendars(cals.as_deref())
    };
    let reminders = fetch_reminders_sync(store, &predicate)?;

    let items: Vec<Value> = reminders
        .iter()
        .filter(|r| show_completed || !unsafe { r.isCompleted() })
        .map(|r| extract_reminder_summary(r))
        .collect();

    let count = items.len();
    Ok(serde_json::to_string(&json!({ "reminders": items, "count": count })).unwrap())
}

fn search_reminders(store: &EKEventStore, args: &Value) -> Result<String, String> {
    let query = args
        .get("query")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();
    let list_name = args.get("list").and_then(|v| v.as_str()).unwrap_or("");
    let show_completed = args
        .get("show_completed")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let cals = if !list_name.is_empty() {
        find_calendar_by_name(store, list_name)
            .map(|cal| NSArray::from_retained_slice(&[cal]))
    } else {
        None
    };

    let predicate = unsafe {
        store.predicateForRemindersInCalendars(cals.as_deref())
    };
    let reminders = fetch_reminders_sync(store, &predicate)?;

    let items: Vec<Value> = reminders
        .iter()
        .filter(|r| {
            if !show_completed && unsafe { r.isCompleted() } {
                return false;
            }
            if query.is_empty() {
                return true;
            }
            let title = unsafe { r.title() }.to_string().to_lowercase();
            let notes = unsafe { r.notes() }
                .map(|s| s.to_string().to_lowercase())
                .unwrap_or_default();
            title.contains(&query) || notes.contains(&query)
        })
        .map(|r| extract_reminder_summary(r))
        .collect();

    let count = items.len();
    Ok(serde_json::to_string(&json!({ "reminders": items, "count": count })).unwrap())
}

fn get_reminder(store: &EKEventStore, args: &Value) -> Result<String, String> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'name' parameter")?;
    let list_name = args.get("list").and_then(|v| v.as_str());

    let reminder = find_reminder(store, name, list_name)?
        .ok_or_else(|| format!("Reminder '{}' not found", name))?;

    let detail = extract_reminder_detail(&reminder);
    Ok(serde_json::to_string(&detail).unwrap())
}

fn create_reminder(store: &EKEventStore, args: &Value) -> Result<String, String> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'name' parameter")?;
    let list_name = args
        .get("list")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let reminder = unsafe { EKReminder::reminderWithEventStore(store) };
    unsafe { reminder.setTitle(Some(&NSString::from_str(name))) };

    // Set calendar
    if !list_name.is_empty() {
        if let Some(cal) = find_calendar_by_name(store, list_name) {
            unsafe { reminder.setCalendar(Some(&cal)) };
        } else {
            return Err(format!("Reminder list '{}' not found", list_name));
        }
    } else if let Some(default_cal) = unsafe { store.defaultCalendarForNewReminders() } {
        unsafe { reminder.setCalendar(Some(&default_cal)) };
    }

    apply_common_fields(store, &reminder, args);

    unsafe {
        store
            .saveReminder_commit_error(&reminder, true)
            .map_err(|e| format!("Failed to save reminder: {:?}", e))?;
    }

    let actual_list = unsafe { reminder.calendar() }
        .map(|c| unsafe { c.title() }.to_string())
        .unwrap_or_default();
    Ok(serde_json::to_string(&json!({
        "success": true, "name": name, "list": actual_list
    }))
    .unwrap())
}

fn complete_reminder(store: &EKEventStore, args: &Value) -> Result<String, String> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'name' parameter")?;
    let list_name = args.get("list").and_then(|v| v.as_str());

    let reminder = find_reminder(store, name, list_name)?
        .ok_or_else(|| format!("Reminder '{}' not found", name))?;

    unsafe { reminder.setCompleted(true) };

    unsafe {
        store
            .saveReminder_commit_error(&reminder, true)
            .map_err(|e| format!("Failed to save reminder: {:?}", e))?;
    }

    let actual_list = unsafe { reminder.calendar() }
        .map(|c| unsafe { c.title() }.to_string())
        .unwrap_or_default();
    Ok(serde_json::to_string(&json!({
        "success": true, "name": name, "list": actual_list, "completed": true
    }))
    .unwrap())
}

fn delete_reminder(store: &EKEventStore, args: &Value) -> Result<String, String> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'name' parameter")?;
    let list_name = args.get("list").and_then(|v| v.as_str());

    let reminder = find_reminder(store, name, list_name)?
        .ok_or_else(|| format!("Reminder '{}' not found", name))?;

    let actual_list = unsafe { reminder.calendar() }
        .map(|c| unsafe { c.title() }.to_string())
        .unwrap_or_default();

    unsafe {
        store
            .removeReminder_commit_error(&reminder, true)
            .map_err(|e| format!("Failed to delete reminder: {:?}", e))?;
    }

    Ok(serde_json::to_string(&json!({
        "success": true, "name": name, "list": actual_list
    }))
    .unwrap())
}

fn edit_reminder(store: &EKEventStore, args: &Value) -> Result<String, String> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'name' parameter")?;
    let list_name = args.get("list").and_then(|v| v.as_str());

    let reminder = find_reminder(store, name, list_name)?
        .ok_or_else(|| format!("Reminder '{}' not found", name))?;

    apply_common_fields(store, &reminder, args);

    unsafe {
        store
            .saveReminder_commit_error(&reminder, true)
            .map_err(|e| format!("Failed to save reminder: {:?}", e))?;
    }

    let actual_list = unsafe { reminder.calendar() }
        .map(|c| unsafe { c.title() }.to_string())
        .unwrap_or_default();
    let final_name = unsafe { reminder.title() }.to_string();
    Ok(serde_json::to_string(&json!({
        "success": true, "name": final_name, "list": actual_list
    }))
    .unwrap())
}

fn create_batch(store: &EKEventStore, args: &Value) -> Result<String, String> {
    let items = args
        .get("items")
        .and_then(|v| v.as_array())
        .ok_or("Missing 'items' array")?;
    let default_list = args.get("list").and_then(|v| v.as_str()).unwrap_or("");

    let mut results = Vec::new();
    for item in items {
        let mut item_args = item.clone();
        // Inherit list from batch if not set on individual item
        if item_args.get("list").and_then(|v| v.as_str()).unwrap_or("").is_empty()
            && !default_list.is_empty()
        {
            item_args["list"] = json!(default_list);
        }
        match create_reminder(store, &item_args) {
            Ok(r) => results.push(serde_json::from_str::<Value>(&r).unwrap_or(json!({"success": true}))),
            Err(e) => results.push(json!({"success": false, "error": e})),
        }
    }

    Ok(serde_json::to_string(&json!({
        "results": results, "count": results.len()
    }))
    .unwrap())
}

fn edit_batch(store: &EKEventStore, args: &Value) -> Result<String, String> {
    let items = args
        .get("items")
        .and_then(|v| v.as_array())
        .ok_or("Missing 'items' array")?;
    let default_list = args.get("list").and_then(|v| v.as_str()).unwrap_or("");

    let mut results = Vec::new();
    for item in items {
        let mut item_args = item.clone();
        if item_args.get("list").and_then(|v| v.as_str()).unwrap_or("").is_empty()
            && !default_list.is_empty()
        {
            item_args["list"] = json!(default_list);
        }
        match edit_reminder(store, &item_args) {
            Ok(r) => results.push(serde_json::from_str::<Value>(&r).unwrap_or(json!({"success": true}))),
            Err(e) => results.push(json!({"success": false, "error": e})),
        }
    }

    Ok(serde_json::to_string(&json!({
        "results": results, "count": results.len()
    }))
    .unwrap())
}

fn create_list(store: &EKEventStore, args: &Value) -> Result<String, String> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'name' parameter")?;

    let calendar = unsafe {
        EKCalendar::calendarForEntityType_eventStore(EKEntityType::Reminder, store)
    };
    unsafe { calendar.setTitle(&NSString::from_str(name)) };

    // Use the default calendar's source, or fall back to first available
    let source = unsafe { store.defaultCalendarForNewReminders() }
        .and_then(|c| unsafe { c.source() })
        .or_else(|| {
            let sources = unsafe { store.sources() };
            sources.to_vec().into_iter().next().and_then(|s| Some(s))
        });

    if let Some(src) = source {
        unsafe { calendar.setSource(Some(&src)) };
    }

    unsafe {
        store
            .saveCalendar_commit_error(&calendar, true)
            .map_err(|e| format!("Failed to create list: {:?}", e))?;
    }

    let id = unsafe { calendar.calendarIdentifier() }.to_string();
    Ok(serde_json::to_string(&json!({
        "success": true, "name": name, "id": id
    }))
    .unwrap())
}

fn delete_list(store: &EKEventStore, args: &Value) -> Result<String, String> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'name' parameter")?;

    let calendar = find_calendar_by_name(store, name)
        .ok_or_else(|| format!("Reminder list '{}' not found", name))?;

    unsafe {
        store
            .removeCalendar_commit_error(&calendar, true)
            .map_err(|e| format!("Failed to delete list: {:?}", e))?;
    }

    Ok(serde_json::to_string(&json!({ "success": true, "name": name })).unwrap())
}

// ============================================================================
// Public dispatcher
// ============================================================================

/// Execute an EventKit operation in-process.
/// Returns the same JSON format as the Swift helper binary.
pub fn execute(action: &str, args: &Value) -> Result<String, String> {
    let store = new_authorized_store()?;
    match action {
        "list_lists" => list_lists(&store),
        "list" => list_reminders(&store, args),
        "search" => search_reminders(&store, args),
        "get" => get_reminder(&store, args),
        "create" => create_reminder(&store, args),
        "create_batch" => create_batch(&store, args),
        "complete" => complete_reminder(&store, args),
        "delete" => delete_reminder(&store, args),
        "edit" => edit_reminder(&store, args),
        "edit_batch" => edit_batch(&store, args),
        "create_list" => create_list(&store, args),
        "delete_list" => delete_list(&store, args),
        _ => Err(format!("Unsupported EventKit action: {}", action)),
    }
}

/// List all reminder calendars via in-process EventKit.
/// Used by `fetch_all_reminder_lists()` as the first fallback.
pub fn list_all_calendars() -> Result<Vec<(String, String)>, String> {
    let store = new_authorized_store()?;
    let calendars =
        unsafe { store.calendarsForEntityType(EKEntityType::Reminder) };
    Ok(calendars
        .to_vec()
        .iter()
        .map(|cal| {
            let name = unsafe { cal.title() }.to_string();
            let id = unsafe { cal.calendarIdentifier() }.to_string();
            (name, id)
        })
        .collect())
}
