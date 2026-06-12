//! Always-on operator-activity recorder (#837, epic #829).
//!
//! Consumes the daemon's single WinEvent hook stream — teed from
//! [`super::a11y_events::A11yEventBridge`], because the process-wide
//! `WIN_EVENT_SENDER` permits exactly one hook subscription — and persists
//! `CF_TIMELINE` rows: foreground app switches, foreground window title
//! changes, idle/active transitions (`GetLastInputInfo` polled at a coarse
//! interval), and recorder session boundaries.
//!
//! Design constraints carried from ADR 2026-06-11-timeline-data-model and
//! field-tested foreground-tracking practice:
//!
//! - WinEvents are *triggers*, not truth. `EVENT_SYSTEM_FOREGROUND` is
//!   delivered asynchronously and frequently names an invisible Alt-Tab
//!   staging window (`ForegroundStaging`), a window not yet shown, or one
//!   already destroyed. When the event hwnd is unusable the recorder
//!   re-reads `GetForegroundWindow` — the source of truth — so a real app
//!   switch hiding behind a transient event still lands in the timeline.
//! - Every idle poll tick also reconciles recorded foreground state against
//!   the real foreground (rows tagged `source: "poll"`), so a missed
//!   WinEvent can never desync the timeline for longer than one interval.
//! - `EVENT_OBJECT_NAMECHANGE` fires for child objects too; a title row is
//!   written only when the *foreground* window's title actually changed.
//! - Idle detection mirrors ActivityWatch's defaults (180 s timeout, coarse
//!   poll); the `idle_start` row is backdated to the last-input instant so
//!   the timeline reflects when input actually stopped.
//!
//! Attribution: rows produced while an agent session holds the real-input
//! lease are tagged `agent { session_id }` (the lease is the canonical "an
//! agent owns the foreground" signal, epic #719); everything else is `human`.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde_json::json;
use synapse_a11y::{AccessibleEvent, AccessibleEventKind};
use synapse_core::types::{TIMELINE_RECORD_VERSION, TimelineActor, TimelineKind, TimelineRecord};
use synapse_storage::{Db, cf, timeline::timeline_key};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

use super::timeline_control::{RecorderControl, SuppressReason};

/// Idle threshold override, in milliseconds. Default mirrors ActivityWatch.
pub const IDLE_TIMEOUT_ENV: &str = "SYNAPSE_TIMELINE_IDLE_TIMEOUT_MS";
const DEFAULT_IDLE_TIMEOUT_MS: u64 = 180_000;
const MIN_IDLE_POLL_INTERVAL_MS: u64 = 250;
const MAX_IDLE_POLL_INTERVAL_MS: u64 = 5_000;
const SHUTDOWN_ACK_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecorderConfig {
    pub idle_timeout_ms: u64,
    pub idle_poll_interval_ms: u64,
}

impl RecorderConfig {
    /// Reads `SYNAPSE_TIMELINE_IDLE_TIMEOUT_MS` and derives the poll cadence.
    ///
    /// # Errors
    ///
    /// Returns an error when the variable is set but is not a positive
    /// integer; the daemon must refuse to start rather than record with a
    /// silently-wrong idle policy.
    pub fn from_env() -> Result<Self> {
        Self::from_raw(std::env::var(IDLE_TIMEOUT_ENV).ok().as_deref())
    }

    fn from_raw(raw: Option<&str>) -> Result<Self> {
        let idle_timeout_ms = match raw {
            None => DEFAULT_IDLE_TIMEOUT_MS,
            Some(value) => value.trim().parse::<u64>().with_context(|| {
                format!(
                    "{IDLE_TIMEOUT_ENV} must be a positive integer of milliseconds, got {value:?}"
                )
            })?,
        };
        if idle_timeout_ms == 0 {
            bail!("{IDLE_TIMEOUT_ENV} must be at least 1 millisecond, got 0");
        }
        let idle_poll_interval_ms = (idle_timeout_ms / 4)
            .clamp(MIN_IDLE_POLL_INTERVAL_MS, MAX_IDLE_POLL_INTERVAL_MS)
            .min(idle_timeout_ms);
        Ok(Self {
            idle_timeout_ms,
            idle_poll_interval_ms,
        })
    }
}

enum RecorderMessage {
    Accessible(AccessibleEvent),
    IdleProbe { idle_ms: u64 },
    Shutdown { done: oneshot::Sender<()> },
}

/// Shared write path: every producer (worker, spawn, drop backstop) goes
/// through one row encoder so key allocation and failure accounting are
/// uniform — and one gate, so pause/exclusion (#843) can never be bypassed
/// by a feed that forgot to check.
#[derive(Clone)]
struct TimelineWriter {
    db: Arc<Db>,
    control: Arc<RecorderControl>,
    seq: Arc<AtomicU32>,
    rows_written: Arc<AtomicU64>,
    write_failures: Arc<AtomicU64>,
    rows_suppressed_paused: Arc<AtomicU64>,
    rows_suppressed_excluded: Arc<AtomicU64>,
}

impl TimelineWriter {
    fn try_write(
        &self,
        ts_ns: u64,
        kind: TimelineKind,
        actor: TimelineActor,
        app: Option<String>,
        payload: serde_json::Value,
    ) -> Result<()> {
        let record = TimelineRecord {
            record_version: TIMELINE_RECORD_VERSION,
            ts_ns,
            kind,
            actor,
            app,
            payload,
        };
        let value = serde_json::to_vec(&record)
            .with_context(|| format!("encode CF_TIMELINE {kind:?} record"))?;
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let key = timeline_key(ts_ns, seq);
        self.db
            .put_batch(cf::CF_TIMELINE, [(key, value)])
            .with_context(|| format!("write CF_TIMELINE {kind:?} row ts_ns={ts_ns} seq={seq}"))?;
        self.rows_written.fetch_add(1, Ordering::Relaxed);
        tracing::debug!(
            code = "TIMELINE_ROW_WRITTEN",
            kind = ?kind,
            ts_ns,
            seq,
            "timeline row written"
        );
        Ok(())
    }

    /// Forces pending batched writes to disk. The batcher acks `put_batch`
    /// on enqueue and flushes on a 100 ms cadence, so anything that must be
    /// durable *now* (session boundaries at shutdown) needs an explicit
    /// flush — a return value alone does not prove the row is on disk.
    fn flush_logged(&self) {
        if let Err(error) = self.db.flush() {
            tracing::error!(
                code = "TIMELINE_FLUSH_FAILED",
                detail = %error,
                "failed to flush batched timeline writes"
            );
        }
    }

    /// The pause/exclusion gate (#843). Checked by every steady-state write;
    /// suppression is counted and debug-logged, never silent.
    fn suppressed(&self, kind: TimelineKind, app: Option<&str>) -> bool {
        match self.control.suppress_reason(app) {
            None => false,
            Some(SuppressReason::Paused) => {
                self.rows_suppressed_paused.fetch_add(1, Ordering::Relaxed);
                tracing::debug!(
                    code = "TIMELINE_ROW_SUPPRESSED_PAUSED",
                    kind = ?kind,
                    "timeline row suppressed: recorder is paused"
                );
                true
            }
            Some(SuppressReason::ExcludedApp) => {
                self.rows_suppressed_excluded
                    .fetch_add(1, Ordering::Relaxed);
                tracing::debug!(
                    code = "TIMELINE_ROW_SUPPRESSED_EXCLUDED",
                    kind = ?kind,
                    app = app.unwrap_or_default(),
                    "timeline row suppressed: process is excluded"
                );
                true
            }
        }
    }

    /// Write path for the steady-state worker: a failed row is a loud
    /// structured error plus a failure count (surfaced by `timeline_stats`,
    /// #842), never a panic that kills the recorder.
    fn write_logged(
        &self,
        ts_ns: u64,
        kind: TimelineKind,
        actor: TimelineActor,
        app: Option<String>,
        payload: serde_json::Value,
    ) {
        if self.suppressed(kind, app.as_deref()) {
            return;
        }
        if let Err(error) = self.try_write(ts_ns, kind, actor, app, payload) {
            self.write_failures.fetch_add(1, Ordering::Relaxed);
            tracing::error!(
                code = "TIMELINE_WRITE_FAILED",
                kind = ?kind,
                ts_ns,
                detail = %format!("{error:#}"),
                "failed to persist timeline row"
            );
        }
    }
}

/// Last recorded foreground window; the dedup baseline for focus/title rows.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ForegroundSnapshot {
    hwnd: i64,
    pid: u32,
    process_name: String,
    process_path: String,
    title: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ForegroundTransition {
    Duplicate,
    TitleChanged,
    Switched,
}

fn classify_foreground_transition(
    prev: Option<&ForegroundSnapshot>,
    next: &ForegroundSnapshot,
) -> ForegroundTransition {
    match prev {
        Some(prev) if prev.hwnd == next.hwnd && prev.pid == next.pid => {
            // Same window: only the title can have moved.
            if prev.title == next.title {
                ForegroundTransition::Duplicate
            } else {
                ForegroundTransition::TitleChanged
            }
        }
        _ => ForegroundTransition::Switched,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IdleEdge {
    Start,
    End,
}

const fn idle_transition(currently_idle: bool, idle_ms: u64, timeout_ms: u64) -> Option<IdleEdge> {
    if !currently_idle && idle_ms >= timeout_ms {
        Some(IdleEdge::Start)
    } else if currently_idle && idle_ms < timeout_ms {
        Some(IdleEdge::End)
    } else {
        None
    }
}

fn now_ts_ns() -> u64 {
    let nanos = Utc::now().timestamp_nanos_opt().unwrap_or(i64::MAX);
    u64::try_from(nanos).unwrap_or(0)
}

/// Resolves who is driving the session right now. An agent session holding
/// the real-input lease owns foreground changes; the operator-preempt
/// sentinel and an unheld lease both mean the human.
fn current_actor() -> TimelineActor {
    let status = synapse_action::lease::status();
    match status.owner_session_id {
        Some(owner) if status.held && owner != synapse_action::OPERATOR_LEASE_OWNER_SESSION_ID => {
            TimelineActor::Agent { session_id: owner }
        }
        _ => TimelineActor::Human,
    }
}

struct WorkerState {
    writer: TimelineWriter,
    config: RecorderConfig,
    foreground: Option<ForegroundSnapshot>,
    idle: bool,
}

impl WorkerState {
    fn handle_accessible(&mut self, event: &AccessibleEvent) {
        // Paused means *perceive nothing*: skip even the foreground/title
        // readbacks, not just the row writes. The snapshot is dropped so the
        // first post-resume trigger re-records reality from scratch.
        if self.writer.control.is_paused() {
            self.foreground = None;
            self.writer
                .rows_suppressed_paused
                .fetch_add(1, Ordering::Relaxed);
            return;
        }
        match event.kind {
            AccessibleEventKind::ForegroundChanged => self.handle_foreground(event.window_id),
            AccessibleEventKind::NameChanged => self.handle_name_change(event.window_id),
            _ => {}
        }
    }

    /// A `ForegroundChanged` WinEvent is a *trigger*, not the truth: it is
    /// delivered asynchronously, and its hwnd can be an Alt-Tab transient
    /// (`ForegroundStaging`), a window that has not been shown yet, or one
    /// that is already destroyed. When the event hwnd is not a usable visible
    /// window, the recorder re-reads the actual foreground window instead of
    /// dropping the trigger — otherwise a real app switch hiding behind a
    /// transient event would silently vanish from the timeline.
    fn handle_foreground(&mut self, window_id: i64) {
        let context = match self.resolve_foreground_trigger(window_id) {
            Some(context) => context,
            None => return,
        };
        self.apply_foreground(&context, "win_event");
    }

    fn resolve_foreground_trigger(
        &self,
        window_id: i64,
    ) -> Option<synapse_core::ForegroundContext> {
        match synapse_a11y::is_window_visible(window_id) {
            Ok(true) => match synapse_a11y::foreground_context(window_id) {
                Ok(context) => return Some(context),
                Err(error) => {
                    tracing::debug!(
                        code = "TIMELINE_FOREGROUND_EVENT_HWND_STALE",
                        hwnd = window_id,
                        detail = %error,
                        "event window vanished mid-resolve; re-reading the real foreground"
                    );
                }
            },
            Ok(false) => {
                tracing::debug!(
                    code = "TIMELINE_FOREGROUND_EVENT_HWND_INVISIBLE",
                    hwnd = window_id,
                    "event window is invisible (transient); re-reading the real foreground"
                );
            }
            Err(error) => {
                tracing::debug!(
                    code = "TIMELINE_FOREGROUND_EVENT_HWND_STALE",
                    hwnd = window_id,
                    detail = %error,
                    "event window vanished before visibility readback; re-reading the real foreground"
                );
            }
        }
        // Source of truth: whatever is actually foreground right now.
        match synapse_a11y::current_foreground_context() {
            Ok(context) => {
                if matches!(synapse_a11y::is_window_visible(context.hwnd), Ok(true)) {
                    Some(context)
                } else {
                    tracing::debug!(
                        code = "TIMELINE_FOREGROUND_UNSETTLED",
                        event_hwnd = window_id,
                        current_hwnd = context.hwnd,
                        "current foreground is itself transient; next trigger or poll will settle it"
                    );
                    None
                }
            }
            Err(error) => {
                tracing::debug!(
                    code = "TIMELINE_FOREGROUND_NONE",
                    event_hwnd = window_id,
                    detail = %error,
                    "no resolvable foreground window for this trigger"
                );
                None
            }
        }
    }

    /// Records the resolved foreground state, deduplicating against the last
    /// recorded snapshot. `source` records which trigger produced the row.
    fn apply_foreground(&mut self, context: &synapse_core::ForegroundContext, source: &str) {
        let next = ForegroundSnapshot {
            hwnd: context.hwnd,
            pid: context.pid,
            process_name: context.process_name.clone(),
            process_path: context.process_path.clone(),
            title: context.window_title.clone(),
        };
        // Excluded processes leave the dedup snapshot untouched: the moment
        // the exclusion lifts (or focus moves to a recordable app), the next
        // trigger classifies as a switch and records reality instead of
        // deduplicating against a window that was never written.
        if self
            .writer
            .suppressed(TimelineKind::FocusChange, Some(&next.process_name))
        {
            return;
        }
        match classify_foreground_transition(self.foreground.as_ref(), &next) {
            ForegroundTransition::Duplicate => {}
            ForegroundTransition::TitleChanged => self.write_title_change(&next),
            ForegroundTransition::Switched => {
                self.writer.write_logged(
                    now_ts_ns(),
                    TimelineKind::FocusChange,
                    current_actor(),
                    Some(next.process_name.clone()),
                    json!({
                        "title": next.title,
                        "process_path": next.process_path,
                        "pid": next.pid,
                        "hwnd": next.hwnd,
                        "source": source,
                    }),
                );
            }
        }
        self.foreground = Some(next);
    }

    fn handle_name_change(&mut self, window_id: i64) {
        let Some(previous) = self.foreground.as_ref() else {
            return;
        };
        if previous.hwnd != window_id {
            return;
        }
        // NAMECHANGE also fires for child objects of the same HWND; re-read
        // the top-level title and only record a real change.
        let context = match synapse_a11y::foreground_context(window_id) {
            Ok(context) => context,
            Err(error) => {
                tracing::debug!(
                    code = "TIMELINE_TITLE_CONTEXT_UNRESOLVED",
                    hwnd = window_id,
                    detail = %error,
                    "foreground window vanished before title readback"
                );
                return;
            }
        };
        if context.window_title == previous.title {
            return;
        }
        let next = ForegroundSnapshot {
            hwnd: context.hwnd,
            pid: context.pid,
            process_name: context.process_name,
            process_path: context.process_path,
            title: context.window_title,
        };
        self.write_title_change(&next);
        self.foreground = Some(next);
    }

    fn write_title_change(&self, next: &ForegroundSnapshot) {
        let previous_title = self
            .foreground
            .as_ref()
            .map(|snapshot| snapshot.title.clone());
        self.writer.write_logged(
            now_ts_ns(),
            TimelineKind::TitleChange,
            current_actor(),
            Some(next.process_name.clone()),
            json!({
                "title": next.title,
                "previous_title": previous_title,
                "pid": next.pid,
                "hwnd": next.hwnd,
            }),
        );
    }

    fn handle_idle_probe(&mut self, idle_ms: u64) {
        if self.writer.control.is_paused() {
            self.foreground = None;
            // The idle tick doubles as the auto-resume clock: a pause armed
            // with `duration_ms` reopens the gate within one poll interval.
            if self.writer.control.auto_resume_due(now_ts_ns()) {
                match resume_recording(&self.writer, "auto_resume") {
                    Ok(_state) => {
                        tracing::info!(
                            code = "TIMELINE_RECORDER_AUTO_RESUMED",
                            "timeline recorder auto-resumed: pause deadline passed"
                        );
                    }
                    Err(error) => {
                        tracing::error!(
                            code = "TIMELINE_RECORDER_AUTO_RESUME_FAILED",
                            detail = %format!("{error:#}"),
                            "timeline auto-resume failed; retrying next idle tick"
                        );
                        return;
                    }
                }
            } else {
                return;
            }
        }
        self.reconcile_foreground();
        let Some(edge) = idle_transition(self.idle, idle_ms, self.config.idle_timeout_ms) else {
            return;
        };
        // Backdate to the last-input instant: the timeline records when input
        // actually stopped/resumed, not when the coarse poll noticed.
        let ts_ns = now_ts_ns().saturating_sub(idle_ms.saturating_mul(1_000_000));
        match edge {
            IdleEdge::Start => {
                self.idle = true;
                self.writer.write_logged(
                    ts_ns,
                    TimelineKind::IdleStart,
                    TimelineActor::Human,
                    None,
                    json!({
                        "idle_ms_at_detection": idle_ms,
                        "idle_timeout_ms": self.config.idle_timeout_ms,
                    }),
                );
            }
            IdleEdge::End => {
                self.idle = false;
                self.writer.write_logged(
                    ts_ns,
                    TimelineKind::IdleEnd,
                    TimelineActor::Human,
                    None,
                    json!({ "idle_ms_at_detection": idle_ms }),
                );
            }
        }
    }

    /// Poll-driven safety net: if a foreground change was missed (hook
    /// hiccup, transient-only event stream), the next idle tick re-syncs the
    /// recorded state to reality, so the timeline can never silently diverge
    /// for longer than one poll interval.
    fn reconcile_foreground(&mut self) {
        let context = match synapse_a11y::current_foreground_context() {
            Ok(context) => context,
            Err(error) => {
                tracing::debug!(
                    code = "TIMELINE_FOREGROUND_NONE",
                    detail = %error,
                    "no foreground window at reconcile tick"
                );
                return;
            }
        };
        if !matches!(synapse_a11y::is_window_visible(context.hwnd), Ok(true)) {
            return;
        }
        self.apply_foreground(&context, "poll");
    }

    fn write_session_end(&self, edge: &str) {
        self.writer.write_logged(
            now_ts_ns(),
            TimelineKind::SessionEnd,
            TimelineActor::Human,
            None,
            session_end_payload(&self.writer, edge),
        );
    }
}

fn session_end_payload(writer: &TimelineWriter, edge: &str) -> serde_json::Value {
    json!({
        "pid": std::process::id(),
        "rows_written": writer.rows_written.load(Ordering::Relaxed),
        "write_failures": writer.write_failures.load(Ordering::Relaxed),
        "edge": edge,
    })
}

/// Outcome of a pause/resume control action, for tool readback (#843).
#[derive(Clone, Debug)]
pub struct RecorderControlOutcome {
    pub was_paused: bool,
    /// Whether a session boundary row was written (and flushed) for this
    /// transition. Re-pausing while paused / re-resuming while recording
    /// writes no row.
    pub boundary_row_written: bool,
    pub state: super::timeline_control::PersistedControlState,
}

/// Pause sequencing: boundary row while still recording, durable control row,
/// then the gate flips. A failure at any step propagates with the system left
/// in the last consistent state it reached.
fn pause_recording(
    writer: &TimelineWriter,
    paused_until_ns: Option<u64>,
    changed_by: &str,
) -> Result<RecorderControlOutcome> {
    let was_paused = writer.control.is_paused();
    let mut boundary_row_written = false;
    if !was_paused {
        writer
            .try_write(
                now_ts_ns(),
                TimelineKind::SessionEnd,
                TimelineActor::Human,
                None,
                json!({
                    "edge": "pause",
                    "by_session": changed_by,
                    "paused_until_ns": paused_until_ns,
                    "pid": std::process::id(),
                    "rows_written": writer.rows_written.load(Ordering::Relaxed),
                    "write_failures": writer.write_failures.load(Ordering::Relaxed),
                }),
            )
            .context("write session_end pause boundary row; recording is unchanged")?;
        writer
            .db
            .flush()
            .context("flush session_end pause boundary row; recording is unchanged")?;
        boundary_row_written = true;
    }
    let state =
        writer
            .control
            .persist_pause(&writer.db, paused_until_ns, now_ts_ns(), changed_by)?;
    tracing::info!(
        code = "TIMELINE_RECORDER_PAUSED",
        paused_until_ns,
        by_session = changed_by,
        "timeline recorder paused"
    );
    Ok(RecorderControlOutcome {
        was_paused,
        boundary_row_written,
        state,
    })
}

/// Resume sequencing: durable control row, the gate opens, then a
/// `session_start { edge: "resume" }` boundary row is written and flushed —
/// the resume-time proof that the write path works. A boundary failure is a
/// hard error: recording IS resumed at that point and the caller must know
/// the write path is broken.
fn resume_recording(writer: &TimelineWriter, changed_by: &str) -> Result<RecorderControlOutcome> {
    let was_paused = writer.control.is_paused();
    let state = writer
        .control
        .persist_resume(&writer.db, now_ts_ns(), changed_by)?;
    let mut boundary_row_written = false;
    if was_paused {
        writer
            .try_write(
                now_ts_ns(),
                TimelineKind::SessionStart,
                TimelineActor::Human,
                None,
                json!({
                    "edge": "resume",
                    "by_session": changed_by,
                    "pid": std::process::id(),
                }),
            )
            .context(
                "write session_start resume boundary row — recording IS resumed but the \
                 timeline write path is broken",
            )?;
        writer.db.flush().context(
            "flush session_start resume boundary row — recording IS resumed but the \
                 timeline write path is broken",
        )?;
        boundary_row_written = true;
        tracing::info!(
            code = "TIMELINE_RECORDER_RESUMED",
            by_session = changed_by,
            "timeline recorder resumed"
        );
    }
    Ok(RecorderControlOutcome {
        was_paused,
        boundary_row_written,
        state,
    })
}

async fn run_worker(
    mut receiver: mpsc::UnboundedReceiver<RecorderMessage>,
    mut state: WorkerState,
) {
    while let Some(message) = receiver.recv().await {
        match message {
            RecorderMessage::Accessible(event) => state.handle_accessible(&event),
            RecorderMessage::IdleProbe { idle_ms } => state.handle_idle_probe(idle_ms),
            RecorderMessage::Shutdown { done } => {
                state.write_session_end("shutdown");
                state.writer.flush_logged();
                let _ = done.send(());
                tracing::info!(
                    code = "TIMELINE_RECORDER_STOPPED",
                    rows_written = state.writer.rows_written.load(Ordering::Relaxed),
                    write_failures = state.writer.write_failures.load(Ordering::Relaxed),
                    "activity recorder stopped"
                );
                return;
            }
        }
    }
    tracing::warn!(
        code = "TIMELINE_RECORDER_CHANNEL_CLOSED",
        "activity recorder channel closed without shutdown; session_end is written by the drop backstop"
    );
}

async fn run_idle_probe(sender: mpsc::UnboundedSender<RecorderMessage>, poll_interval_ms: u64) {
    let period = Duration::from_millis(poll_interval_ms.max(1));
    // First tick after one full period (not immediately): spawn already
    // probed the idle source, and the WinEvent path covers startup state.
    let mut interval = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        match synapse_a11y::millis_since_last_input() {
            Ok(idle_ms) => {
                if sender.send(RecorderMessage::IdleProbe { idle_ms }).is_err() {
                    return;
                }
            }
            Err(error) => {
                tracing::error!(
                    code = "TIMELINE_IDLE_PROBE_FAILED",
                    detail = %error,
                    "idle probe failed; idle/active transitions are not being recorded this tick"
                );
            }
        }
    }
}

/// Always-on operator-activity recorder. One per daemon; owns the timeline
/// write path for foreground/title/idle/session rows.
pub struct ActivityRecorder {
    sender: mpsc::UnboundedSender<RecorderMessage>,
    writer: TimelineWriter,
    config: RecorderConfig,
    shutdown_requested: AtomicBool,
    sink_closed_logged: AtomicBool,
    worker: Mutex<Option<JoinHandle<()>>>,
    idle_probe: Mutex<Option<JoinHandle<()>>>,
}

impl std::fmt::Debug for ActivityRecorder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ActivityRecorder")
            .field("config", &self.config)
            .field(
                "rows_written",
                &self.writer.rows_written.load(Ordering::Relaxed),
            )
            .field(
                "write_failures",
                &self.writer.write_failures.load(Ordering::Relaxed),
            )
            .finish_non_exhaustive()
    }
}

impl ActivityRecorder {
    /// Starts the recorder: probes the idle source once (fail-fast on a
    /// platform where idle tracking cannot work), writes the `session_start`
    /// row synchronously (fail-fast on a broken write path), then spawns the
    /// event worker and the idle-poll task.
    ///
    /// # Errors
    ///
    /// Returns an error when the idle probe or the `session_start` write
    /// fails; the daemon must refuse to start with a recorder that cannot
    /// record. A recorder hydrated into the paused state (#843) writes no
    /// `session_start` — paused means zero rows — unless its auto-resume
    /// deadline already passed while the daemon was down, in which case it
    /// resumes immediately.
    pub fn spawn(
        db: Arc<Db>,
        config: RecorderConfig,
        control: Arc<RecorderControl>,
    ) -> Result<Self> {
        let initial_idle_ms = synapse_a11y::millis_since_last_input()
            .context("probe GetLastInputInfo for the activity recorder idle source")?;
        if control.auto_resume_due(now_ts_ns()) {
            control
                .persist_resume(&db, now_ts_ns(), "startup_auto_resume")
                .context("auto-resume expired timeline pause at recorder startup")?;
            tracing::info!(
                code = "TIMELINE_RECORDER_AUTO_RESUMED",
                "timeline pause deadline passed while the daemon was down; resuming at startup"
            );
        }
        let writer = TimelineWriter {
            db,
            control,
            seq: Arc::new(AtomicU32::new(0)),
            rows_written: Arc::new(AtomicU64::new(0)),
            write_failures: Arc::new(AtomicU64::new(0)),
            rows_suppressed_paused: Arc::new(AtomicU64::new(0)),
            rows_suppressed_excluded: Arc::new(AtomicU64::new(0)),
        };
        if writer.control.is_paused() {
            tracing::info!(
                code = "TIMELINE_RECORDER_STARTED_PAUSED",
                paused_until_ns = writer.control.paused_until_ns(),
                "activity recorder started in the persisted paused state; no rows until resume"
            );
        } else {
            writer
                .try_write(
                    now_ts_ns(),
                    TimelineKind::SessionStart,
                    TimelineActor::Human,
                    None,
                    json!({
                        "edge": "startup",
                        "pid": std::process::id(),
                        "idle_timeout_ms": config.idle_timeout_ms,
                        "idle_poll_interval_ms": config.idle_poll_interval_ms,
                        "initial_idle_ms": initial_idle_ms,
                    }),
                )
                .context("write CF_TIMELINE session_start row at recorder startup")?;
            // The batcher acks on enqueue; flush so a broken write path fails
            // the daemon at startup instead of surfacing 100 ms later in a log.
            writer
                .db
                .flush()
                .context("flush CF_TIMELINE session_start row at recorder startup")?;
        }

        let (sender, receiver) = mpsc::unbounded_channel();
        let state = WorkerState {
            writer: writer.clone(),
            config,
            foreground: None,
            idle: false,
        };
        let worker = tokio::spawn(run_worker(receiver, state));
        let idle_probe = tokio::spawn(run_idle_probe(sender.clone(), config.idle_poll_interval_ms));
        tracing::info!(
            code = "TIMELINE_RECORDER_STARTED",
            idle_timeout_ms = config.idle_timeout_ms,
            idle_poll_interval_ms = config.idle_poll_interval_ms,
            initial_idle_ms,
            "activity recorder started"
        );
        Ok(Self {
            sender,
            writer,
            config,
            shutdown_requested: AtomicBool::new(false),
            sink_closed_logged: AtomicBool::new(false),
            worker: Mutex::new(Some(worker)),
            idle_probe: Mutex::new(Some(idle_probe)),
        })
    }

    /// Cheap, non-blocking sink for the WinEvent bridge. Irrelevant kinds are
    /// filtered before crossing the channel.
    pub fn record_accessible_event(&self, event: &AccessibleEvent) {
        if !matches!(
            event.kind,
            AccessibleEventKind::ForegroundChanged | AccessibleEventKind::NameChanged
        ) {
            return;
        }
        if self
            .sender
            .send(RecorderMessage::Accessible(event.clone()))
            .is_err()
            && !self.sink_closed_logged.swap(true, Ordering::Relaxed)
        {
            tracing::error!(
                code = "TIMELINE_RECORDER_DOWN",
                "activity recorder worker is gone; foreground timeline rows are no longer recorded"
            );
        }
    }

    /// Graceful stop: drains the worker, writes `session_end`, and stops the
    /// idle probe. Idempotent.
    pub async fn shutdown(&self) {
        if self.shutdown_requested.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Some(probe) = self.take_task(&self.idle_probe) {
            probe.abort();
        }
        let (done_tx, done_rx) = oneshot::channel();
        if self
            .sender
            .send(RecorderMessage::Shutdown { done: done_tx })
            .is_err()
        {
            tracing::error!(
                code = "TIMELINE_RECORDER_SHUTDOWN_WORKER_GONE",
                "activity recorder worker was already gone at shutdown; writing session_end directly"
            );
            self.write_session_end_direct("shutdown_worker_gone");
            return;
        }
        match tokio::time::timeout(SHUTDOWN_ACK_TIMEOUT, done_rx).await {
            Ok(Ok(())) => {
                if let Some(worker) = self.take_task(&self.worker) {
                    let _ = worker.await;
                }
            }
            _ => {
                tracing::error!(
                    code = "TIMELINE_RECORDER_SHUTDOWN_TIMEOUT",
                    timeout_ms =
                        u64::try_from(SHUTDOWN_ACK_TIMEOUT.as_millis()).unwrap_or(u64::MAX),
                    "activity recorder worker did not acknowledge shutdown; aborting it"
                );
                if let Some(worker) = self.take_task(&self.worker) {
                    worker.abort();
                }
                self.write_session_end_direct("shutdown_timeout");
            }
        }
    }

    /// Live counters for health/FSV readback.
    #[must_use]
    pub fn readback(&self) -> (u64, u64) {
        (
            self.writer.rows_written.load(Ordering::Relaxed),
            self.writer.write_failures.load(Ordering::Relaxed),
        )
    }

    /// Suppressed-row counters: `(paused, excluded)` (#843 FSV readback).
    #[must_use]
    pub fn suppressed_counters(&self) -> (u64, u64) {
        (
            self.writer.rows_suppressed_paused.load(Ordering::Relaxed),
            self.writer.rows_suppressed_excluded.load(Ordering::Relaxed),
        )
    }

    /// Pauses recording: boundary row, durable control state, gate closed.
    ///
    /// # Errors
    ///
    /// Returns an error when the boundary row or the durable control write
    /// fails; the error states exactly which step failed and what state the
    /// recorder was left in.
    pub fn pause(
        &self,
        paused_until_ns: Option<u64>,
        changed_by: &str,
    ) -> Result<RecorderControlOutcome> {
        pause_recording(&self.writer, paused_until_ns, changed_by)
    }

    /// Resumes recording: durable control state, gate open, boundary row.
    ///
    /// # Errors
    ///
    /// Returns an error when the durable control write fails (still paused)
    /// or when the boundary row fails (resumed, write path broken — the
    /// error says so explicitly).
    pub fn resume(&self, changed_by: &str) -> Result<RecorderControlOutcome> {
        resume_recording(&self.writer, changed_by)
    }

    fn take_task(&self, slot: &Mutex<Option<JoinHandle<()>>>) -> Option<JoinHandle<()>> {
        match slot.lock() {
            Ok(mut guard) => guard.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        }
    }

    fn write_session_end_direct(&self, edge: &str) {
        if let Err(error) = self.writer.try_write(
            now_ts_ns(),
            TimelineKind::SessionEnd,
            TimelineActor::Human,
            None,
            session_end_payload(&self.writer, edge),
        ) {
            self.writer.write_failures.fetch_add(1, Ordering::Relaxed);
            tracing::error!(
                code = "TIMELINE_WRITE_FAILED",
                kind = ?TimelineKind::SessionEnd,
                detail = %format!("{error:#}"),
                "failed to persist session_end row"
            );
        }
        self.writer.flush_logged();
    }
}

impl Drop for ActivityRecorder {
    fn drop(&mut self) {
        if let Some(probe) = self.take_task(&self.idle_probe) {
            probe.abort();
        }
        if let Some(worker) = self.take_task(&self.worker) {
            worker.abort();
        }
        // Backstop: an unwound daemon still closes the recorder session so
        // the timeline never shows a session_start without a matching end.
        if !self.shutdown_requested.swap(true, Ordering::SeqCst) {
            self.write_session_end_direct("drop");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(hwnd: i64, pid: u32, title: &str) -> ForegroundSnapshot {
        ForegroundSnapshot {
            hwnd,
            pid,
            process_name: "test.exe".to_owned(),
            process_path: r"C:\test.exe".to_owned(),
            title: title.to_owned(),
        }
    }

    #[test]
    fn config_defaults_match_activitywatch_prior_art() {
        let config = RecorderConfig::from_raw(None).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(config.idle_timeout_ms, 180_000);
        assert_eq!(config.idle_poll_interval_ms, 5_000);
    }

    #[test]
    fn config_short_timeout_derives_proportional_poll() {
        let config =
            RecorderConfig::from_raw(Some("2000")).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(config.idle_timeout_ms, 2_000);
        assert_eq!(config.idle_poll_interval_ms, 500);
    }

    #[test]
    fn config_rejects_zero_and_garbage() {
        assert!(
            RecorderConfig::from_raw(Some("0")).is_err(),
            "0 must be rejected"
        );
        assert!(
            RecorderConfig::from_raw(Some("fast")).is_err(),
            "non-numeric must be rejected"
        );
        assert!(
            RecorderConfig::from_raw(Some("")).is_err(),
            "empty string must be rejected"
        );
    }

    #[test]
    fn foreground_transitions_classify_switch_title_duplicate() {
        let first = snapshot(100, 7, "Inbox");
        assert_eq!(
            classify_foreground_transition(None, &first),
            ForegroundTransition::Switched,
            "first foreground must be a switch"
        );
        assert_eq!(
            classify_foreground_transition(Some(&first), &snapshot(100, 7, "Inbox")),
            ForegroundTransition::Duplicate,
            "identical foreground must not produce a row"
        );
        assert_eq!(
            classify_foreground_transition(Some(&first), &snapshot(100, 7, "Drafts")),
            ForegroundTransition::TitleChanged,
            "same window with new title is a title change"
        );
        assert_eq!(
            classify_foreground_transition(Some(&first), &snapshot(200, 7, "Inbox")),
            ForegroundTransition::Switched,
            "new hwnd is a switch even with identical title"
        );
        assert_eq!(
            classify_foreground_transition(Some(&first), &snapshot(100, 8, "Inbox")),
            ForegroundTransition::Switched,
            "hwnd reuse by a different pid is a switch"
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn recorder_writes_real_foreground_rows_into_cf_timeline() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_test_writer()
            .try_init();
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let db = Arc::new(
            Db::open(temp.path(), synapse_core::SCHEMA_VERSION)
                .unwrap_or_else(|error| panic!("open temp db: {error}")),
        );
        let config = RecorderConfig::from_raw(Some("600000"))
            .unwrap_or_else(|error| panic!("config: {error}"));
        let control = Arc::new(
            crate::m3::timeline_control::RecorderControl::hydrate(&db)
                .unwrap_or_else(|error| panic!("hydrate control: {error:#}")),
        );
        let recorder = ActivityRecorder::spawn(Arc::clone(&db), config, control)
            .unwrap_or_else(|error| panic!("spawn recorder: {error}"));
        let (after_start, _failures) = recorder.readback();
        assert_eq!(
            after_start, 1,
            "session_start must be written synchronously"
        );

        // Real foreground window: the event the WinEvent hook would deliver.
        let context = synapse_a11y::current_foreground_context()
            .unwrap_or_else(|error| panic!("real foreground context: {error}"));
        let event = AccessibleEvent {
            seq: 1,
            at_ms: 1,
            window_id: context.hwnd,
            element_id: None,
            kind: AccessibleEventKind::ForegroundChanged,
            name: None,
            value: None,
        };
        println!(
            "readback=cf_timeline edge=real_foreground before=rows:{} foreground:{}",
            recorder.readback().0,
            context.process_name
        );
        recorder.record_accessible_event(&event);
        wait_for_rows(&recorder, 2).await;

        // Edge: identical foreground event must not produce another row.
        recorder.record_accessible_event(&event);
        // Edge: a vanished/invalid event hwnd re-resolves to the real
        // foreground (already recorded), so it dedups to no row — and must
        // never crash.
        let vanished = AccessibleEvent {
            window_id: 0x000d_ead0,
            ..event.clone()
        };
        recorder.record_accessible_event(&vanished);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(
            recorder.readback().0,
            2,
            "duplicate and vanished-window events must not write rows"
        );

        // Agent attribution: while a session holds the real-input lease, a
        // foreground change must be tagged agent{session_id}. Uses the real
        // lease registry and a second real visible window.
        let other_window = synapse_a11y::visible_top_level_window_contexts()
            .unwrap_or_else(|error| panic!("enumerate windows: {error}"))
            .into_iter()
            .find(|candidate| candidate.hwnd != context.hwnd);
        if let Some(other) = other_window {
            let lease_session = format!("fsv-agent-{}", std::process::id());
            // The lease registry is process-global and other tests in this
            // binary exercise it; retry briefly instead of flaking on overlap.
            let acquire_deadline = std::time::Instant::now() + Duration::from_secs(5);
            loop {
                let outcome =
                    synapse_action::lease::try_acquire(&lease_session, Duration::from_secs(30));
                match outcome {
                    synapse_action::LeaseOutcome::Acquired(_)
                    | synapse_action::LeaseOutcome::Renewed(_) => break,
                    other => {
                        assert!(
                            std::time::Instant::now() < acquire_deadline,
                            "real-input lease must be acquirable for the attribution edge: {other:?}"
                        );
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }
            println!(
                "readback=cf_timeline edge=agent_attribution before=lease_held_by:{lease_session} window:{}",
                other.process_name
            );
            let agent_event = AccessibleEvent {
                window_id: other.hwnd,
                ..event.clone()
            };
            recorder.record_accessible_event(&agent_event);
            wait_for_rows(&recorder, 3).await;
            synapse_action::lease::release(&lease_session)
                .unwrap_or_else(|error| panic!("release lease: {error:?}"));
        } else {
            panic!("attribution edge needs a second visible window; none found");
        }

        recorder.shutdown().await;
        println!(
            "readback=cf_timeline edge=post_shutdown counters={:?}",
            recorder.readback()
        );
        let rows = db
            .scan_cf(cf::CF_TIMELINE)
            .unwrap_or_else(|error| panic!("scan CF_TIMELINE: {error}"));
        println!(
            "readback=cf_timeline edge=real_foreground after=rows:{}",
            rows.len()
        );
        assert_eq!(
            rows.len(),
            4,
            "session_start + human focus_change + agent focus_change + session_end"
        );
        let records: Vec<TimelineRecord> = rows
            .iter()
            .map(|(key, value)| {
                if let Err(error) = synapse_storage::timeline::decode_timeline_key(key) {
                    panic!("decode key: {error}");
                }
                serde_json::from_slice(value)
                    .unwrap_or_else(|error| panic!("decode record: {error}"))
            })
            .collect();
        assert_eq!(records[0].kind, TimelineKind::SessionStart);
        assert_eq!(records[1].kind, TimelineKind::FocusChange);
        assert_eq!(records[2].kind, TimelineKind::FocusChange);
        assert_eq!(records[3].kind, TimelineKind::SessionEnd);
        assert_eq!(
            records[1].app.as_deref(),
            Some(context.process_name.as_str()),
            "focus_change row must carry the real foreground process"
        );
        assert_eq!(
            records[1].actor,
            TimelineActor::Human,
            "unleased foreground change must be attributed to the human"
        );
        let expected_session = format!("fsv-agent-{}", std::process::id());
        assert_eq!(
            records[2].actor,
            TimelineActor::Agent {
                session_id: expected_session
            },
            "leased foreground change must be attributed to the acting agent session"
        );
        assert!(
            records
                .windows(2)
                .all(|pair| pair[0].ts_ns <= pair[1].ts_ns),
            "rows must iterate in chronological order"
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn pause_and_exclusion_gates_suppress_real_rows() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_test_writer()
            .try_init();
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let db = Arc::new(
            Db::open(temp.path(), synapse_core::SCHEMA_VERSION)
                .unwrap_or_else(|error| panic!("open temp db: {error}")),
        );
        let config = RecorderConfig::from_raw(Some("600000"))
            .unwrap_or_else(|error| panic!("config: {error}"));
        let control = Arc::new(
            RecorderControl::hydrate(&db).unwrap_or_else(|error| panic!("hydrate: {error:#}")),
        );
        let recorder = ActivityRecorder::spawn(Arc::clone(&db), config, Arc::clone(&control))
            .unwrap_or_else(|error| panic!("spawn recorder: {error}"));
        assert_eq!(recorder.readback().0, 1, "session_start");

        let context = synapse_a11y::current_foreground_context()
            .unwrap_or_else(|error| panic!("real foreground context: {error}"));
        let event = AccessibleEvent {
            seq: 1,
            at_ms: 1,
            window_id: context.hwnd,
            element_id: None,
            kind: AccessibleEventKind::ForegroundChanged,
            name: None,
            value: None,
        };

        // Pause: boundary row written while still recording, then silence.
        println!(
            "readback=cf_timeline edge=pause before=rows:{}",
            recorder.readback().0
        );
        let outcome = recorder
            .pause(None, "fsv-pause")
            .unwrap_or_else(|error| panic!("pause: {error:#}"));
        assert!(!outcome.was_paused);
        assert!(outcome.boundary_row_written);
        assert_eq!(recorder.readback().0, 2, "session_end{{edge=pause}}");
        recorder.record_accessible_event(&event);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(
            recorder.readback().0,
            2,
            "paused recorder must write zero rows for real events"
        );
        assert!(
            recorder.suppressed_counters().0 >= 1,
            "paused suppression must be counted: {:?}",
            recorder.suppressed_counters()
        );
        // Re-pause is honest about being a no-op.
        let again = recorder
            .pause(None, "fsv-pause")
            .unwrap_or_else(|error| panic!("re-pause: {error:#}"));
        assert!(again.was_paused);
        assert!(!again.boundary_row_written);

        // Resume: boundary row proves the write path, recording restarts.
        let resumed = recorder
            .resume("fsv-pause")
            .unwrap_or_else(|error| panic!("resume: {error:#}"));
        assert!(resumed.was_paused);
        assert!(resumed.boundary_row_written);
        assert_eq!(recorder.readback().0, 3, "session_start{{edge=resume}}");
        recorder.record_accessible_event(&event);
        wait_for_rows(&recorder, 4).await;

        // Exclusion: the current foreground exe stops producing rows.
        control
            .persist_exclusion_update(
                &db,
                std::slice::from_ref(&context.process_name),
                &[],
                now_ts_ns(),
                "fsv-exclude",
            )
            .unwrap_or_else(|error| panic!("exclude: {error:#}"));
        println!(
            "readback=cf_timeline edge=excluded before=rows:{} app:{}",
            recorder.readback().0,
            context.process_name
        );
        let title_changed = AccessibleEvent {
            kind: AccessibleEventKind::NameChanged,
            ..event.clone()
        };
        recorder.record_accessible_event(&event);
        recorder.record_accessible_event(&title_changed);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(
            recorder.readback().0,
            4,
            "excluded process must write zero rows even while focused"
        );
        assert!(
            recorder.suppressed_counters().1 >= 1,
            "exclusion suppression must be counted: {:?}",
            recorder.suppressed_counters()
        );

        // Removing the exclusion restores recording for a different window.
        control
            .persist_exclusion_update(
                &db,
                &[],
                std::slice::from_ref(&context.process_name),
                now_ts_ns(),
                "fsv-exclude",
            )
            .unwrap_or_else(|error| panic!("un-exclude: {error:#}"));
        let other_window = synapse_a11y::visible_top_level_window_contexts()
            .unwrap_or_else(|error| panic!("enumerate windows: {error}"))
            .into_iter()
            .find(|candidate| {
                candidate.hwnd != context.hwnd && candidate.process_name != context.process_name
            });
        if let Some(other) = other_window {
            let other_event = AccessibleEvent {
                window_id: other.hwnd,
                ..event.clone()
            };
            recorder.record_accessible_event(&other_event);
            wait_for_rows(&recorder, 5).await;
        } else {
            println!("readback=cf_timeline edge=unexclude skipped=no_second_window");
        }

        recorder.shutdown().await;
        let rows = db
            .scan_cf(cf::CF_TIMELINE)
            .unwrap_or_else(|error| panic!("scan CF_TIMELINE: {error}"));
        let records: Vec<TimelineRecord> = rows
            .iter()
            .map(|(_key, value)| {
                serde_json::from_slice(value).unwrap_or_else(|error| panic!("decode: {error}"))
            })
            .collect();
        let kinds: Vec<TimelineKind> = records.iter().map(|record| record.kind).collect();
        println!("readback=cf_timeline edge=physical_sot kinds={kinds:?}");
        assert_eq!(records[0].kind, TimelineKind::SessionStart);
        assert_eq!(records[1].kind, TimelineKind::SessionEnd);
        assert_eq!(
            records[1].payload["edge"], "pause",
            "pause boundary row must carry edge=pause: {:?}",
            records[1].payload
        );
        assert_eq!(records[2].kind, TimelineKind::SessionStart);
        assert_eq!(
            records[2].payload["edge"], "resume",
            "resume boundary row must carry edge=resume: {:?}",
            records[2].payload
        );
        assert_eq!(records[3].kind, TimelineKind::FocusChange);
        assert_eq!(
            records.last().map(|record| record.kind),
            Some(TimelineKind::SessionEnd),
            "shutdown must close the session"
        );
        // The only NameChanged event ever sent arrived while the process was
        // excluded, so no title row may exist; and the excluded-window focus
        // events must not have added a second focus row for that process.
        assert!(
            !kinds.contains(&TimelineKind::TitleChange),
            "excluded-window title event must not produce a row: {records:?}"
        );
        let focus_rows_for_excluded = records
            .iter()
            .filter(|record| {
                record.kind == TimelineKind::FocusChange
                    && record.app.as_deref() == Some(context.process_name.as_str())
            })
            .count();
        assert_eq!(
            focus_rows_for_excluded, 1,
            "only the pre-exclusion focus row may exist for {}: {records:?}",
            context.process_name
        );
    }

    #[cfg(windows)]
    async fn wait_for_rows(recorder: &ActivityRecorder, want: u64) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if recorder.readback().0 >= want {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "recorder did not reach {want} rows in time; readback={:?}",
                recorder.readback()
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    #[test]
    fn idle_transitions_fire_exactly_on_edges() {
        assert_eq!(
            idle_transition(false, 179_999, 180_000),
            None,
            "below threshold"
        );
        assert_eq!(
            idle_transition(false, 180_000, 180_000),
            Some(IdleEdge::Start),
            "threshold is inclusive"
        );
        assert_eq!(idle_transition(true, 200_000, 180_000), None, "still idle");
        assert_eq!(
            idle_transition(true, 1_000, 180_000),
            Some(IdleEdge::End),
            "input resumption ends idle"
        );
        assert_eq!(
            idle_transition(false, 0, 180_000),
            None,
            "active stays active"
        );
    }
}
