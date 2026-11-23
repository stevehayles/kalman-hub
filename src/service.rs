//! The operations behind both the MCP tools and the REST API.

use crate::auth::Role;
use crate::model::{build_briefing, is_valid_status, view, Job, STATUSES};
use crate::AppState;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;

pub type OpResult = Result<Value, String>;

const MAX_JOBS: usize = 2000;
const MAX_TEXT: usize = 2000;

fn now_utc() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn claude_only(role: Role, what: &str) -> Result<(), String> {
    if role == Role::Claude {
        Ok(())
    } else {
        Err(format!("{what} can only be done with the Claude token"))
    }
}

fn parse<T: for<'de> Deserialize<'de>>(args: Value) -> Result<T, String> {
    serde_json::from_value(if args.is_null() { json!({}) } else { args })
        .map_err(|e| format!("invalid arguments: {e}"))
}

#[derive(Deserialize)]
struct SnapshotArgs {
    jobs: Vec<Job>,
    #[serde(default)]
    notes: Option<String>,
}

pub fn save_snapshot(state: &AppState, role: Role, args: Value) -> OpResult {
    claude_only(role, "Saving the tracker snapshot")?;
    let a: SnapshotArgs = parse(args)?;
    if a.jobs.is_empty() {
        return Err("jobs is empty; refusing to wipe the tracker".into());
    }
    if a.jobs.len() > MAX_JOBS {
        return Err(format!("too many jobs ({}), the limit is {MAX_JOBS}", a.jobs.len()));
    }
    let mut seen = HashSet::new();
    for j in &a.jobs {
        if !seen.insert(j.id) {
            return Err(format!("job id {} appears more than once", j.id));
        }
        if j.task.trim().is_empty() {
            return Err(format!("job {} has no task text", j.id));
        }
        if !is_valid_status(&j.status) {
            return Err(format!("job {} has unknown status {:?}; use one of {:?}", j.id, j.status, STATUSES));
        }
        if j.end < j.start {
            return Err(format!("job {} ends before it starts", j.id));
        }
    }
    if a.notes.as_deref().map(str::len).unwrap_or(0) > MAX_TEXT * 4 {
        return Err("notes are too long".into());
    }
    let taken_at = now_utc();
    let n = state.db.replace_jobs(&a.jobs, a.notes.as_deref(), &taken_at)?;
    Ok(json!({ "saved_jobs": n, "taken_at": taken_at }))
}

#[derive(Deserialize, Default)]
struct BriefingArgs {
    #[serde(default)]
    window_days: Option<i64>,
}

pub fn get_briefing(state: &AppState, args: Value) -> OpResult {
    let a: BriefingArgs = parse(args)?;
    let window = a.window_days.unwrap_or(state.cfg.soon_days).clamp(1, 120);
    let today = state.cfg.today();
    let taken_at = state.db.meta("taken_at")?;
    let stale = match taken_at.as_deref().and_then(|t| DateTime::parse_from_rfc3339(t).ok()) {
        Some(t) => (Utc::now() - t.with_timezone(&Utc)).num_hours() > state.cfg.stale_hours,
        None => true,
    };
    let notes = state.db.meta("notes")?;
    let pending = state.db.pending_updates()?.len();
    let b = build_briefing(state.db.jobs()?, today, &state.cfg.tz_name, window, taken_at, stale, notes, pending);
    serde_json::to_value(b).map_err(|e| e.to_string())
}

#[derive(Deserialize, Default)]
pub struct ListArgs {
    #[serde(default)]
    pub workstream: Option<String>,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub flag: Option<String>,
}

pub fn list_jobs(state: &AppState, args: Value) -> OpResult {
    let a: ListArgs = parse(args)?;
    let today = state.cfg.today();
    let eq = |want: &Option<String>, have: &str| {
        want.as_deref().map(|w| w.eq_ignore_ascii_case(have)).unwrap_or(true)
    };
    let jobs: Vec<_> = state
        .db
        .jobs()?
        .into_iter()
        .map(|j| view(j, today, state.cfg.soon_days))
        .filter(|v| {
            eq(&a.workstream, &v.job.workstream)
                && eq(&a.owner, v.job.owner.as_deref().unwrap_or(""))
                && eq(&a.status, &v.job.status)
                && eq(&a.flag, v.flag)
        })
        .collect();
    Ok(json!({ "today": today, "count": jobs.len(), "jobs": jobs }))
}

#[derive(Deserialize)]
pub struct QueueArgs {
    pub job_id: i64,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

pub fn queue_update(state: &AppState, role: Role, args: Value) -> OpResult {
    let a: QueueArgs = parse(args)?;
    if a.status.is_none() && a.note.as_deref().map(str::trim).unwrap_or("").is_empty() {
        return Err("give a status, a note, or both".into());
    }
    if let Some(s) = &a.status {
        if !is_valid_status(s) {
            return Err(format!("unknown status {s:?}; use one of {STATUSES:?}"));
        }
    }
    if a.note.as_deref().map(str::len).unwrap_or(0) > MAX_TEXT {
        return Err(format!("note is longer than {MAX_TEXT} characters"));
    }
    if !state.db.job_exists(a.job_id)? {
        return Err(format!("no job with id {} in the latest snapshot", a.job_id));
    }
    let id = state.db.queue_update(a.job_id, a.status.as_deref(), a.note.as_deref(), role.name(), &now_utc())?;
    Ok(json!({ "queued_update_id": id, "applies_on": "the next Claude sync" }))
}

pub fn list_pending(state: &AppState) -> OpResult {
    let updates = state.db.pending_updates()?;
    Ok(json!({ "count": updates.len(), "updates": updates }))
}

#[derive(Deserialize)]
struct AppliedArgs {
    ids: Vec<i64>,
    #[serde(default)]
    result: Option<String>,
}

pub fn mark_applied(state: &AppState, role: Role, args: Value) -> OpResult {
    claude_only(role, "Marking updates as applied")?;
    let a: AppliedArgs = parse(args)?;
    let n = state.db.mark_applied(&a.ids, a.result.as_deref(), &now_utc())?;
    Ok(json!({ "marked_applied": n }))
}
