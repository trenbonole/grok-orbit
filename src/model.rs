//! Shared domain types: what we know about a Grok Build session ("mission").

use std::path::PathBuf;
use std::time::SystemTime;

/// How much context a mission is assumed to have before auto-compact kicks in.
/// grok-build doesn't expose the per-model window in session files, so the fuel
/// gauge is normalized against this (overridable with --context-window).
pub const DEFAULT_CONTEXT_WINDOW: u64 = 256_000;

/// A mission is one Grok Build session directory.
#[derive(Debug, Clone)]
pub struct Mission {
    /// Session ID (directory name, usually a UUIDv7).
    pub id: String,
    /// Human title: generated_title / session_summary from summary.json, or the id.
    pub title: String,
    /// Decoded working directory the session ran in.
    pub cwd: String,
    /// Model id last used, if known.
    pub model: Option<String>,
    /// Parent session id when this is a fork or a subagent child.
    pub parent_id: Option<String>,
    /// True when this session lives under another session's lineage (rendered as a probe).
    pub is_probe: bool,
    pub status: MissionStatus,
    /// Combined token count sniffed from signals.json (0 when unknown).
    pub tokens: u64,
    /// Turn count if we could sniff it.
    pub turns: Option<u64>,
    /// 0.0..=1.0 — fraction of the assumed context window consumed.
    pub fuel_used: f64,
    /// Last time any file in the session dir changed.
    pub last_activity: Option<SystemTime>,
    /// When the session was created (from summary.json or dir mtime).
    pub created_at: Option<SystemTime>,
    /// What the agent seems to be doing right now (from the tail of updates.jsonl).
    pub activity: Activity,
    /// Full path to the session directory.
    pub path: PathBuf,
}

/// Mission state, derived from file mtimes + the tail of updates.jsonl.
/// The mapping is heuristic and documented in the README.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissionStatus {
    /// updates.jsonl written to in the last few seconds — agent is working.
    Burning,
    /// Recently active but quiet — likely waiting for the user.
    Holding,
    /// No activity for a while and the last event looked ok — mission complete.
    Landed,
    /// The last event in updates.jsonl looked like an error.
    Lost,
}

impl MissionStatus {
    pub fn label(self) -> &'static str {
        match self {
            MissionStatus::Burning => "BURN",
            MissionStatus::Holding => "HOLD",
            MissionStatus::Landed => "LANDED",
            MissionStatus::Lost => "LOST",
        }
    }
}

/// Coarse classification of the newest event in updates.jsonl.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Activity {
    #[default]
    Unknown,
    /// Streaming an answer or thinking.
    Talking,
    /// Running a tool (payload deployment, in mission-control speak).
    ToolCall,
    /// Waiting on a permission request.
    Permission,
    /// The last event carried an error.
    Error,
}

impl Activity {
    pub fn blurb(self) -> &'static str {
        match self {
            Activity::Unknown => "telemetry nominal",
            Activity::Talking => "transmitting",
            Activity::ToolCall => "deploying payload",
            Activity::Permission => "awaiting launch clearance",
            Activity::Error => "Houston, we have a problem",
        }
    }
}
