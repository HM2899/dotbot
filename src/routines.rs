//! Routines: a prompt that goes to a Bot (or into a group) on a schedule, or when its
//! webhook is called. `<home>/routines.json`. Times are local time.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use chrono::{Datelike, Duration, Local, NaiveTime, TimeZone};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum When {
    /// Once, at a time (milliseconds since 1970).
    At { at: i64 },
    Daily { time: String },
    Weekdays { time: String },
    /// `day`: 0 is Sunday. `days`, when set, lists several weekdays and replaces `day`.
    Weekly {
        #[serde(default)]
        day: u32,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        days: Vec<u32>,
        time: String,
    },
    /// `day` 1-31; a month without that day runs on its last day.
    Monthly { day: u32, time: String },
    /// `month` 1-12, `day` 1-31; February 29 runs on the 28th in other years.
    Yearly { month: u32, day: u32, time: String },
    Every { minutes: u32 },
    /// Only when its webhook is called.
    Webhook,
    /// Provider event delivered to the webhook, verified before dispatch.
    Event {
        source: String,
        #[serde(default)]
        event: String,
        #[serde(default)]
        filters: BTreeMap<String, String>,
        /// A key name, never a secret value.
        #[serde(default, rename = "signingSecret")]
        signing_secret: String,
    },
}

impl When {
    pub fn triggered(&self) -> bool { matches!(self, Self::Webhook | Self::Event { .. }) }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Routine {
    pub id: String,
    pub title: String,
    pub prompt: String,
    /// `bot:<id>` or `room:<id>`.
    pub target: String,
    pub when: When,
    /// IANA timezone for wall-clock schedules; None preserves server-local times.
    #[serde(default)]
    pub timezone: Option<String>,
    /// The secret part of the webhook address.
    pub key: String,
    #[serde(default)]
    pub next: Option<i64>,
    #[serde(default)]
    pub last: Option<i64>,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub created_by: Option<String>,
}

fn yes() -> bool {
    true
}

fn time_of(text: &str) -> Result<NaiveTime> {
    NaiveTime::parse_from_str(text.trim(), "%H:%M").or_else(|_| bail!("A time is HH:MM, such as 07:30."))
}

/// The first time after `now` that `when` fires, or None when it never will again.
pub fn next_after(when: &When, now: chrono::DateTime<Local>) -> Result<Option<i64>> {
    next_in(when, now)
}

pub fn next_after_in(when: &When, now: chrono::DateTime<Local>, timezone: Option<&str>) -> Result<Option<i64>> {
    match timezone.filter(|z| !z.trim().is_empty()) {
        Some(zone) => {
            let zone: chrono_tz::Tz = zone.parse().map_err(|_| anyhow::anyhow!("Use an IANA timezone, such as Asia/Shanghai or America/New_York."))?;
            next_in(when, now.with_timezone(&zone))
        }
        None => next_after(when, now),
    }
}

fn next_in<T: TimeZone>(when: &When, now: chrono::DateTime<T>) -> Result<Option<i64>> {
    if let When::Weekly { day, days, .. } = when && (*day > 6 || days.iter().any(|d| *d > 6)) { bail!("Weekly day is 0 (Sunday) through 6 (Saturday)."); }
    if let When::Monthly { day, .. } | When::Yearly { day, .. } = when && !(1..=31).contains(day) { bail!("Day of the month is 1 through 31."); }
    if let When::Yearly { month, .. } = when && !(1..=12).contains(month) { bail!("Month is 1 through 12."); }
    let zone = now.timezone();
    let at_day = |day: chrono::NaiveDate, time: NaiveTime| {
        // On a repeated hour, choose the first occurrence, once per local day.
        // A skipped hour is skipped for that day, not shifted silently.
        match zone.from_local_datetime(&day.and_time(time)) {
            chrono::LocalResult::Single(t) => Some(t.timestamp_millis()),
            chrono::LocalResult::Ambiguous(a, b) => Some(a.timestamp_millis().min(b.timestamp_millis())),
            chrono::LocalResult::None => None,
        }
    };
    Ok(match when {
        When::At { at } => (*at > now.timestamp_millis()).then_some(*at),
        When::Every { minutes } => {
            if *minutes < 5 {
                bail!("A routine runs at most every 5 minutes.");
            }
            Some((now + Duration::minutes(*minutes as i64)).timestamp_millis())
        }
        When::Webhook => None,
        When::Event { source, signing_secret, filters, .. } => {
            if !["github", "slack", "teams"].contains(&source.as_str()) { bail!("Event source is github, slack or teams."); }
            if source != "teams" && signing_secret.trim().is_empty() { bail!("GitHub and Slack event routines need a signingSecret key name."); }
            if !signing_secret.is_empty() && crate::secrets::clean_name(signing_secret)? != *signing_secret { bail!("signingSecret must be a saved key name, not a value."); }
            if filters.keys().any(|k| !k.starts_with('/') || k.len() > 256) { bail!("Event filter keys are JSON pointers, such as /repository/full_name or /event/channel."); }
            None
        }
        When::Daily { time } | When::Weekdays { time } | When::Weekly { time, .. } => {
            let t = time_of(time)?;
            let mut day = now.date_naive();
            for _ in 0..8 {
                let fits = match when {
                    When::Weekdays { .. } => !day.weekday().num_days_from_sunday().is_multiple_of(6),
                    When::Weekly { day: d, days, .. } => if days.is_empty() { day.weekday().num_days_from_sunday() == *d } else { days.contains(&day.weekday().num_days_from_sunday()) },
                    _ => true,
                };
                if fits
                    && let Some(ms) = at_day(day, t)
                        && ms > now.timestamp_millis() {
                            return Ok(Some(ms));
                        }
                day = day.succ_opt().unwrap();
            }
            None
        }
        When::Monthly { day, time } | When::Yearly { day, time, .. } => {
            let t = time_of(time)?;
            let today = now.date_naive();
            // Months from this one on (yearly: the same month in the coming years).
            let (step, first) = match when { When::Yearly { month, .. } => (12, (today.year(), *month)), _ => (1, (today.year(), today.month())) };
            let (mut year, mut month) = first;
            for _ in 0..60 {
                let last = (28..=31).rev().find(|d| chrono::NaiveDate::from_ymd_opt(year, month, *d).is_some()).unwrap();
                let date = chrono::NaiveDate::from_ymd_opt(year, month, (*day).min(last)).unwrap();
                if let Some(ms) = at_day(date, t) && ms > now.timestamp_millis() { return Ok(Some(ms)); }
                month += step;
                while month > 12 { month -= 12; year += 1; }
            }
            None
        }
    })
}

/// A schedule as the user and Bots write it: {"kind": "daily", "time": "07:00"}, etc.
#[cfg(test)]
pub fn parse_when(value: &Value) -> Result<When> {
    parse_when_in(value, None)
}

pub fn parse_when_in(value: &Value, timezone: Option<&str>) -> Result<When> {
    let mut value = value.clone();
    if value["kind"] == "at" && let Some(raw) = value["at"].as_str() {
        let ms = if let Ok(at) = chrono::DateTime::parse_from_rfc3339(raw) { at.timestamp_millis() } else {
            let date = chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M").map_err(|_| anyhow::anyhow!("Use ISO date/time or milliseconds for when.at."))?;
            if let Some(zone) = timezone.filter(|z| !z.trim().is_empty()) {
                date.and_local_timezone(zone.parse::<chrono_tz::Tz>().map_err(|_| anyhow::anyhow!("Unknown timezone"))?).single().map(|d| d.timestamp_millis())
            } else { date.and_local_timezone(Local).single().map(|d| d.timestamp_millis()) }
            .ok_or_else(|| anyhow::anyhow!("That local time is ambiguous or skipped by daylight saving; include an explicit UTC offset."))?
        };
        value["at"] = serde_json::json!(ms);
    }
    let when: When = serde_json::from_value(value).map_err(|_| anyhow::anyhow!("when.kind is at, daily, weekdays, weekly, monthly, yearly, every, webhook or event; use HH:MM times and minutes >= 5."))?;
    next_after_in(&when, Local::now(), timezone)?;
    Ok(when)
}

pub fn describe(when: &When) -> String {
    match when {
        When::At { at } => format!("once, {}", Local.timestamp_millis_opt(*at).single().map(|t| t.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default()),
        When::Daily { time } => format!("every day at {time}"),
        When::Weekdays { time } => format!("weekdays at {time}"),
        When::Weekly { day, days, time } => {
            let names = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
            let list = if days.is_empty() { vec![*day] } else { days.clone() };
            format!("every {} at {time}", list.iter().map(|d| names[(*d % 7) as usize]).collect::<Vec<_>>().join(", "))
        }
        When::Monthly { day, time } => format!("monthly on day {day} at {time}"),
        When::Yearly { month, day, time } => format!("yearly on {month}/{day} at {time}"),
        When::Every { minutes } => format!("every {minutes} minutes"),
        When::Webhook => "when its webhook is called".into(),
        When::Event { source, event, .. } => format!("on {source} {}", if event.is_empty() { "events" } else { event }),
    }
}

pub struct Routines {
    path: PathBuf,
    pub list: tokio::sync::Mutex<Vec<Routine>>,
    history_path: PathBuf,
    history: tokio::sync::Mutex<Vec<RoutineRun>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutineRun {
    pub id: String,
    pub routine: String,
    pub target: String,
    pub trigger: String,
    pub event_id: Option<String>,
    pub started: i64,
    pub finished: Option<i64>,
    pub status: String,
    pub result: Option<String>,
}

impl Routines {
    pub fn open(home: &Path) -> Result<Self> {
        let path = home.join("routines.json");
        // Saves go through `durable`, which keeps these documents in SQLite once it
        // exists; reading the old JSON file would revive paused or edited routines.
        let read = |path: &Path| -> Result<Vec<Value>> {
            match crate::sqlite_storage::read(path) {
                Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
                Err(e) => Err(e.into()),
            }
        };
        let list = read(&path)?.into_iter().map(serde_json::from_value).collect::<Result<Vec<Routine>, _>>()?;
        let history_path = home.join("routine-runs.json");
        let mut history = read(&history_path)?.into_iter().map(serde_json::from_value).collect::<Result<Vec<RoutineRun>, _>>()?;
        for run in history.iter_mut().filter(|r| matches!(r.status.as_str(), "queued" | "running")) {
            run.status = "interrupted".into(); run.finished = Some(crate::store::now_ms()); run.result = Some("The server restarted before this run finished.".into());
        }
        Self::write(&history_path, &history)?;
        Ok(Routines { path, list: tokio::sync::Mutex::new(list), history_path, history: tokio::sync::Mutex::new(history) })
    }

    pub fn save(&self, list: &[Routine]) -> Result<()> {
        Self::write(&self.path, list)
    }

    fn write<T: Serialize + ?Sized>(path: &Path, value: &T) -> Result<()> {
        crate::durable::save(path,&value)
    }

    pub async fn history(&self, routine: Option<&str>) -> Vec<RoutineRun> {
        self.history.lock().await.iter().rev().filter(|r| routine.is_none_or(|id| r.routine == id)).take(100).cloned().collect()
    }

    /// Reserve an event once across provider retries and server restarts. Prevent
    /// manual/timer overlap; distinct incoming events each retain their own job.
    pub async fn begin(&self, routine: &Routine, trigger: &str, event_id: Option<String>) -> Result<Option<RoutineRun>> {
        let mut history = self.history.lock().await;
        if history.iter().any(|r| r.routine == routine.id && ((trigger != "event" && matches!(r.status.as_str(), "queued" | "running")) || event_id.as_ref().is_some_and(|id| r.event_id.as_ref() == Some(id)))) { return Ok(None); }
        let run = RoutineRun { id: uuid::Uuid::new_v4().simple().to_string(), routine: routine.id.clone(), target: routine.target.clone(), trigger: trigger.into(), event_id, started: crate::store::now_ms(), finished: None, status: "queued".into(), result: None };
        let mut next = history.clone();
        if next.len() >= 1000 && let Some(at) = next.iter().position(|r| !matches!(r.status.as_str(), "queued" | "running")) { next.remove(at); }
        next.push(run.clone());
        Self::write(&self.history_path, &next)?; *history = next;
        Ok(Some(run))
    }

    pub async fn start(&self, id: &str) -> Result<()> {
        let mut history = self.history.lock().await;
        let mut next = history.clone();
        let run = next.iter_mut().find(|r| r.id == id).ok_or_else(|| anyhow::anyhow!("Unknown routine run"))?;
        if run.status != "queued" { return Ok(()); }
        run.status = "running".into(); run.started = crate::store::now_ms();
        Self::write(&self.history_path, &next)?; *history = next;
        Ok(())
    }

    pub async fn end(&self, id: &str, result: &str, failed: bool) -> Result<()> {
        let mut history = self.history.lock().await;
        let mut next = history.clone();
        let run = next.iter_mut().find(|r| r.id == id).ok_or_else(|| anyhow::anyhow!("Unknown routine run"))?;
        run.status = if failed { "failed" } else { "completed" }.into();
        run.finished = Some(crate::store::now_ms());
        run.result = Some(crate::team::clip(result, 8000));
        Self::write(&self.history_path, &next)?; *history = next;
        Ok(())
    }

    /// The routines due now, each moved on to its next time.
    pub async fn due(&self, now: chrono::DateTime<Local>) -> Result<Vec<Routine>> {
        let mut list = self.list.lock().await;
        let mut next = list.clone();
        let mut out = Vec::new();
        for routine in next.iter_mut().filter(|r| r.enabled) {
            if routine.next.is_some_and(|n| n <= now.timestamp_millis()) {
                routine.next = next_after_in(&routine.when, now, routine.timezone.as_deref())?;
                if routine.next.is_none() && matches!(routine.when, When::At { .. }) {
                    routine.enabled = false;
                }
                out.push(routine.clone());
            }
        }
        if !out.is_empty() {
            self.save(&next)?; *list = next;
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedules_find_their_next_time() {
        // A Wednesday, 10:00 local.
        let now = Local.with_ymd_and_hms(2026, 10, 21, 10, 0, 0).unwrap();
        let next = |w: When| next_after(&w, now).unwrap().map(|ms| Local.timestamp_millis_opt(ms).unwrap().format("%a %H:%M").to_string());
        assert_eq!(next(When::Daily { time: "07:00".into() }).as_deref(), Some("Thu 07:00"));
        assert_eq!(next(When::Daily { time: "11:30".into() }).as_deref(), Some("Wed 11:30"));
        assert_eq!(next(When::Weekly { day: 1, days: vec![], time: "09:00".into() }).as_deref(), Some("Mon 09:00"));
        assert_eq!(next(When::Every { minutes: 30 }).as_deref(), Some("Wed 10:30"));
        assert_eq!(next(When::Weekly { day: 0, days: vec![1, 3, 5], time: "09:00".into() }).as_deref(), Some("Fri 09:00"));
        let date = |w: When| next_after(&w, now).unwrap().map(|ms| Local.timestamp_millis_opt(ms).unwrap().format("%Y-%m-%d %H:%M").to_string());
        assert_eq!(date(When::Monthly { day: 21, time: "11:00".into() }).as_deref(), Some("2026-10-21 11:00"));
        assert_eq!(date(When::Monthly { day: 21, time: "09:00".into() }).as_deref(), Some("2026-11-21 09:00"));
        assert_eq!(date(When::Monthly { day: 31, time: "09:00".into() }).as_deref(), Some("2026-10-31 09:00"));
        let november = Local.with_ymd_and_hms(2026, 11, 1, 10, 0, 0).unwrap();
        assert_eq!(next_after(&When::Monthly { day: 31, time: "09:00".into() }, november).unwrap().map(|ms| Local.timestamp_millis_opt(ms).unwrap().format("%m-%d").to_string()).as_deref(), Some("11-30"));
        assert_eq!(date(When::Yearly { month: 2, day: 29, time: "08:00".into() }).as_deref(), Some("2027-02-28 08:00"));
        assert_eq!(date(When::Yearly { month: 12, day: 25, time: "08:00".into() }).as_deref(), Some("2026-12-25 08:00"));
        assert!(parse_when(&serde_json::json!({"kind": "monthly", "day": 32, "time": "09:00"})).is_err());
        assert!(parse_when(&serde_json::json!({"kind": "weekly", "days": [1, 7], "time": "09:00"})).is_err());
        assert!(parse_when(&serde_json::json!({"kind": "weekly", "day": 2, "time": "09:00"})).is_ok());
        assert_eq!(next(When::Webhook), None);
        let friday = Local.with_ymd_and_hms(2026, 10, 23, 18, 0, 0).unwrap();
        assert_eq!(next_after(&When::Weekdays { time: "07:00".into() }, friday).unwrap().map(|ms| Local.timestamp_millis_opt(ms).unwrap().format("%a").to_string()).as_deref(), Some("Mon"));
        assert!(next_after(&When::Every { minutes: 1 }, now).is_err());
        assert!(parse_when(&serde_json::json!({"kind": "daily", "time": "7am"})).is_err());
        assert_eq!(describe(&When::Weekdays { time: "07:00".into() }), "weekdays at 07:00");
    }

    #[test]
    fn timezone_schedules_handle_dst_gaps_and_repeated_hours_once() {
        let date = |s: &str| chrono::DateTime::parse_from_rfc3339(s).unwrap();
        let next = |now: &str, time: &str, zone: &str| next_after_in(&When::Daily { time: time.into() }, date(now).with_timezone(&Local), Some(zone)).unwrap().unwrap();
        assert_eq!(next("2026-10-21T06:00:00+08:00", "07:00", "Asia/Shanghai"), date("2026-10-21T07:00:00+08:00").timestamp_millis());
        assert_eq!(next("2026-03-08T01:59:00-05:00", "02:30", "America/New_York"), date("2026-03-09T02:30:00-04:00").timestamp_millis());
        assert_eq!(next("2026-11-01T01:45:00-04:00", "01:30", "America/New_York"), date("2026-11-02T01:30:00-05:00").timestamp_millis());
        assert!(parse_when_in(&serde_json::json!({"kind":"at","at":"2026-11-01T01:30"}), Some("America/New_York")).is_err());
        assert!(parse_when(&serde_json::json!({"kind":"weekly","day":7,"time":"07:00"})).is_err());
        assert!(parse_when_in(&serde_json::json!({"kind":"daily","time":"07:00"}), Some("invalid/zone")).is_err());
        let at = parse_when_in(&serde_json::json!({"kind":"at","at":"2099-10-21T07:00"}), Some("Asia/Shanghai")).unwrap();
        assert_eq!(at, When::At { at: date("2099-10-21T07:00:00+08:00").timestamp_millis() });
    }

    #[tokio::test]
    async fn run_receipts_deduplicate_events_and_make_restarts_explicit() {
        let home = tempfile::tempdir().unwrap(); let routines = Routines::open(home.path()).unwrap();
        let routine = Routine { id: "r1".into(), title: "test".into(), prompt: "check".into(), target: "bot:b1".into(), when: When::Webhook, timezone: None, key: "key".into(), next: None, last: None, enabled: true, created_by: None };
        let first = routines.begin(&routine, "event", Some("one".into())).await.unwrap().unwrap();
        assert!(routines.begin(&routine, "event", Some("one".into())).await.unwrap().is_none());
        // Distinct events queue independently, even while the first is running.
        assert!(routines.begin(&routine, "event", Some("two".into())).await.unwrap().is_some());
        routines.end(&first.id, "done", false).await.unwrap();
        let reopened = Routines::open(home.path()).unwrap();
        assert!(reopened.begin(&routine, "event", Some("one".into())).await.unwrap().is_none());
        let history = reopened.history(Some("r1")).await;
        assert_eq!(history[0].status, "interrupted"); assert_eq!(history[1].status, "completed");
        assert_eq!(history[1].result.as_deref(), Some("done"));
        assert!(reopened.begin(&routine, "manual", None).await.unwrap().is_some());
        assert!(reopened.begin(&routine, "manual", None).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn paused_routines_stay_paused_after_restart_with_sqlite() {
        let home = tempfile::tempdir().unwrap();
        let mut routine = Routine { id: "r1".into(), title: "test".into(), prompt: "check".into(), target: "bot:b1".into(), when: When::Daily { time: "07:00".into() }, timezone: None, key: "key".into(), next: None, last: None, enabled: true, created_by: None };
        // A JSON file left from before the SQLite migration must not win over later saves.
        std::fs::write(home.path().join("routines.json"), serde_json::to_vec(&[&routine]).unwrap()).unwrap();
        crate::sqlite_storage::initialize(home.path()).unwrap();
        let routines = Routines::open(home.path()).unwrap();
        routine.enabled = false;
        routines.save(std::slice::from_ref(&routine)).unwrap();
        let run = routines.begin(&routine, "manual", None).await.unwrap().unwrap();
        routines.end(&run.id, "done", false).await.unwrap();
        let reopened = Routines::open(home.path()).unwrap();
        assert!(!reopened.list.lock().await[0].enabled);
        assert_eq!(reopened.history(Some("r1")).await[0].status, "completed");
    }
}
