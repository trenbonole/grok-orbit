//! Read-only discovery of Grok Build sessions on disk.
//!
//! grok-build persists every session under `~/.grok/sessions/<encoded-cwd>/<session-id>/`
//! (base dir overridable with `GROK_HOME`). Each session dir holds, among others:
//!   - `summary.json`   — metadata (title, model, parent session, counters)
//!   - `updates.jsonl`  — append-only ACP event stream (the live heartbeat)
//!   - `signals.json`   — token usage / turn counters
//!
//! None of these schemas are formally documented, so everything here parses
//! defensively: unknown shapes degrade to "unknown", never to a crash. All
//! reads are size-bounded (another process writes these files), and parsed
//! metadata is cached by mtime so the 2-second rescan stays cheap.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::Value;

use crate::model::{Activity, Mission, MissionStatus, DEFAULT_CONTEXT_WINDOW};

/// Tunables for the status heuristics.
#[derive(Debug, Clone)]
pub struct ScanConfig {
    /// updates.jsonl touched within this many seconds => Burning.
    pub burn_secs: u64,
    /// ...within this many seconds => Holding. Older => Landed/Lost.
    pub hold_secs: u64,
    /// Sessions untouched for longer than this are ignored entirely.
    pub max_age_secs: u64,
    /// Assumed context window for the fuel gauge.
    pub context_window: u64,
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            burn_secs: 12,
            hold_secs: 15 * 60,
            max_age_secs: 14 * 24 * 60 * 60,
            context_window: DEFAULT_CONTEXT_WINDOW,
        }
    }
}

/// Resolve the grok-build home directory: `$GROK_HOME`, else `~/.grok`.
pub fn grok_home() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("GROK_HOME") {
        if !home.trim().is_empty() {
            return Some(PathBuf::from(home));
        }
    }
    dirs::home_dir().map(|h| h.join(".grok"))
}

/// summary.json / signals.json are small metadata files; anything bigger than
/// this is not what we think it is.
const MAX_JSON_BYTES: u64 = 4 * 1024 * 1024;
/// A recorded working-directory path can't legitimately be longer than this.
const MAX_CWD_BYTES: u64 = 64 * 1024;
/// How many bytes of updates.jsonl tail to inspect.
const TAIL_BYTES: u64 = 64 * 1024;

/// Metadata parsed from one session dir, cached across scans by file mtimes.
#[derive(Clone, Default)]
struct SessionMeta {
    summary_mtime: Option<SystemTime>,
    signals_mtime: Option<SystemTime>,
    title: Option<String>,
    model: Option<String>,
    parent_id: Option<String>,
    tokens: u64,
    turns: Option<u64>,
    /// Real context window size, when signals.json reports one.
    context_window: Option<u64>,
    /// Real context usage fraction (0..=1), when signals.json reports one.
    context_usage: Option<f64>,
}

/// Stateful scanner: caches parsed metadata and tail classification per
/// (file, mtime) so a rescan every couple of seconds stays cheap even with
/// hundreds of old sessions. Caches are pruned to the paths seen each scan.
#[derive(Default)]
pub struct Scanner {
    tail_cache: HashMap<PathBuf, (SystemTime, Activity)>,
    meta_cache: HashMap<PathBuf, SessionMeta>,
}

impl Scanner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Walk `<home>/sessions/*/*` and build a Mission per session directory.
    pub fn scan(&mut self, home: &Path, now: SystemTime, cfg: &ScanConfig) -> Vec<Mission> {
        let sessions_root = home.join("sessions");
        let mut missions = Vec::new();
        let mut seen_sessions: HashSet<PathBuf> = HashSet::new();
        let mut seen_tails: HashSet<PathBuf> = HashSet::new();
        let Ok(groups) = fs::read_dir(&sessions_root) else {
            return missions;
        };
        for group in groups.flatten() {
            let group_path = group.path();
            if !group_path.is_dir() {
                continue;
            }
            let cwd = decode_group_dir(&group_path);
            let Ok(entries) = fs::read_dir(&group_path) else {
                continue;
            };
            for entry in entries.flatten() {
                let session_path = entry.path();
                if !session_path.is_dir() {
                    continue;
                }
                if let Some(m) = self.read_session(&session_path, &cwd, now, cfg) {
                    seen_sessions.insert(session_path.clone());
                    seen_tails.insert(session_path.join("updates.jsonl"));
                    missions.push(m);
                }
            }
        }
        // Evict cache entries for sessions that aged out or were deleted.
        self.meta_cache.retain(|k, _| seen_sessions.contains(k));
        self.tail_cache.retain(|k, _| seen_tails.contains(k));

        // Newest first so the orbit view and mission log both lead with fresh missions.
        missions.sort_by_key(|m| std::cmp::Reverse(m.last_activity));
        missions
    }

    fn read_session(
        &mut self,
        path: &Path,
        cwd: &str,
        now: SystemTime,
        cfg: &ScanConfig,
    ) -> Option<Mission> {
        let id = path.file_name()?.to_string_lossy().to_string();
        let updates_path = path.join("updates.jsonl");
        let last_activity = mtime(&updates_path)
            .or_else(|| mtime(&path.join("summary.json")))
            .or_else(|| mtime(path));

        // A future mtime (clock skew, or the agent wrote between our `now`
        // snapshot and this stat) means "just now", never "ancient".
        let age = match last_activity {
            Some(t) => now.duration_since(t).unwrap_or(Duration::ZERO),
            None => Duration::from_secs(u64::MAX),
        };
        if age.as_secs() > cfg.max_age_secs {
            return None;
        }

        let meta = self.session_meta(path);
        let title = meta
            .title
            .clone()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| short_id(&id));

        // Classify the tail of every session we keep; the (path, mtime) cache
        // makes this a one-time read for finished sessions.
        let activity = self.classify_tail(&updates_path);
        let status = derive_status(age, activity, cfg);

        Some(Mission {
            id,
            title,
            cwd: cwd.to_string(),
            model: meta.model.clone(),
            is_probe: meta.parent_id.is_some(),
            parent_id: meta.parent_id.clone(),
            status,
            tokens: meta.tokens,
            turns: meta.turns,
            // Fuel: real usage fraction if reported, else real window if
            // reported, else the assumed --context-window.
            fuel_used: meta
                .context_usage
                .unwrap_or_else(|| {
                    let window = meta.context_window.unwrap_or(cfg.context_window).max(1);
                    meta.tokens as f64 / window as f64
                })
                .clamp(0.0, 1.0),
            last_activity,
            created_at: fs::metadata(path)
                .ok()
                .and_then(|m| m.created().or_else(|_| m.modified()).ok()),
            activity,
            path: path.to_path_buf(),
        })
    }

    /// Parse summary.json + signals.json for a session, reusing the cached
    /// result while both files' mtimes are unchanged.
    fn session_meta(&mut self, session_path: &Path) -> SessionMeta {
        let summary_path = session_path.join("summary.json");
        let signals_path = session_path.join("signals.json");
        let summary_mtime = mtime(&summary_path);
        let signals_mtime = mtime(&signals_path);

        if let Some(cached) = self.meta_cache.get(session_path) {
            if cached.summary_mtime == summary_mtime && cached.signals_mtime == signals_mtime {
                return cached.clone();
            }
        }

        let summary = read_json(&summary_path);
        let signals = read_json(&signals_path);
        let mut meta = SessionMeta {
            summary_mtime,
            signals_mtime,
            ..SessionMeta::default()
        };
        if let Some(s) = &summary {
            meta.title = first_string(
                s,
                &["generated_title", "session_summary", "title", "summary"],
            );
            meta.model = first_string(s, &["current_model_id", "model_id", "model"]);
            meta.parent_id = first_string(s, &["parent_session_id", "parent_id"])
                .filter(|p| !p.trim().is_empty());
        }
        if let Some(s) = &signals {
            // Known grok-build schema first (verified against v0.2.111 on
            // disk: camelCase fields), generic sniffing as drift fallback.
            meta.tokens = s
                .get("contextTokensUsed")
                .and_then(Value::as_u64)
                .unwrap_or_else(|| sniff_tokens(s));
            meta.turns = s
                .get("turnCount")
                .and_then(Value::as_u64)
                .or_else(|| sniff_counter(s, &["num_turns", "turns", "turn_count", "turncount"]));
            meta.context_window = s
                .get("contextWindowTokens")
                .and_then(Value::as_u64)
                .filter(|w| *w > 0);
            meta.context_usage = s
                .get("contextWindowUsage")
                .and_then(Value::as_f64)
                .map(|p| (p / 100.0).clamp(0.0, 1.0));
        }
        self.meta_cache
            .insert(session_path.to_path_buf(), meta.clone());
        meta
    }

    fn classify_tail(&mut self, updates_path: &Path) -> Activity {
        let Some(mt) = mtime(updates_path) else {
            return Activity::Unknown;
        };
        if let Some((cached_mt, act)) = self.tail_cache.get(updates_path) {
            if *cached_mt == mt {
                return *act;
            }
        }
        let act = classify_last_event(updates_path);
        self.tail_cache
            .insert(updates_path.to_path_buf(), (mt, act));
        act
    }

    #[cfg(test)]
    fn cache_sizes(&self) -> (usize, usize) {
        (self.meta_cache.len(), self.tail_cache.len())
    }
}

/// Status heuristic: recency buckets, with the tail event breaking the tie
/// between "landed fine" and "went down in flames".
pub fn derive_status(age: Duration, activity: Activity, cfg: &ScanConfig) -> MissionStatus {
    if activity == Activity::Error {
        return MissionStatus::Lost;
    }
    let secs = age.as_secs();
    if secs <= cfg.burn_secs {
        MissionStatus::Burning
    } else if secs <= cfg.hold_secs {
        MissionStatus::Holding
    } else {
        MissionStatus::Landed
    }
}

/// Decode a `<encoded-cwd>` group directory name back into a path for display.
/// grok-build URL-encodes the working directory; long paths fall back to a
/// slug+hash with the original recorded in a `.cwd` file inside the group.
fn decode_group_dir(group_path: &Path) -> String {
    if let Some(original) = read_capped(&group_path.join(".cwd"), MAX_CWD_BYTES) {
        let trimmed = original.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    let name = group_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    percent_decode(&name)
}

/// Minimal percent-decoder (enough for URL-encoded paths; invalid escapes pass through).
pub fn percent_decode(s: &str) -> String {
    fn hex_val(b: u8) -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push(hi * 16 + lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn mtime(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}

/// Read a regular file, refusing anything over `cap` bytes. The bound is
/// enforced at the reader (`take`), so a file that grows after the stat still
/// can't blow past it.
fn read_capped(path: &Path, cap: u64) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > cap {
        return None;
    }
    let mut text = String::new();
    fs::File::open(path)
        .ok()?
        .take(cap)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

fn read_json(path: &Path) -> Option<Value> {
    let text = read_capped(path, MAX_JSON_BYTES)?;
    serde_json::from_str(&text).ok()
}

/// First non-empty string among the given keys, searched at the root and one
/// level deep (summary.json nests some fields under `info`).
fn first_string(v: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(s) = v.get(key).and_then(Value::as_str) {
            return Some(s.to_string());
        }
    }
    if let Some(obj) = v.as_object() {
        for child in obj.values() {
            if child.is_object() {
                for key in keys {
                    if let Some(s) = child.get(key).and_then(Value::as_str) {
                        return Some(s.to_string());
                    }
                }
            }
        }
    }
    None
}

/// Guard against pathological JSON nesting (serde_json's own recursion limit
/// is 128; we stop well before doing pointless work).
const MAX_WALK_DEPTH: usize = 24;

/// signals.json schema is undocumented, so sniff token counts generically:
/// prefer an explicit total, else max(input-ish) + max(output-ish).
pub fn sniff_tokens(v: &Value) -> u64 {
    let mut totals: Vec<u64> = Vec::new();
    let mut inputs: Vec<u64> = Vec::new();
    let mut outputs: Vec<u64> = Vec::new();
    walk_numbers(v, 0, &mut |key, n| {
        let k = key.to_ascii_lowercase();
        if !k.contains("token") {
            return;
        }
        // "contextWindowTokens" is a capacity, "totalTokensBeforeCompaction"
        // is history — neither is current usage; both would poison the sums.
        if k.contains("window") || k.contains("before") {
            return;
        }
        if k.contains("total") || k.contains("used") {
            totals.push(n);
        } else if k.contains("input") || k.contains("prompt") {
            inputs.push(n);
        } else if k.contains("output") || k.contains("completion") {
            outputs.push(n);
        }
    });
    if let Some(max_total) = totals.into_iter().max() {
        return max_total;
    }
    inputs
        .into_iter()
        .max()
        .unwrap_or(0)
        .saturating_add(outputs.into_iter().max().unwrap_or(0))
}

fn sniff_counter(v: &Value, names: &[&str]) -> Option<u64> {
    let mut found: Option<u64> = None;
    walk_numbers(v, 0, &mut |key, n| {
        let k = key.to_ascii_lowercase();
        if names.iter().any(|name| k == *name) {
            found = Some(found.map_or(n, |f| f.max(n)));
        }
    });
    found
}

fn walk_numbers(v: &Value, depth: usize, f: &mut impl FnMut(&str, u64)) {
    if depth > MAX_WALK_DEPTH {
        return;
    }
    match v {
        Value::Object(map) => {
            for (k, child) in map {
                if let Some(n) = child.as_u64() {
                    f(k, n);
                }
                walk_numbers(child, depth + 1, f);
            }
        }
        Value::Array(items) => {
            for child in items {
                walk_numbers(child, depth + 1, f);
            }
        }
        _ => {}
    }
}

/// Read the last non-empty line of updates.jsonl and classify it.
fn classify_last_event(path: &Path) -> Activity {
    let Ok(mut file) = fs::File::open(path) else {
        return Activity::Unknown;
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    if len == 0 {
        return Activity::Unknown;
    }
    let start = len.saturating_sub(TAIL_BYTES);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return Activity::Unknown;
    }
    let mut buf = Vec::with_capacity(TAIL_BYTES.min(len) as usize);
    // Bound at the reader: the file may keep growing while we read.
    if file.take(TAIL_BYTES).read_to_end(&mut buf).is_err() {
        return Activity::Unknown;
    }
    let text = String::from_utf8_lossy(&buf);
    for line in text.lines().rev() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // If we started mid-line, a truncated first line may not parse; that's
        // fine — we only ever look at the last complete lines.
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            return classify_event(&v);
        }
    }
    Activity::Unknown
}

/// Classify one ACP session-update event by its structural type markers only —
/// never by message content (an assistant saying the word "error" is not a crash).
pub fn classify_event(v: &Value) -> Activity {
    let mut markers: Vec<String> = Vec::new();
    collect_type_markers(v, 0, &mut markers);
    let joined = markers.join(" ").to_ascii_lowercase();
    if v.get("error").is_some() || joined.contains("error") || joined.contains("failure") {
        return Activity::Error;
    }
    if joined.contains("permission") {
        return Activity::Permission;
    }
    if joined.contains("tool_call") || joined.contains("toolcall") {
        return Activity::ToolCall;
    }
    if joined.contains("message_chunk")
        || joined.contains("thought_chunk")
        || joined.contains("messagechunk")
    {
        return Activity::Talking;
    }
    Activity::Unknown
}

/// Gather values of type-ish fields ("sessionUpdate", "type", "kind", "method",
/// "event") from the root and up to two levels of nested objects.
fn collect_type_markers(v: &Value, depth: usize, out: &mut Vec<String>) {
    if depth > 2 {
        return;
    }
    if let Some(obj) = v.as_object() {
        for field in [
            "sessionUpdate",
            "session_update",
            "type",
            "kind",
            "method",
            "event",
        ] {
            if let Some(s) = obj.get(field).and_then(Value::as_str) {
                out.push(s.to_string());
            }
        }
        for child in obj.values() {
            if child.is_object() {
                collect_type_markers(child, depth + 1, out);
            }
        }
    }
}

/// Char-safe short display id (session dir names are arbitrary bytes; never
/// byte-slice them).
fn short_id(id: &str) -> String {
    let head: String = id.chars().take(8).collect();
    format!("mission {head}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn percent_decode_roundtrips_typical_paths() {
        assert_eq!(
            percent_decode("C%3A%5CUsers%5Cme%5Cproj"),
            "C:\\Users\\me\\proj"
        );
        assert_eq!(
            percent_decode("%2Fhome%2Fyassir%2Fcape"),
            "/home/yassir/cape"
        );
        assert_eq!(percent_decode("plain-name"), "plain-name");
        // Invalid escapes pass through untouched.
        assert_eq!(percent_decode("100%zz"), "100%zz");
        assert_eq!(percent_decode("trailing%"), "trailing%");
    }

    #[test]
    fn sniff_tokens_prefers_totals_then_sums_io() {
        assert_eq!(sniff_tokens(&json!({"total_tokens": 1234})), 1234);
        assert_eq!(
            sniff_tokens(&json!({"usage": {"input_tokens": 100, "output_tokens": 20}})),
            120
        );
        assert_eq!(
            sniff_tokens(&json!({"a": {"total_tokens": 5}, "b": {"total_tokens": 9}})),
            9
        );
        assert_eq!(sniff_tokens(&json!({"unrelated": 7})), 0);
    }

    #[test]
    fn real_grok_build_signals_schema_is_parsed() {
        // Verbatim (trimmed) signals.json from grok-build v0.2.111 on disk.
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_session(
            home,
            "C%3A%5Ctmp%5Cgrokotchi-testbed",
            "019f8241-b9ad-7541-a70a-7b127888dc97",
            &json!({
                "info": {"id": "019f8241-b9ad-7541-a70a-7b127888dc97", "cwd": "C:\\tmp\\grokotchi-testbed"},
                "session_summary": "List the files in the current directory using your tools,",
                "generated_title": "List the files in the current directory using your tools,",
                "current_model_id": "qwen2.5-coder:7b",
                "agent_name": "grok-build-plan"
            }),
            &json!({
                "turnCount": 1, "userMessageCount": 1, "errorCount": 0,
                "compactionCount": 0, "totalTokensBeforeCompaction": 0,
                "contextWindowUsage": 22, "contextTokensUsed": 7317,
                "contextWindowTokens": 32768, "toolCallCount": 0,
                "primaryModelId": "qwen2.5-coder:7b"
            }),
            &format!(
                "{}\n",
                json!({"timestamp": 1784596778, "method": "session/update",
                       "params": {"sessionId": "x", "update": {"sessionUpdate": "agent_message_chunk",
                       "content": {"type": "text", "text": "hi"}}}})
            ),
        );
        let mut scanner = Scanner::new();
        let missions = scanner.scan(home, SystemTime::now(), &ScanConfig::default());
        assert_eq!(missions.len(), 1);
        let m = &missions[0];
        assert_eq!(m.tokens, 7317, "contextTokensUsed is the real usage");
        assert_eq!(m.turns, Some(1), "turnCount is the real turn counter");
        assert!(
            (m.fuel_used - 0.22).abs() < 0.001,
            "fuel comes from contextWindowUsage percent, got {}",
            m.fuel_used
        );
        assert_eq!(m.activity, Activity::Talking);
        assert_eq!(m.model.as_deref(), Some("qwen2.5-coder:7b"));
    }

    #[test]
    fn sniff_tokens_ignores_capacity_and_history_fields() {
        // totalTokensBeforeCompaction=0 must not shadow real usage in the
        // generic fallback path, and window capacity is not usage.
        let v = json!({"totalTokensBeforeCompaction": 0, "contextTokensUsed": 7317, "contextWindowTokens": 32768});
        assert_eq!(sniff_tokens(&v), 7317);
    }

    #[test]
    fn sniff_tokens_saturates_instead_of_overflowing() {
        let v = json!({"input_tokens": u64::MAX, "output_tokens": u64::MAX});
        assert_eq!(sniff_tokens(&v), u64::MAX);
    }

    #[test]
    fn short_id_survives_multibyte_names() {
        // Byte 8 falls inside a multi-byte char — must not panic.
        assert_eq!(short_id("日本語セッション"), "mission 日本語セッション");
        assert_eq!(short_id("0198f1e2-aaaa"), "mission 0198f1e2");
        assert_eq!(short_id(""), "mission ");
    }

    #[test]
    fn classify_event_uses_structure_not_content() {
        assert_eq!(
            classify_event(&json!({"sessionUpdate": "tool_call", "title": "run tests"})),
            Activity::ToolCall
        );
        assert_eq!(
            classify_event(&json!({"update": {"sessionUpdate": "agent_message_chunk"}})),
            Activity::Talking
        );
        assert_eq!(
            classify_event(&json!({"method": "session/request_permission"})),
            Activity::Permission
        );
        assert_eq!(
            classify_event(&json!({"error": {"code": -1}})),
            Activity::Error
        );
        // The word "error" inside message CONTENT must not flag the mission as lost.
        assert_eq!(
            classify_event(&json!({
                "sessionUpdate": "agent_message_chunk",
                "content": {"text": "I fixed the error handling"}
            })),
            Activity::Talking
        );
    }

    #[test]
    fn derive_status_buckets_by_age() {
        let cfg = ScanConfig::default();
        let s = |secs, act| derive_status(Duration::from_secs(secs), act, &cfg);
        assert_eq!(s(2, Activity::ToolCall), MissionStatus::Burning);
        assert_eq!(s(60, Activity::Unknown), MissionStatus::Holding);
        assert_eq!(s(3600 * 24, Activity::Unknown), MissionStatus::Landed);
        // Errors mean LOST at any age — including 1-14 day old sessions.
        assert_eq!(s(3600 * 24 * 3, Activity::Error), MissionStatus::Lost);
        assert_eq!(s(2, Activity::Error), MissionStatus::Lost);
    }

    fn write_session(
        home: &Path,
        group: &str,
        id: &str,
        summary: &Value,
        signals: &Value,
        updates: &str,
    ) -> PathBuf {
        let session = home.join("sessions").join(group).join(id);
        fs::create_dir_all(&session).unwrap();
        fs::write(session.join("summary.json"), summary.to_string()).unwrap();
        fs::write(session.join("signals.json"), signals.to_string()).unwrap();
        fs::write(session.join("updates.jsonl"), updates).unwrap();
        session
    }

    #[test]
    fn scan_reads_a_synthetic_session_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_session(
            home,
            "C%3A%5Cwork%5Capp",
            "0198f1e2-aaaa-bbbb-cccc-000000000001",
            &json!({
                "info": {"session_id": "0198f1e2-aaaa-bbbb-cccc-000000000001"},
                "generated_title": "fix the flaky login test",
                "current_model_id": "grok-4-code",
                "num_messages": 12
            }),
            &json!({"token_usage": {"input_tokens": 90_000, "output_tokens": 4_000}, "num_turns": 7}),
            &format!(
                "{}\n{}\n",
                json!({"sessionUpdate": "agent_message_chunk", "content": "hi"}),
                json!({"sessionUpdate": "tool_call", "title": "cargo test"})
            ),
        );

        let mut scanner = Scanner::new();
        let missions = scanner.scan(home, SystemTime::now(), &ScanConfig::default());
        assert_eq!(missions.len(), 1);
        let m = &missions[0];
        assert_eq!(m.title, "fix the flaky login test");
        assert_eq!(m.cwd, "C:\\work\\app");
        assert_eq!(m.model.as_deref(), Some("grok-4-code"));
        assert_eq!(m.tokens, 94_000);
        assert_eq!(m.turns, Some(7));
        assert_eq!(m.status, MissionStatus::Burning);
        assert_eq!(m.activity, Activity::ToolCall);
        assert!(!m.is_probe);
    }

    #[test]
    fn scan_survives_garbage_files_and_unicode_names() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        // Non-ASCII session dir with no usable summary — hits the short_id path.
        let session = home.join("sessions").join("group").join("日本語セッション");
        fs::create_dir_all(&session).unwrap();
        fs::write(session.join("summary.json"), "not json at all {{{").unwrap();
        fs::write(session.join("signals.json"), "[1,2,").unwrap();
        fs::write(session.join("updates.jsonl"), "\x00\x01binary\n").unwrap();

        let mut scanner = Scanner::new();
        let missions = scanner.scan(home, SystemTime::now(), &ScanConfig::default());
        assert_eq!(missions.len(), 1);
        assert_eq!(missions[0].title, "mission 日本語セッション");
        assert_eq!(missions[0].tokens, 0);
    }

    #[test]
    fn future_mtimes_mean_just_now_not_ancient() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_session(
            home,
            "g",
            "sess-1",
            &json!({"generated_title": "racing the scanner"}),
            &json!({}),
            "{}\n",
        );
        // Scan with a `now` far in the past: every mtime is "in the future".
        // The session must still appear, as Burning (age zero), not vanish.
        let mut scanner = Scanner::new();
        let missions = scanner.scan(home, SystemTime::UNIX_EPOCH, &ScanConfig::default());
        assert_eq!(missions.len(), 1);
        assert_eq!(missions[0].status, MissionStatus::Burning);
    }

    #[test]
    fn oversized_metadata_files_are_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let session = home.join("sessions").join("g").join("sess-big");
        fs::create_dir_all(&session).unwrap();
        // 5 MB of valid JSON — over the 4 MB cap, must be skipped, not loaded.
        let big = format!(
            "{{\"total_tokens\": 42, \"pad\": \"{}\"}}",
            "x".repeat(5 * 1024 * 1024)
        );
        fs::write(session.join("signals.json"), big).unwrap();
        fs::write(session.join("updates.jsonl"), "{}\n").unwrap();

        let mut scanner = Scanner::new();
        let missions = scanner.scan(home, SystemTime::now(), &ScanConfig::default());
        assert_eq!(missions.len(), 1);
        assert_eq!(
            missions[0].tokens, 0,
            "oversized signals.json must be ignored"
        );
    }

    #[test]
    fn caches_are_pruned_when_sessions_disappear() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let session = write_session(home, "g", "sess-1", &json!({}), &json!({}), "{}\n");

        let mut scanner = Scanner::new();
        scanner.scan(home, SystemTime::now(), &ScanConfig::default());
        assert_eq!(scanner.cache_sizes(), (1, 1));

        fs::remove_dir_all(&session).unwrap();
        scanner.scan(home, SystemTime::now(), &ScanConfig::default());
        assert_eq!(
            scanner.cache_sizes(),
            (0, 0),
            "caches must not leak dead sessions"
        );
    }

    #[test]
    fn metadata_cache_hits_on_unchanged_mtime() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_session(
            home,
            "g",
            "sess-1",
            &json!({"generated_title": "cached"}),
            &json!({"total_tokens": 7}),
            "{}\n",
        );
        let mut scanner = Scanner::new();
        let a = scanner.scan(home, SystemTime::now(), &ScanConfig::default());
        let b = scanner.scan(home, SystemTime::now(), &ScanConfig::default());
        assert_eq!(a[0].title, b[0].title);
        assert_eq!(a[0].tokens, b[0].tokens);
    }

    #[test]
    fn old_error_sessions_are_lost_not_landed() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_session(
            home,
            "g",
            "sess-err",
            &json!({"generated_title": "went down in flames"}),
            &json!({}),
            &format!(
                "{}\n",
                json!({"error": {"code": -32000, "message": "boom"}})
            ),
        );
        // Age the heuristic, not the filesystem: hold_secs=0 so anything
        // non-fresh is Landed unless the tail says Error.
        let cfg = ScanConfig {
            burn_secs: 0,
            hold_secs: 0,
            ..ScanConfig::default()
        };
        // now = far future so age is huge but under max_age? No — max_age would
        // also trip. Use a now slightly ahead instead.
        let now = SystemTime::now() + Duration::from_secs(3600);
        let mut scanner = Scanner::new();
        let missions = scanner.scan(home, now, &cfg);
        assert_eq!(missions.len(), 1);
        assert_eq!(missions[0].status, MissionStatus::Lost);
    }

    #[test]
    fn cwd_file_overrides_encoded_group_name() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let group = home.join("sessions").join("longpath-abc123hash");
        let session = group.join("sess-1");
        fs::create_dir_all(&session).unwrap();
        fs::write(group.join(".cwd"), "/very/long/original/path\n").unwrap();
        fs::write(session.join("updates.jsonl"), "{}\n").unwrap();

        let mut scanner = Scanner::new();
        let missions = scanner.scan(home, SystemTime::now(), &ScanConfig::default());
        assert_eq!(missions[0].cwd, "/very/long/original/path");
    }
}
