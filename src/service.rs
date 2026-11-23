//! The operations behind both the MCP tools and the REST API.

use crate::auth::Role;
use crate::model::{build_briefing, is_valid_status, view, Job, NewChange, STATUSES};
use crate::AppState;
use chrono::{DateTime, Duration, NaiveDate, Utc};
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
    let pending = state.db.pending_updates()?;
    let since = (Utc::now() - Duration::days(7)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let mut recent = state.db.updates_since(&since)?;
    recent.reverse();
    recent.truncate(15);
    let jobs = state.db.jobs()?;
    let mut underway: Vec<_> = jobs
        .iter()
        .cloned()
        .map(|j| view(j, today, window))
        .filter(|v| v.flag == "Underway")
        .collect();
    underway.sort_by_key(|v| v.job.end);
    let mut b = build_briefing(jobs, today, &state.cfg.tz_name, window, taken_at, stale, notes, pending, recent);
    b.telegram_text = crate::model::telegram_text(&b, &underway, &state.cfg.tracker_url);
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
    let all = state.db.jobs()?;
    let mut workstreams: Vec<String> = all.iter().map(|j| j.workstream.clone()).collect();
    workstreams.sort();
    workstreams.dedup();
    let mut owners: Vec<String> = all.iter().filter_map(|j| j.owner.clone()).collect();
    owners.sort();
    owners.dedup();
    let eq = |want: &Option<String>, have: &str| {
        want.as_deref().map(|w| w.eq_ignore_ascii_case(have)).unwrap_or(true)
    };
    let jobs: Vec<_> = all
        .into_iter()
        .map(|j| view(j, today, state.cfg.soon_days))
        .filter(|v| {
            eq(&a.workstream, &v.job.workstream)
                && eq(&a.owner, v.job.owner.as_deref().unwrap_or(""))
                && eq(&a.status, &v.job.status)
                && eq(&a.flag, v.flag)
        })
        .collect();
    Ok(json!({ "today": today, "count": jobs.len(), "jobs": jobs, "workstreams": workstreams, "owners": owners }))
}

fn clean(s: Option<String>) -> Option<String> {
    s.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

fn check_text(name: &str, v: &Option<String>, max: usize) -> Result<(), String> {
    if v.as_deref().map(str::len).unwrap_or(0) > max {
        return Err(format!("{name} is longer than {max} characters"));
    }
    Ok(())
}

fn check_status(s: &Option<String>) -> Result<(), String> {
    match s {
        Some(s) if !is_valid_status(s) => Err(format!("unknown status {s:?}; use one of {STATUSES:?}")),
        _ => Ok(()),
    }
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
    queue_job_change(state, role, json!({ "job_id": a.job_id, "status": a.status, "note": a.note }))
}

#[derive(Deserialize)]
struct ChangeArgs {
    job_id: i64,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    start: Option<NaiveDate>,
    #[serde(default)]
    end: Option<NaiveDate>,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    depends_on: Option<String>,
    #[serde(default)]
    note: Option<String>,
}

pub fn queue_job_change(state: &AppState, role: Role, args: Value) -> OpResult {
    let a: ChangeArgs = parse(args)?;
    let (status, owner, depends_on, note) = (clean(a.status), clean(a.owner), clean(a.depends_on), clean(a.note));
    if status.is_none() && a.start.is_none() && a.end.is_none() && owner.is_none() && depends_on.is_none() && note.is_none() {
        return Err("give at least one of status, start, end, owner, depends_on or note".into());
    }
    check_status(&status)?;
    check_text("note", &note, MAX_TEXT)?;
    check_text("owner", &owner, 100)?;
    check_text("depends_on", &depends_on, 200)?;
    let job = state
        .db
        .jobs()?
        .into_iter()
        .find(|j| j.id == a.job_id)
        .ok_or_else(|| format!("no job with id {} in the latest snapshot; use list_jobs to find the right id", a.job_id))?;
    let start = a.start.unwrap_or(job.start);
    let end = a.end.unwrap_or(job.end);
    if end < start {
        return Err(format!("that would make job {} end ({end}) before it starts ({start})", job.id));
    }
    let change = NewChange {
        kind: "change",
        job_id: Some(job.id),
        status,
        start: a.start,
        end: a.end,
        owner,
        depends_on,
        note,
        ..Default::default()
    };
    let id = state.db.queue_change(&change, role.name(), &now_utc())?;
    Ok(json!({
        "queued_change_id": id,
        "job": { "id": job.id, "task": job.task },
        "applies_on": "the next Claude sync (daily at 06:02 London time)"
    }))
}

#[derive(Deserialize)]
struct NewJobArgs {
    task: String,
    workstream: String,
    start: NaiveDate,
    end: NaiveDate,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    depends_on: Option<String>,
    #[serde(default)]
    plan_section: Option<String>,
    #[serde(default)]
    note: Option<String>,
    /// Add it even if a job with very similar wording exists.
    #[serde(default)]
    allow_similar: bool,
}

fn words(s: &str) -> HashSet<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 2)
        .map(str::to_string)
        .collect()
}

pub fn queue_new_job(state: &AppState, role: Role, args: Value) -> OpResult {
    let a: NewJobArgs = parse(args)?;
    let task = a.task.trim().to_string();
    let workstream = a.workstream.trim().to_string();
    if task.is_empty() || workstream.is_empty() {
        return Err("task and workstream are both needed".into());
    }
    check_text("task", &Some(task.clone()), 300)?;
    if a.end < a.start {
        return Err("the job would end before it starts".into());
    }
    let (owner, depends_on, plan_section, note) =
        (clean(a.owner), clean(a.depends_on), clean(a.plan_section), clean(a.note));
    check_text("note", &note, MAX_TEXT)?;

    let jobs = state.db.jobs()?;
    let known: HashSet<String> = jobs.iter().map(|j| j.workstream.to_lowercase()).collect();
    if !known.is_empty() && !known.contains(&workstream.to_lowercase()) {
        let mut list: Vec<_> = jobs.iter().map(|j| j.workstream.clone()).collect();
        list.sort();
        list.dedup();
        return Err(format!("unknown workstream {workstream:?}; use one of {list:?}"));
    }
    if !a.allow_similar {
        let mine = words(&task);
        let pending_new = state.db.pending_updates()?.into_iter().filter(|c| c.kind == "new");
        let existing = jobs
            .iter()
            .map(|j| (Some(j.id), j.task.clone()))
            .chain(pending_new.filter_map(|c| c.task.map(|t| (None, t))));
        for (id, other) in existing {
            let theirs = words(&other);
            let shared = mine.intersection(&theirs).count();
            let smaller = mine.len().min(theirs.len()).max(1);
            if shared * 100 / smaller >= 70 {
                let which = id.map(|i| format!("job {i}")).unwrap_or_else(|| "a new job already queued".into());
                return Err(format!(
                    "this looks like {which}: {other:?}. Update that one instead, or set allow_similar if it really is a separate job"
                ));
            }
        }
    }
    let change = NewChange {
        kind: "new",
        task: Some(task.clone()),
        workstream: Some(workstream),
        start: Some(a.start),
        end: Some(a.end),
        owner,
        depends_on,
        plan_section,
        note,
        status: Some("Not started".into()),
        ..Default::default()
    };
    let id = state.db.queue_change(&change, role.name(), &now_utc())?;
    Ok(json!({
        "queued_change_id": id,
        "task": task,
        "applies_on": "the next Claude sync (daily at 06:02 London time), which gives it a tracker ID"
    }))
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
