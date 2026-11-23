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
    /// The whole briefing as a ready-to-send Telegram message.
    pub telegram_text: String,
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
        let n = pending_changes.len();
        summary.push_str(&if n == 1 { " 1 change waiting for the next sync.".to_string() } else { format!(" {n} changes waiting for the next sync.") });
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
        telegram_text: String::new(),
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
    fn short_names_read_cleanly() {
        assert_eq!(short("Appoint accountant and solicitor (SEIS/EIS, IP transfer)"), "Appoint accountant and solicitor");
        assert_eq!(short("Start R&D records; agree claim approach"), "Start R&D records");
        assert_eq!(short("Chart processing pipeline: UKHO data to app tiles"), "Chart processing pipeline");
        assert_eq!(short("RaceVu app: UKHO chart licence signed"), "RaceVu app: UKHO chart licence signed");
        assert_eq!(short("Trademark clearance: RaceVu first (before hardware order), then Kalman, SailVu"), "Trademark clearance: RaceVu first, then Kalman…");
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

/// Groups workstreams into the six areas used in the tracker's colours.
fn area(workstream: &str) -> &'static str {
    match workstream.to_ascii_lowercase().as_str() {
        "product" | "software" => "Product",
        "hardware" | "compliance" | "manufacturing" | "documentation" => "Hardware",
        "marketing" | "campaign" | "website" | "sales" | "launch" => "Go-to-market",
        "funding" => "Funding",
        "co-founder" | "team" => "People",
        "gate" => "Gate",
        _ => "Company",
    }
}

const AREA_ORDER: [&str; 7] = ["Company", "Funding", "Product", "Hardware", "Go-to-market", "People", "Gate"];

/// A task name short enough for a phone. Drops anything in brackets and any second
/// clause after a semicolon, uses the part before a colon when that part says enough
/// on its own, and keeps at most six words.
fn short(task: &str) -> String {
    let mut plain = String::new();
    let mut depth = 0;
    for c in task.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ if depth == 0 => plain.push(c),
            _ => {}
        }
    }
    let plain = plain.split_whitespace().collect::<Vec<_>>().join(" ").replace(" ,", ",").replace(" :", ":");
    let mut text = plain.split(';').next().unwrap_or("").trim().to_string();
    if let Some((before, _)) = text.split_once(':') {
        if before.split_whitespace().count() >= 3 {
            text = before.trim().to_string();
        }
    }
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut s = words.iter().take(6).cloned().collect::<Vec<_>>().join(" ");
    s = s.trim_end_matches([':', ';', ',', '.', '-', ' ']).to_string();
    if words.len() > 6 {
        s.push('…');
    }
    s
}

fn day(d: NaiveDate) -> String {
    d.format("%a %-d %b").to_string()
}

fn short_day(d: NaiveDate) -> String {
    d.format("%-d %b").to_string()
}

fn item(v: &JobView) -> String {
    format!("#{} {}", v.job.id, short(&v.job.task))
}

/// The finished Telegram message, so the assistant only has to send it.
pub fn telegram_text(b: &Briefing, underway: &[JobView], tracker_url: &str) -> String {
    let mut out = Vec::new();
    out.push(format!("Kalman · {}", b.today.format("%a %-d %b")));

    let problems = b.overdue.len() + b.blocked.len();
    let mut counts = vec![format!("{} starting in the next 3 weeks", b.starting_soon.len()), format!("{} under way", b.underway_count)];
    counts.push(if problems == 0 { "nothing overdue".to_string() } else { format!("{problems} overdue or blocked") });
    out.push(counts.join(" · "));
    if b.snapshot_is_stale {
        out.push("⚠️ Tracker data is out of date: check the morning sync.".into());
    }

    let attention: Vec<String> = b
        .overdue
        .iter()
        .map(|v| format!("• {}: overdue since {}", item(v), short_day(v.job.end)))
        .chain(b.blocked.iter().map(|v| format!("• {}: blocked", item(v))))
        .chain(b.start_now.iter().map(|v| format!("• {}: should have started {}", item(v), short_day(v.job.start))))
        .collect();
    if !attention.is_empty() {
        out.push(String::new());
        out.push("⚠️ Needs attention".into());
        out.extend(attention);
    }

    if !b.finishing_this_week.is_empty() {
        out.push(String::new());
        out.push("⏳ Finishing this week".into());
        for v in &b.finishing_this_week {
            out.push(format!("• {}: due {}", item(v), short_day(v.job.end)));
        }
    }

    if !b.starting_soon.is_empty() {
        out.push(String::new());
        out.push(format!("📅 Starting in the next {} days", b.window_days));
        let mut dates: Vec<NaiveDate> = b.starting_soon.iter().map(|v| v.job.start).collect();
        dates.dedup();
        let shown = dates.len().min(6);
        for d in &dates[..shown] {
            let on_day: Vec<&JobView> = b.starting_soon.iter().filter(|v| v.job.start == *d).collect();
            out.push(day(*d));
            if on_day.len() <= 3 {
                for v in on_day {
                    out.push(format!("• {}", item(v)));
                }
            } else {
                for a in AREA_ORDER {
                    let names: Vec<String> = on_day.iter().filter(|v| area(&v.job.workstream) == a).map(|v| item(v)).collect();
                    if !names.is_empty() {
                        out.push(format!("• {a}: {}", names.join(", ")));
                    }
                }
            }
        }
        if dates.len() > shown {
            let rest = b.starting_soon.iter().filter(|v| dates[shown..].contains(&v.job.start)).count();
            out.push(format!("• +{rest} more after {}", short_day(dates[shown - 1])));
        }
    }

    if !underway.is_empty() {
        out.push(String::new());
        out.push("🔧 Under way".into());
        for v in underway.iter().take(6) {
            out.push(format!("• {} (ends {})", item(v), short_day(v.job.end)));
        }
        if underway.len() > 6 {
            out.push(format!("• +{} more", underway.len() - 6));
        }
    }

    let recent: Vec<String> = b
        .recent_activity
        .iter()
        .filter(|c| {
            chrono::DateTime::parse_from_rfc3339(&c.created_at)
                .map(|t| (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_hours() < 24)
                .unwrap_or(false)
        })
        .map(|c| {
            let what = match (c.kind.as_str(), &c.status, &c.note) {
                ("new", _, _) => format!("new job: {}", c.task.as_deref().map(short).unwrap_or_default()),
                (_, Some(s), Some(n)) => format!("{s}, {n}"),
                (_, Some(s), None) => s.clone(),
                (_, None, Some(n)) => n.clone(),
                _ => "dates or owner changed".into(),
            };
            match c.job_id {
                Some(id) => format!("• #{id}: {what}"),
                None => format!("• {what}"),
            }
        })
        .collect();
    if !recent.is_empty() {
        out.push(String::new());
        out.push("📝 Last 24 hours".into());
        out.extend(recent);
    }
    if !b.pending_changes.is_empty() {
        let n = b.pending_changes.len();
        out.push(if n == 1 {
            "1 change reaches the tracker at the next sync.".to_string()
        } else {
            format!("{n} changes reach the tracker at the next sync.")
        });
    }

    if let Some(g) = &b.next_gate {
        out.push(String::new());
        out.push(format!("🏁 Next gate: {} · {} · {} days", g.job.task, short_day(g.job.end), g.days_to_end));
    }

    let focus = b
        .overdue
        .first()
        .map(|v| format!("{} is overdue. Done, or move the date?", item(v)))
        .or_else(|| b.start_now.first().map(|v| format!("{} was due to start {}. Started yet?", item(v), short_day(v.job.start))))
        .or_else(|| {
            b.starting_soon
                .first()
                .filter(|v| v.days_to_start <= 7)
                .map(|v| format!("{} starts {}. Ready to go?", item(v), day(v.job.start)))
        });
    if let Some(q) = focus {
        out.push(String::new());
        out.push(format!("❓ {q}"));
    }

    out.push(String::new());
    out.push(format!("📋 Tracker: {tracker_url}"));
    out.join("\n")
}
