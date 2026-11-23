use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

pub const STATUSES: [&str; 5] = ["Not started", "In progress", "Blocked", "Done", "Not needed"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: i64,
    pub task: String,
    pub workstream: String,
    pub start: NaiveDate,
    pub end: NaiveDate,
    pub status: String,
    #[serde(default)]
    pub depends_on: Option<String>,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub plan_section: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobView {
    #[serde(flatten)]
    pub job: Job,
    pub flag: &'static str,
    pub days_to_start: i64,
    pub days_to_end: i64,
}

/// A change Pepper (or Claude) has asked for. `kind` is "change" for an existing job or
/// "new" for a job to add. Only the fields that should change are set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeRequest {
    pub id: i64,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<NaiveDate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<NaiveDate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depends_on: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workstream: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_section: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub requested_by: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub applied_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
}

#[derive(Debug, Default)]
pub struct NewChange {
    pub kind: &'static str,
    pub job_id: Option<i64>,
    pub status: Option<String>,
    pub start: Option<NaiveDate>,
    pub end: Option<NaiveDate>,
    pub owner: Option<String>,
    pub depends_on: Option<String>,
    pub task: Option<String>,
    pub workstream: Option<String>,
    pub plan_section: Option<String>,
    pub note: Option<String>,
}

pub fn is_valid_status(s: &str) -> bool {
    STATUSES.contains(&s)
}

pub fn is_gate(job: &Job) -> bool {
    job.workstream.eq_ignore_ascii_case("gate")
}

/// Same rules as the Flag column in the Claude tracker, so both agree.
pub fn flag(job: &Job, today: NaiveDate, soon_days: i64) -> &'static str {
    match job.status.as_str() {
        "Done" => return "Done",
        "Not needed" => return "",
        "Blocked" => return "Blocked",
        _ => {}
    }
    if today > job.end {
        return "Overdue";
    }
    if today >= job.start {
        return if job.status == "In progress" { "Underway" } else { "Start now" };
    }
    if (job.start - today).num_days() <= soon_days {
        return "Starts soon";
    }
    ""
}

pub fn view(job: Job, today: NaiveDate, soon_days: i64) -> JobView {
    let flag = flag(&job, today, soon_days);
    let days_to_start = (job.start - today).num_days();
    let days_to_end = (job.end - today).num_days();
    JobView { job, flag, days_to_start, days_to_end }
}

#[derive(Debug, Serialize)]
pub struct Briefing {
    pub today: NaiveDate,
    pub timezone: String,
    pub window_days: i64,
    pub snapshot_taken_at: Option<String>,
    pub snapshot_is_stale: bool,
    pub notes_from_claude: Option<String>,
    pub overdue: Vec<JobView>,
    pub blocked: Vec<JobView>,
    pub start_now: Vec<JobView>,
    pub starting_soon: Vec<JobView>,
    pub finishing_this_week: Vec<JobView>,
    pub underway_count: usize,
    pub next_gate: Option<JobView>,
    pub pending_changes: Vec<ChangeRequest>,
    pub recent_activity: Vec<ChangeRequest>,
    pub summary: String,
}

#[allow(clippy::too_many_arguments)]
pub fn build_briefing(
    jobs: Vec<Job>,
    today: NaiveDate,
    timezone: &str,
    window_days: i64,
    snapshot_taken_at: Option<String>,
    snapshot_is_stale: bool,
    notes_from_claude: Option<String>,
    pending_changes: Vec<ChangeRequest>,
    recent_activity: Vec<ChangeRequest>,
) -> Briefing {
    let mut overdue = Vec::new();
    let mut blocked = Vec::new();
    let mut start_now = Vec::new();
    let mut starting_soon = Vec::new();
    let mut finishing_this_week = Vec::new();
    let mut underway_count = 0;
    let mut next_gate: Option<JobView> = None;

    for job in jobs {
        let v = view(job, today, window_days);
        if is_gate(&v.job) && v.job.status != "Done" && v.days_to_end >= 0 {
            let earlier = next_gate.as_ref().map(|g| v.job.end < g.job.end).unwrap_or(true);
            if earlier {
                next_gate = Some(v.clone());
            }
        }
        match v.flag {
            "Overdue" => overdue.push(v),
            "Blocked" => blocked.push(v),
            "Start now" => start_now.push(v),
            "Starts soon" => starting_soon.push(v),
            "Underway" => {
                underway_count += 1;
                if v.days_to_end <= 7 {
                    finishing_this_week.push(v);
                }
            }
            _ => {}
        }
    }

    overdue.sort_by_key(|v| v.job.end);
    start_now.sort_by_key(|v| v.job.start);
    starting_soon.sort_by_key(|v| v.job.start);
    finishing_this_week.sort_by_key(|v| v.job.end);

    let mut parts = Vec::new();
    if !overdue.is_empty() {
        parts.push(format!("{} overdue", overdue.len()));
    }
    if !blocked.is_empty() {
        parts.push(format!("{} blocked", blocked.len()));
    }
    if !start_now.is_empty() {
        parts.push(format!("{} should start now", start_now.len()));
    }
    if !starting_soon.is_empty() {
        parts.push(format!("{} starting in the next {} days", starting_soon.len(), window_days));
    }
    if !finishing_this_week.is_empty() {
        parts.push(format!("{} due to finish this week", finishing_this_week.len()));
    }
    let mut summary = if parts.is_empty() {
        "Nothing needs attention today.".to_string()
    } else {
        format!("{}.", parts.join(", "))
    };
    if let Some(g) = &next_gate {
        summary.push_str(&format!(" Next gate: {} on {} ({} days).", g.job.task, g.job.end, g.days_to_end));
    }
    if !pending_changes.is_empty() {
        summary.push_str(&format!(" {} change(s) waiting for the next sync.", pending_changes.len()));
    }
    if snapshot_is_stale {
        summary.push_str(" Warning: the tracker snapshot is out of date, so check the Claude tracker.");
    }

    Briefing {
        today,
        timezone: timezone.to_string(),
        window_days,
        snapshot_taken_at,
        snapshot_is_stale,
        notes_from_claude,
        overdue,
        blocked,
        start_now,
        starting_soon,
        finishing_this_week,
        underway_count,
        next_gate,
        pending_changes,
        recent_activity,
        summary,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(start: &str, end: &str, status: &str) -> Job {
        Job {
            id: 1,
            task: "t".into(),
            workstream: "Product".into(),
            start: start.parse().unwrap(),
            end: end.parse().unwrap(),
            status: status.into(),
            depends_on: None,
            owner: None,
            plan_section: None,
        }
    }

    #[test]
    fn flags_match_tracker_rules() {
        let today: NaiveDate = "2026-12-10".parse().unwrap();
        assert_eq!(flag(&job("2026-12-01", "2026-12-05", "In progress"), today, 21), "Overdue");
        assert_eq!(flag(&job("2026-12-01", "2026-12-20", "Not started"), today, 21), "Start now");
        assert_eq!(flag(&job("2026-12-01", "2026-12-20", "In progress"), today, 21), "Underway");
        assert_eq!(flag(&job("2026-12-20", "2026-12-30", "Not started"), today, 21), "Starts soon");
        assert_eq!(flag(&job("2027-03-01", "2027-03-30", "Not started"), today, 21), "");
        assert_eq!(flag(&job("2026-12-01", "2026-12-05", "Done"), today, 21), "Done");
        assert_eq!(flag(&job("2026-12-01", "2026-12-05", "Blocked"), today, 21), "Blocked");
    }
}
