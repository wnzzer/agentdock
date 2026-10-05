//! Token usage, read from the clients' own logs.
//!
//! Claude Code and Codex each write every model call, with its token counts,
//! into JSONL transcripts under their config directory. Reading those -- the
//! user's own, every isolated session home, every account -- counts everything
//! the host spent, including sessions started outside AgentDock, and needs no
//! cooperation from the clients.
//!
//! Files are parsed once and then only from where they were last read, since a
//! transcript only ever grows; a request after the first costs the new lines.
use crate::{AppState, Result};
use axum::{
    Json, Router,
    extract::{Query, State},
    routing::get,
};
use chrono::{DateTime, Datelike, Duration as ChronoDuration, FixedOffset, Timelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::File,
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::SystemTime,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    ClaudeCode,
    Codex,
}

/// One model call, with its tokens split the way prices are.
#[derive(Debug, Clone)]
struct Call {
    at: i64,
    provider: Provider,
    model: Arc<str>,
    /// The client's own conversation id.
    conversation: Arc<str>,
    cwd: Arc<str>,
    /// Set when the file sits in an AgentDock session's own client home.
    home_session: Option<Arc<str>>,
    /// Claude logs a reply more than once; one key per real call.
    dedupe: Option<Arc<str>>,
    tokens: Tokens,
    /// A message the person sent, rather than a model call: it counts toward
    /// messages and activity, never tokens.
    user: bool,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Tokens {
    /// Input that was neither read from nor written to the cache.
    pub input: u64,
    pub cache_read: u64,
    pub cache_write_5m: u64,
    pub cache_write_1h: u64,
    pub output: u64,
    /// Thinking or reasoning, already part of `output`.
    pub reasoning: u64,
}
impl Tokens {
    fn add(&mut self, other: &Tokens) {
        self.input += other.input;
        self.cache_read += other.cache_read;
        self.cache_write_5m += other.cache_write_5m;
        self.cache_write_1h += other.cache_write_1h;
        self.output += other.output;
        self.reasoning += other.reasoning;
    }
    pub fn total(&self) -> u64 {
        self.input + self.cache_read + self.cache_write_5m + self.cache_write_1h + self.output
    }
}

/// API list price in dollars per million tokens: input, output, cache read.
/// Cache writes are 1.25x input for five minutes and 2x for an hour. Only
/// models with a published price get one; the rest show tokens without a cost.
pub fn price(model: &str) -> Option<(f64, f64, f64)> {
    let m = model.to_ascii_lowercase();
    let m = m.strip_prefix("anthropic.").unwrap_or(&m);
    let has = |needle: &str| m.contains(needle);
    Some(if has("fable") || has("mythos") {
        (10.0, 50.0, 0.25)
    } else if has("opus-5-5") {
        (4.0, 20.0, 0.20)
    } else if has("opus-5")
        || has("opus-4-8")
        || has("opus-4-7")
        || has("opus-4-6")
        || has("opus-4-5")
    {
        (5.0, 25.0, 0.50)
    } else if has("opus-4") {
        (15.0, 75.0, 1.50)
    } else if has("sonnet-5") {
        (2.0, 10.0, 0.20)
    } else if has("sonnet-4") || has("sonnet-3-7") {
        (3.0, 15.0, 0.30)
    } else if has("haiku-4-5") {
        (1.0, 5.0, 0.10)
    } else if has("haiku-3-5") {
        (0.8, 4.0, 0.08)
    } else {
        return None;
    })
}

/// A price per million tokens, as the user set it for a model.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
}

/// The one price list: what the user set, else the published price. A model
/// reached through a gateway (or any model without a published price) costs
/// nothing until the user says what it costs them.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Prices {
    #[serde(default)]
    pub models: BTreeMap<String, Price>,
}
impl Prices {
    pub fn of(&self, model: &str) -> Option<(Price, bool)> {
        if let Some(custom) = self.models.get(&model.to_ascii_lowercase()) {
            return Some((*custom, true));
        }
        price(model).map(|(input, output, cache_read)| {
            (
                Price {
                    input,
                    output,
                    cache_read,
                },
                false,
            )
        })
    }
    pub fn cost(&self, model: &str, tokens: &Tokens) -> Option<f64> {
        self.of(model).map(|(price, _)| tokens_cost(price, tokens))
    }
}

fn prices_path(state_dir: &Path) -> PathBuf {
    state_dir.join("prices.json")
}
fn load_prices(state_dir: &Path) -> Prices {
    std::fs::read(prices_path(state_dir))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn tokens_cost(price: Price, tokens: &Tokens) -> f64 {
    let Price {
        input,
        output,
        cache_read: read,
    } = price;
    (tokens.input as f64 * input
        + tokens.cache_write_5m as f64 * input * 1.25
        + tokens.cache_write_1h as f64 * input * 2.0
        + tokens.cache_read as f64 * read
        + tokens.output as f64 * output)
        / 1_000_000.0
}

fn number(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn timestamp(value: &Value) -> Option<i64> {
    DateTime::parse_from_rfc3339(value.get("timestamp")?.as_str()?)
        .ok()
        .map(|at| at.timestamp())
}

/// What a file has given so far, and where to continue.
struct FileState {
    len: u64,
    modified: Option<SystemTime>,
    offset: u64,
    calls: Vec<Call>,
    /// Codex states its model, conversation and directory once, then counts.
    codex_model: Arc<str>,
    codex_conversation: Arc<str>,
    codex_cwd: Arc<str>,
    codex_total: u64,
}

struct Source {
    provider: Provider,
    file: PathBuf,
    home_session: Option<Arc<str>>,
}

fn empty() -> Arc<str> {
    Arc::from("")
}

/// A message the person typed: text, not a tool result passed back, not a
/// note the client wrote into the conversation itself.
fn claude_user_message(line: &str, source: &Source, state: &mut FileState) {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return;
    };
    if value.get("type").and_then(Value::as_str) != Some("user")
        || value.get("isMeta").and_then(Value::as_bool) == Some(true)
    {
        return;
    }
    let typed = match value
        .get("message")
        .and_then(|message| message.get("content"))
    {
        Some(Value::String(text)) => !text.trim().is_empty(),
        Some(Value::Array(blocks)) => {
            blocks
                .iter()
                .any(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                && !blocks
                    .iter()
                    .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
        }
        _ => false,
    };
    let Some(at) = timestamp(&value).filter(|_| typed) else {
        return;
    };
    state.calls.push(Call {
        at,
        provider: Provider::ClaudeCode,
        model: empty(),
        conversation: Arc::from(value.get("sessionId").and_then(Value::as_str).unwrap_or("")),
        cwd: Arc::from(value.get("cwd").and_then(Value::as_str).unwrap_or("")),
        home_session: source.home_session.clone(),
        dedupe: value
            .get("uuid")
            .and_then(Value::as_str)
            .map(|id| Arc::from(format!("user:{id}"))),
        user: true,
        tokens: Tokens::default(),
    });
}

fn parse_claude(line: &str, source: &Source, state: &mut FileState) {
    if line.contains("\"type\":\"user\"") {
        claude_user_message(line, source, state);
        return;
    }
    if !line.contains("\"usage\"") {
        return;
    }
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return;
    };
    let Some(message) = value.get("message") else {
        return;
    };
    let Some(usage) = message.get("usage") else {
        return;
    };
    let model = message.get("model").and_then(Value::as_str).unwrap_or("");
    if model.is_empty() || model == "<synthetic>" {
        return;
    }
    let Some(at) = timestamp(&value) else { return };
    let (write_5m, write_1h) = match usage.get("cache_creation") {
        Some(split) => (
            number(split, "ephemeral_5m_input_tokens"),
            number(split, "ephemeral_1h_input_tokens"),
        ),
        None => (number(usage, "cache_creation_input_tokens"), 0),
    };
    let dedupe = message.get("id").and_then(Value::as_str).map(|id| {
        Arc::from(format!(
            "{id}:{}",
            value.get("requestId").and_then(Value::as_str).unwrap_or("")
        ))
    });
    state.calls.push(Call {
        at,
        provider: Provider::ClaudeCode,
        model: Arc::from(model),
        conversation: Arc::from(value.get("sessionId").and_then(Value::as_str).unwrap_or("")),
        cwd: Arc::from(value.get("cwd").and_then(Value::as_str).unwrap_or("")),
        home_session: source.home_session.clone(),
        dedupe,
        user: false,
        tokens: Tokens {
            input: number(usage, "input_tokens"),
            cache_read: number(usage, "cache_read_input_tokens"),
            cache_write_5m: write_5m,
            cache_write_1h: write_1h,
            output: number(usage, "output_tokens"),
            reasoning: usage
                .get("output_tokens_details")
                .map(|details| number(details, "thinking_tokens"))
                .unwrap_or(0),
        },
    });
}

fn parse_codex(line: &str, source: &Source, state: &mut FileState) {
    let interesting = line.contains("\"token_count\"")
        || line.contains("\"task_started\"")
        || line.contains("\"turn_context\"")
        || line.contains("\"session_meta\"");
    if !interesting {
        return;
    }
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return;
    };
    let payload = value.get("payload").unwrap_or(&Value::Null);
    match value.get("type").and_then(Value::as_str) {
        Some("session_meta") => {
            if let Some(id) = payload.get("id").and_then(Value::as_str) {
                state.codex_conversation = Arc::from(id);
            }
            if let Some(cwd) = payload.get("cwd").and_then(Value::as_str) {
                state.codex_cwd = Arc::from(cwd);
            }
        }
        Some("turn_context") => {
            if let Some(model) = payload.get("model").and_then(Value::as_str) {
                state.codex_model = Arc::from(model);
            }
            if let Some(cwd) = payload.get("cwd").and_then(Value::as_str) {
                state.codex_cwd = Arc::from(cwd);
            }
        }
        // Codex starts a task for each message the person sends.
        Some("event_msg")
            if payload.get("type").and_then(Value::as_str) == Some("task_started") =>
        {
            let Some(at) = timestamp(&value) else { return };
            state.calls.push(Call {
                at,
                provider: Provider::Codex,
                model: state.codex_model.clone(),
                conversation: state.codex_conversation.clone(),
                cwd: state.codex_cwd.clone(),
                home_session: source.home_session.clone(),
                dedupe: None,
                user: true,
                tokens: Tokens::default(),
            });
        }
        Some("event_msg") if payload.get("type").and_then(Value::as_str) == Some("token_count") => {
            let Some(info) = payload.get("info").filter(|info| !info.is_null()) else {
                return;
            };
            // The same running total is reported again with a rate-limit update.
            let total = info
                .get("total_token_usage")
                .map(|t| number(t, "total_tokens"))
                .unwrap_or(0);
            if total != 0 && total == state.codex_total {
                return;
            }
            state.codex_total = total;
            let Some(last) = info.get("last_token_usage") else {
                return;
            };
            let Some(at) = timestamp(&value) else { return };
            let cached = number(last, "cached_input_tokens");
            state.calls.push(Call {
                at,
                provider: Provider::Codex,
                model: state.codex_model.clone(),
                conversation: state.codex_conversation.clone(),
                cwd: state.codex_cwd.clone(),
                home_session: source.home_session.clone(),
                dedupe: None,
                user: false,
                tokens: Tokens {
                    // OpenAI counts cached input inside input; split it out.
                    input: number(last, "input_tokens").saturating_sub(cached),
                    cache_read: cached,
                    cache_write_5m: number(last, "cache_write_input_tokens"),
                    cache_write_1h: 0,
                    output: number(last, "output_tokens"),
                    reasoning: number(last, "reasoning_output_tokens"),
                },
            });
        }
        _ => {}
    }
}

/// Read what the file has gained since last time; start over if it shrank.
fn refresh(source: &Source, state: &mut FileState) {
    let Ok(metadata) = std::fs::metadata(&source.file) else {
        return;
    };
    let modified = metadata.modified().ok();
    if metadata.len() == state.len && modified == state.modified {
        return;
    }
    if metadata.len() < state.offset {
        *state = new_state();
    }
    let Ok(mut file) = File::open(&source.file) else {
        return;
    };
    if file.seek(SeekFrom::Start(state.offset)).is_err() {
        return;
    }
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    loop {
        line.clear();
        let Ok(read) = reader.read_line(&mut line) else {
            break;
        };
        // A line still being written waits for the next read.
        if read == 0 || !line.ends_with('\n') {
            break;
        }
        state.offset += read as u64;
        match source.provider {
            Provider::ClaudeCode => parse_claude(&line, source, state),
            Provider::Codex => parse_codex(&line, source, state),
        }
    }
    state.len = metadata.len();
    state.modified = modified;
}

fn new_state() -> FileState {
    FileState {
        len: 0,
        modified: None,
        offset: 0,
        calls: Vec::new(),
        codex_model: empty(),
        codex_conversation: empty(),
        codex_cwd: empty(),
        codex_total: 0,
    }
}

fn jsonl_files(root: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() && depth > 0 {
            jsonl_files(&path, out, depth - 1);
        } else if kind.is_file() && path.extension().is_some_and(|ext| ext == "jsonl") {
            out.push(path);
        }
    }
}

/// Every transcript under the given client homes. A home is a Claude config
/// directory (projects/) or a Codex home (sessions/, archived_sessions/), or both.
fn sources(homes: &[Home]) -> Vec<Source> {
    let mut seen = HashSet::new();
    let mut sources = Vec::new();
    for (home, session) in homes {
        let home = dunce::canonicalize(home).unwrap_or_else(|_| home.clone());
        if !seen.insert(home.clone()) {
            continue;
        }
        for (provider, subdir, depth) in [
            (Provider::ClaudeCode, "projects", 3),
            (Provider::Codex, "sessions", 4),
            (Provider::Codex, "archived_sessions", 4),
        ] {
            let mut files = Vec::new();
            jsonl_files(&home.join(subdir), &mut files, depth);
            sources.extend(files.into_iter().map(|file| Source {
                provider,
                file,
                home_session: session.clone(),
            }));
        }
    }
    sources
}

fn cache() -> &'static Mutex<HashMap<PathBuf, FileState>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, FileState>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// All calls from every transcript under the homes, de-duplicated.
fn collect(homes: &[Home]) -> Vec<Call> {
    let mut cache = cache().lock().expect("usage cache");
    let mut calls = Vec::new();
    let mut seen = HashSet::new();
    for source in sources(homes) {
        let state = cache.entry(source.file.clone()).or_insert_with(new_state);
        refresh(&source, state);
        for call in &state.calls {
            if let Some(key) = &call.dedupe
                && !seen.insert(key.clone())
            {
                continue;
            }
            calls.push(call.clone());
        }
    }
    calls
}

#[derive(Debug, Default, Serialize)]
pub struct Bucket {
    #[serde(flatten)]
    pub tokens: Tokens,
    pub total: u64,
    pub cost: f64,
    /// Tokens from models without a published price, left out of `cost`.
    pub unpriced: u64,
    pub calls: u64,
    /// Messages the person sent.
    pub user_messages: u64,
    /// Five-minute slots with anything happening, in minutes.
    pub active_minutes: u64,
}
impl Bucket {
    fn add(&mut self, call: &Call, prices: &Prices) {
        if call.user {
            self.user_messages += 1;
            return;
        }
        self.tokens.add(&call.tokens);
        self.total += call.tokens.total();
        match prices.cost(&call.model, &call.tokens) {
            Some(value) => self.cost += value,
            None => self.unpriced += call.tokens.total(),
        }
        self.calls += 1;
    }
}

#[derive(Serialize)]
pub struct DayRow {
    pub date: String,
    #[serde(flatten)]
    pub bucket: Bucket,
}
#[derive(Serialize)]
pub struct ModelRow {
    pub model: String,
    pub provider: Provider,
    /// The price this model is costed at, and whether the user set it.
    pub price: Option<Price>,
    pub custom_price: bool,
    #[serde(flatten)]
    pub bucket: Bucket,
}
#[derive(Serialize)]
pub struct ConversationRow {
    pub conversation: String,
    pub provider: Provider,
    /// The AgentDock session this conversation belongs to, when known.
    pub session_id: Option<String>,
    pub title: Option<String>,
    pub cwd: String,
    pub models: Vec<String>,
    pub first_at: i64,
    pub last_at: i64,
    #[serde(flatten)]
    pub bucket: Bucket,
}
#[derive(Serialize)]
pub struct Report {
    pub from: i64,
    pub to: i64,
    pub totals: Bucket,
    /// The same length of time just before `from`, for a trend.
    pub previous: Bucket,
    pub days: Vec<DayRow>,
    pub models: Vec<ModelRow>,
    pub providers: Vec<ModelRow>,
    /// Tokens by local weekday (0 = Monday) and hour: 7 x 24.
    pub heatmap: Vec<Vec<u64>>,
    pub heatmap_cost: Vec<Vec<f64>>,
    /// Active minutes by weekday and hour.
    pub heatmap_active: Vec<Vec<u64>>,
    /// Summed span of every conversation, first event to last, in seconds.
    pub duration_seconds: i64,
    pub previous_duration_seconds: i64,
    /// What there is to filter by in this range, before any filter.
    pub options: Options,
    pub conversations: Vec<ConversationRow>,
    pub conversation_count: usize,
    pub directories: Vec<(String, u64)>,
}

#[derive(Serialize, Default)]
pub struct Options {
    pub providers: Vec<Provider>,
    pub models: Vec<String>,
    pub projects: Vec<String>,
}

/// Narrow the report to one client, one model or one project directory.
#[derive(Default, Clone)]
pub struct Filter {
    pub provider: Option<Provider>,
    pub model: Option<String>,
    pub project: Option<String>,
}
impl Filter {
    fn keeps(&self, call: &Call) -> bool {
        self.provider.is_none_or(|provider| provider == call.provider)
            // A message belongs to a conversation, not a model: keep it under a model filter.
            && self.model.as_deref().is_none_or(|model| call.user || &*call.model == model)
            && self.project.as_deref().is_none_or(|project| {
                &*call.cwd == project || call.cwd.starts_with(&format!("{}/", project.trim_end_matches('/')))
            })
    }
}

#[derive(Deserialize)]
pub struct UsageQuery {
    /// Unix seconds; defaults to thirty days ago.
    from: Option<i64>,
    to: Option<i64>,
    /// Minutes east of UTC, for local days and hours.
    tz: Option<i32>,
    provider: Option<String>,
    model: Option<String>,
    project: Option<String>,
}

/// A client home, and the AgentDock session it belongs to when it is one's own.
type Home = (PathBuf, Option<Arc<str>>);

/// Conversations met while reading, with the models each used.
type Conversations = HashMap<(Provider, Arc<str>), (ConversationRow, HashSet<Arc<str>>)>;

pub struct SessionLink {
    pub id: String,
    pub title: String,
}

fn report(
    calls: &[Call],
    from: i64,
    to: i64,
    tz_minutes: i32,
    links: &HashMap<String, SessionLink>,
    prices: &Prices,
    filter: &Filter,
) -> Report {
    let offset = FixedOffset::east_opt(tz_minutes.clamp(-14 * 60, 14 * 60) * 60)
        .unwrap_or(FixedOffset::east_opt(0).unwrap());
    let local = |at: i64| {
        DateTime::<Utc>::from_timestamp(at, 0)
            .unwrap_or_default()
            .with_timezone(&offset)
    };
    let span = (to - from).max(0);
    let mut totals = Bucket::default();
    let mut previous = Bucket::default();
    let mut days: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut models: HashMap<(String, Provider), Bucket> = HashMap::new();
    let mut providers: HashMap<Provider, Bucket> = HashMap::new();
    let mut heatmap = vec![vec![0u64; 24]; 7];
    let mut heatmap_cost = vec![vec![0f64; 24]; 7];
    // Activity is counted in five-minute slots, so a burst of calls is one slot.
    let slot = |at: i64| at.div_euclid(300);
    let mut active: HashSet<i64> = HashSet::new();
    let mut previous_active: HashSet<i64> = HashSet::new();
    let mut previous_spans: HashMap<(Provider, Arc<str>), (i64, i64)> = HashMap::new();
    let mut options_providers: HashSet<Provider> = HashSet::new();
    let mut options_models: HashMap<String, u64> = HashMap::new();
    let mut options_projects: HashMap<Arc<str>, u64> = HashMap::new();
    let mut conversations: Conversations = HashMap::new();
    let mut directories: HashMap<Arc<str>, u64> = HashMap::new();
    // Every day of the range has a row, so a quiet day shows as zero, not a gap.
    let mut day = local(from).date_naive();
    let last = local(to).date_naive();
    while day <= last && days.len() < 400 {
        days.insert(day.format("%Y-%m-%d").to_string(), Bucket::default());
        day += ChronoDuration::days(1);
    }
    for call in calls {
        let in_range = call.at >= from && call.at < to;
        if in_range {
            options_providers.insert(call.provider);
            if !call.user && !call.model.is_empty() {
                *options_models.entry(call.model.to_string()).or_default() += call.tokens.total();
            }
            if !call.cwd.is_empty() {
                *options_projects.entry(call.cwd.clone()).or_default() += call.tokens.total();
            }
        }
        if !filter.keeps(call) {
            continue;
        }
        if call.at >= from - span && call.at < from {
            previous.add(call, prices);
            previous_active.insert(slot(call.at));
            let span = previous_spans
                .entry((call.provider, call.conversation.clone()))
                .or_insert((call.at, call.at));
            span.0 = span.0.min(call.at);
            span.1 = span.1.max(call.at);
            continue;
        }
        if !in_range {
            continue;
        }
        totals.add(call, prices);
        active.insert(slot(call.at));
        let at = local(call.at);
        days.entry(at.format("%Y-%m-%d").to_string())
            .or_default()
            .add(call, prices);
        if !call.user {
            models
                .entry((call.model.to_string(), call.provider))
                .or_default()
                .add(call, prices);
            let (weekday, hour) = (
                at.weekday().num_days_from_monday() as usize,
                at.hour() as usize,
            );
            heatmap[weekday][hour] += call.tokens.total();
            heatmap_cost[weekday][hour] += prices.cost(&call.model, &call.tokens).unwrap_or(0.0);
            *directories.entry(call.cwd.clone()).or_default() += call.tokens.total();
        }
        providers
            .entry(call.provider)
            .or_default()
            .add(call, prices);
        let link = call
            .home_session
            .as_deref()
            .and_then(|id| {
                links
                    .get(id)
                    .map(|link| (id.to_string(), link.title.clone()))
            })
            .or_else(|| {
                links
                    .get(&*call.conversation)
                    .map(|link| (link.id.clone(), link.title.clone()))
            });
        let (row, model_set) = conversations
            .entry((call.provider, call.conversation.clone()))
            .or_insert_with(|| {
                (
                    ConversationRow {
                        conversation: call.conversation.to_string(),
                        provider: call.provider,
                        session_id: link.as_ref().map(|(id, _)| id.clone()),
                        title: link.as_ref().map(|(_, title)| title.clone()),
                        cwd: call.cwd.to_string(),
                        models: Vec::new(),
                        first_at: call.at,
                        last_at: call.at,
                        bucket: Bucket::default(),
                    },
                    HashSet::new(),
                )
            });
        row.first_at = row.first_at.min(call.at);
        row.last_at = row.last_at.max(call.at);
        row.bucket.add(call, prices);
        if !call.user && !call.model.is_empty() && model_set.insert(call.model.clone()) {
            row.models.push(call.model.to_string());
        }
    }
    totals.active_minutes = active.len() as u64 * 5;
    previous.active_minutes = previous_active.len() as u64 * 5;
    let mut heatmap_active = vec![vec![0u64; 24]; 7];
    for slot in &active {
        let at = local(slot * 300);
        heatmap_active[at.weekday().num_days_from_monday() as usize][at.hour() as usize] += 5;
        if let Some(day) = days.get_mut(&at.format("%Y-%m-%d").to_string()) {
            day.active_minutes += 5;
        }
    }
    let duration_seconds = conversations
        .values()
        .map(|(row, _)| row.last_at - row.first_at)
        .sum();
    let previous_duration_seconds = previous_spans
        .values()
        .map(|(first, last)| last - first)
        .sum();
    let mut options_models: Vec<(String, u64)> = options_models.into_iter().collect();
    options_models.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    let mut options_projects: Vec<(Arc<str>, u64)> = options_projects.into_iter().collect();
    options_projects.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    let options = Options {
        providers: [Provider::ClaudeCode, Provider::Codex]
            .into_iter()
            .filter(|provider| options_providers.contains(provider))
            .collect(),
        models: options_models
            .into_iter()
            .map(|(model, _)| model)
            .take(40)
            .collect(),
        projects: options_projects
            .into_iter()
            .map(|(cwd, _)| cwd.to_string())
            .take(40)
            .collect(),
    };
    let mut models: Vec<ModelRow> = models
        .into_iter()
        .map(|((model, provider), bucket)| {
            let priced = prices.of(&model);
            ModelRow {
                price: priced.map(|(price, _)| price),
                custom_price: priced.is_some_and(|(_, custom)| custom),
                model,
                provider,
                bucket,
            }
        })
        .collect();
    models.sort_by_key(|row| std::cmp::Reverse(row.bucket.total));
    let mut providers: Vec<ModelRow> = providers
        .into_iter()
        .map(|(provider, bucket)| ModelRow {
            model: String::new(),
            provider,
            price: None,
            custom_price: false,
            bucket,
        })
        .collect();
    providers.sort_by_key(|row| std::cmp::Reverse(row.bucket.total));
    let conversation_count = conversations.len();
    let mut conversations: Vec<ConversationRow> =
        conversations.into_values().map(|(row, _)| row).collect();
    conversations.sort_by_key(|row| std::cmp::Reverse(row.bucket.total));
    conversations.truncate(100);
    let mut directories: Vec<(String, u64)> = directories
        .into_iter()
        .map(|(cwd, total)| (cwd.to_string(), total))
        .collect();
    directories.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    directories.truncate(20);
    Report {
        from,
        to,
        totals,
        previous,
        days: days
            .into_iter()
            .map(|(date, bucket)| DayRow { date, bucket })
            .collect(),
        models,
        providers,
        heatmap,
        heatmap_cost,
        heatmap_active,
        duration_seconds,
        previous_duration_seconds,
        options,
        conversations,
        conversation_count,
        directories,
    }
}

/// The user's own homes, every home a session was given, every account and
/// every directory a session was pointed at; and the sessions to name calls by.
fn homes(state: &AppState) -> Result<(Vec<Home>, HashMap<String, SessionLink>)> {
    let sessions = state
        .store
        .list_sessions(None)
        .map_err(crate::ApiError::internal)?;
    let mut homes: Vec<Home> = state
        .native_sources
        .iter()
        .map(|source| (source.config_dir.clone(), None))
        .collect();
    for root in ["sessions", "accounts"] {
        if let Ok(entries) = std::fs::read_dir(state.state_dir.join(root)) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let session = (root == "sessions").then(|| Arc::from(name.as_str()));
                homes.push((entry.path(), session));
            }
        }
    }
    let mut links = HashMap::new();
    for session in &sessions {
        if let Some(dir) = &session.native_config_dir {
            homes.push((PathBuf::from(dir), None));
        }
        let link = || SessionLink {
            id: session.id.to_string(),
            title: session.title.clone(),
        };
        links.insert(session.id.to_string(), link());
        if let Some(native) = &session.provider_session_id {
            links.insert(native.clone(), link());
        }
    }
    Ok((homes, links))
}

/// Read every transcript once, in the background after start, so the usage
/// page opens on figures instead of on a first full read of months of logs.
pub fn warm(state: AppState) {
    tokio::task::spawn_blocking(move || {
        if let Ok((homes, _)) = homes(&state) {
            collect(&homes);
        }
    });
}

async fn usage(
    State(state): State<AppState>,
    Query(query): Query<UsageQuery>,
) -> Result<Json<Report>> {
    let now = Utc::now().timestamp();
    let to = query.to.unwrap_or(now).min(now + 86_400);
    let from = query.from.unwrap_or(to - 30 * 86_400).min(to);
    let tz = query.tz.unwrap_or(0);
    let filter = Filter {
        provider: match query.provider.as_deref() {
            Some("claude_code") => Some(Provider::ClaudeCode),
            Some("codex") => Some(Provider::Codex),
            _ => None,
        },
        model: query.model.filter(|model| !model.is_empty()),
        project: query.project.filter(|project| !project.is_empty()),
    };
    let (homes, links) = homes(&state)?;
    let prices = load_prices(&state.state_dir);
    let report = tokio::task::spawn_blocking(move || {
        let calls = collect(&homes);
        report(&calls, from, to, tz, &links, &prices, &filter)
    })
    .await
    .map_err(crate::ApiError::internal)?;
    Ok(Json(report))
}

async fn get_prices(State(state): State<AppState>) -> Json<Prices> {
    Json(load_prices(&state.state_dir))
}

/// Replace the user's prices. Kept small and sane: a typo must not become a
/// cost a thousand times too large without the page saying so.
async fn put_prices(
    State(state): State<AppState>,
    Json(prices): Json<Prices>,
) -> Result<Json<Prices>> {
    if prices.models.len() > 200 {
        return Err(crate::ApiError::bad("Too many model prices"));
    }
    let mut clean = Prices::default();
    for (model, price) in prices.models {
        let model = model.trim().to_ascii_lowercase();
        let sane = |value: f64| value.is_finite() && (0.0..=10_000.0).contains(&value);
        if model.is_empty()
            || model.len() > 200
            || ![price.input, price.output, price.cache_read]
                .into_iter()
                .all(sane)
        {
            return Err(crate::ApiError::bad("Invalid model price"));
        }
        clean.models.insert(model, price);
    }
    let bytes = serde_json::to_vec_pretty(&clean).map_err(crate::ApiError::internal)?;
    std::fs::write(prices_path(&state.state_dir), bytes).map_err(crate::ApiError::internal)?;
    Ok(Json(clean))
}

/// How much of a subscription window is likely left, in tokens and in what
/// they would cost through the API.
///
/// The provider reports only a percentage of the window used. Dividing what
/// this host's logs show the account spent in the same window by that
/// percentage estimates the whole window; what is left follows. It counts only
/// use from this host, so an account also used elsewhere reads optimistic.
#[derive(Serialize)]
pub struct Allowance {
    pub account_id: String,
    pub account: String,
    pub provider: Provider,
    /// The subscription, as the provider names it (Max, Pro, Plus…).
    pub plan: Option<String>,
    pub window: &'static str,
    pub window_minutes: f64,
    pub used_percent: f64,
    pub window_start: i64,
    pub resets_at: i64,
    pub used_tokens: u64,
    pub used_cost: f64,
    pub estimated_total_tokens: Option<u64>,
    pub estimated_remaining_tokens: Option<u64>,
    pub estimated_total_cost: Option<f64>,
    pub estimated_remaining_cost: Option<f64>,
    /// Too little used yet, or too little seen here, to extrapolate from.
    pub low_confidence: bool,
}

fn epoch(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number.as_f64().map(|n| {
            if n > 1e12 {
                (n / 1000.0) as i64
            } else {
                n as i64
            }
        }),
        Value::String(text) => DateTime::parse_from_rfc3339(text)
            .ok()
            .map(|at| at.timestamp())
            .or_else(|| text.parse::<f64>().ok().map(|n| n as i64)),
        _ => None,
    }
}

pub(crate) fn estimate(
    used_percent: f64,
    used_tokens: u64,
    used_cost: f64,
) -> (Option<u64>, Option<u64>, Option<f64>, Option<f64>, bool) {
    if used_percent <= 0.0 || used_tokens == 0 {
        return (None, None, None, None, true);
    }
    let share = (used_percent / 100.0).min(1.0);
    let total = (used_tokens as f64 / share) as u64;
    let total_cost = (used_cost > 0.0).then(|| used_cost / share);
    let low = used_percent < 5.0 || used_tokens < 100_000;
    (
        Some(total),
        Some(total.saturating_sub(used_tokens)),
        total_cost,
        total_cost.map(|cost| (cost - used_cost).max(0.0)),
        low,
    )
}

async fn allowance(State(state): State<AppState>) -> Result<Json<Vec<Allowance>>> {
    let Json(accounts) = crate::accounts::list(State(state.clone())).await?;
    let prices = load_prices(&state.state_dir);
    let now = Utc::now().timestamp();
    let rows = tokio::task::spawn_blocking(move || {
        let mut rows = Vec::new();
        for account in accounts {
            let Some(limits) = &account.limits else {
                continue;
            };
            let provider = match account.provider {
                agentdock_domain::ProviderKind::Codex => Provider::Codex,
                _ => Provider::ClaudeCode,
            };
            let calls = collect(&[(PathBuf::from(&account.storage_path), None)]);
            for (window, limit) in [
                ("primary", &limits.primary),
                ("secondary", &limits.secondary),
            ] {
                let Some(limit) = limit else { continue };
                let (Some(minutes), Some(resets_at)) = (
                    // A window named but not measured is still a known length.
                    limit.window_minutes.or(match limit.window_kind {
                        Some(crate::accounts::WindowKind::FiveHour) => Some(300.0),
                        Some(crate::accounts::WindowKind::Weekly) => Some(10_080.0),
                        None => None,
                    }),
                    limit.resets_at.as_ref().and_then(epoch),
                ) else {
                    continue;
                };
                let start = resets_at - (minutes * 60.0) as i64;
                if resets_at < now {
                    continue;
                }
                let (mut tokens, mut cost) = (0u64, 0f64);
                for call in calls
                    .iter()
                    .filter(|call| call.provider == provider && call.at >= start)
                {
                    tokens += call.tokens.total();
                    cost += prices.cost(&call.model, &call.tokens).unwrap_or(0.0);
                }
                let (total, remaining, total_cost, remaining_cost, low) =
                    estimate(limit.used_percent, tokens, cost);
                rows.push(Allowance {
                    account_id: account.id.to_string(),
                    account: account.name.clone(),
                    provider,
                    plan: account.plan.clone(),
                    window,
                    window_minutes: minutes,
                    used_percent: limit.used_percent,
                    window_start: start,
                    resets_at,
                    used_tokens: tokens,
                    used_cost: cost,
                    estimated_total_tokens: total,
                    estimated_remaining_tokens: remaining,
                    estimated_total_cost: total_cost,
                    estimated_remaining_cost: remaining_cost,
                    low_confidence: low,
                });
            }
        }
        rows
    })
    .await
    .map_err(crate::ApiError::internal)?;
    Ok(Json(rows))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/usage", get(usage))
        .route("/api/usage/prices", get(get_prices).put(put_prices))
        .route("/api/usage/allowance", get(allowance))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(path: &Path, lines: &[Value]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
    }

    #[test]
    fn transcripts_of_both_clients_are_counted_once_each() {
        let home = std::env::temp_dir().join(format!("agentdock-usage-{}", uuid::Uuid::new_v4()));
        let claude = home.join("projects/-repo/c1.jsonl");
        let reply = serde_json::json!({"type":"assistant","timestamp":"2026-10-01T10:00:00Z","sessionId":"c1","cwd":"/repo","requestId":"r1",
            "message":{"id":"m1","model":"claude-opus-5-5","usage":{"input_tokens":10,"cache_read_input_tokens":1000,"output_tokens":100,
            "cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":200},"output_tokens_details":{"thinking_tokens":40}}}});
        // The same reply logged twice is one call.
        write(&claude, &[reply.clone(), reply]);
        let codex = home.join("sessions/2026/10/01/rollout.jsonl");
        let count = |total: u64, input: u64| {
            serde_json::json!({"timestamp":"2026-10-01T11:00:00Z","type":"event_msg","payload":{"type":"token_count","info":{
            "total_token_usage":{"total_tokens":total},"last_token_usage":{"input_tokens":input,"cached_input_tokens":60,"output_tokens":5,"reasoning_output_tokens":2}}}})
        };
        write(
            &codex,
            &[
                serde_json::json!({"timestamp":"2026-10-01T11:00:00Z","type":"session_meta","payload":{"id":"x1","cwd":"/repo"}}),
                serde_json::json!({"timestamp":"2026-10-01T11:00:00Z","type":"turn_context","payload":{"model":"gpt-6-astra","cwd":"/repo"}}),
                count(105, 100),
                // A repeated running total is a rate-limit update, not a call.
                count(105, 100),
            ],
        );
        let homes = vec![(home.clone(), None)];
        let calls = collect(&homes);
        assert_eq!(calls.len(), 2);
        let from = DateTime::parse_from_rfc3339("2026-10-01T00:00:00Z")
            .unwrap()
            .timestamp();
        let mut links = HashMap::new();
        links.insert(
            "c1".to_string(),
            SessionLink {
                id: "s1".into(),
                title: "Fixture".into(),
            },
        );
        let result = report(
            &calls,
            from,
            from + 86_400,
            480,
            &links,
            &Prices::default(),
            &Filter::default(),
        );
        assert_eq!(result.totals.calls, 2);
        assert_eq!(result.totals.tokens.cache_read, 1060);
        assert_eq!(
            result.totals.tokens.input,
            10 + 40,
            "Codex's cached input is split out of its input"
        );
        assert_eq!(result.totals.tokens.cache_write_1h, 200);
        // Opus 5.5: 10 x $4 + 200 x $8 + 1000 x $0.20 + 100 x $20, per million.
        assert!((result.totals.cost - (40.0 + 1600.0 + 200.0 + 2000.0) / 1e6).abs() < 1e-12);
        assert_eq!(
            result.totals.unpriced,
            40 + 60 + 5,
            "a model without a published price has no cost"
        );
        let linked = result
            .conversations
            .iter()
            .find(|row| row.conversation == "c1")
            .unwrap();
        assert_eq!(linked.session_id.as_deref(), Some("s1"));
        assert_eq!(
            result.days.len(),
            2,
            "both local days the range touches have a row"
        );

        // An appended line is all a second read costs, and it is counted.
        write(&codex, &[count(300, 150)]);
        assert_eq!(collect(&homes).len(), 3);
        std::fs::remove_dir_all(home).ok();
    }

    #[test]
    fn messages_activity_and_filters() {
        let home = std::env::temp_dir().join(format!("agentdock-usage-{}", uuid::Uuid::new_v4()));
        let claude = home.join("projects/-repo/c2.jsonl");
        let user = |uuid: &str, at: &str, content: Value| serde_json::json!({"type":"user","uuid":uuid,"timestamp":at,"sessionId":"c2","cwd":"/repo/app","message":{"role":"user","content":content}});
        let reply = |id: &str, at: &str| {
            serde_json::json!({"type":"assistant","timestamp":at,"sessionId":"c2","cwd":"/repo/app","requestId":id,
            "message":{"id":id,"model":"claude-sonnet-5-5","usage":{"input_tokens":100,"output_tokens":10}}})
        };
        write(
            &claude,
            &[
                user(
                    "u1",
                    "2026-10-01T10:00:00Z",
                    Value::String("Build it".into()),
                ),
                reply("m1", "2026-10-01T10:01:00Z"),
                // A tool result comes back as a user line; the person did not type it.
                user(
                    "u2",
                    "2026-10-01T10:02:00Z",
                    serde_json::json!([{"type":"tool_result","content":"ok"}]),
                ),
                user(
                    "u3",
                    "2026-10-01T10:20:00Z",
                    serde_json::json!([{"type":"text","text":"Now test it"}]),
                ),
                reply("m2", "2026-10-01T10:21:00Z"),
            ],
        );
        let calls = collect(&[(home.clone(), None)]);
        let from = DateTime::parse_from_rfc3339("2026-10-01T00:00:00Z")
            .unwrap()
            .timestamp();
        let all = report(
            &calls,
            from,
            from + 86_400,
            0,
            &HashMap::new(),
            &Prices::default(),
            &Filter::default(),
        );
        assert_eq!(all.totals.user_messages, 2);
        assert_eq!(all.totals.calls, 2);
        assert_eq!(
            all.totals.active_minutes, 10,
            "two separate five-minute slots"
        );
        assert_eq!(all.duration_seconds, 21 * 60);
        assert_eq!(all.options.projects, vec!["/repo/app".to_string()]);
        let elsewhere = Filter {
            project: Some("/other".into()),
            ..Filter::default()
        };
        assert_eq!(
            report(
                &calls,
                from,
                from + 86_400,
                0,
                &HashMap::new(),
                &Prices::default(),
                &elsewhere
            )
            .totals
            .total,
            0
        );
        let parent = Filter {
            project: Some("/repo".into()),
            model: Some("claude-sonnet-5-5".into()),
            ..Filter::default()
        };
        let narrowed = report(
            &calls,
            from,
            from + 86_400,
            0,
            &HashMap::new(),
            &Prices::default(),
            &parent,
        );
        assert_eq!(
            (narrowed.totals.calls, narrowed.totals.user_messages),
            (2, 2),
            "a project includes its subdirectories"
        );
        std::fs::remove_dir_all(home).ok();
    }

    #[test]
    fn a_user_price_wins_and_an_unpriced_model_gains_a_cost() {
        let mut prices = Prices::default();
        prices.models.insert(
            "gpt-6-astra".into(),
            Price {
                input: 1.0,
                output: 8.0,
                cache_read: 0.1,
            },
        );
        let tokens = Tokens {
            input: 1_000_000,
            output: 1_000_000,
            cache_read: 1_000_000,
            ..Tokens::default()
        };
        assert_eq!(prices.cost("GPT-6-Astra", &tokens), Some(9.1));
        assert!(
            prices
                .of("claude-opus-5-5")
                .is_some_and(|(_, custom)| !custom)
        );
        assert_eq!(prices.cost("mystery-model", &tokens), None);
    }

    #[test]
    fn the_rest_of_a_window_is_extrapolated_from_what_was_spent_in_it() {
        let (total, remaining, total_cost, remaining_cost, low) = estimate(25.0, 2_000_000, 10.0);
        assert_eq!((total, remaining), (Some(8_000_000), Some(6_000_000)));
        assert_eq!((total_cost, remaining_cost), (Some(40.0), Some(30.0)));
        assert!(!low);
        assert!(
            estimate(2.0, 2_000_000, 1.0).4,
            "two percent is too little to extrapolate from"
        );
        assert_eq!(estimate(0.0, 10, 0.0).0, None);
    }

    #[test]
    fn prices_exist_only_for_published_models() {
        assert_eq!(price("claude-opus-5-5"), Some((4.0, 20.0, 0.20)));
        assert_eq!(price("claude-sonnet-4-5-20250929"), Some((3.0, 15.0, 0.30)));
        assert_eq!(price("claude-opus-4-1-20250805"), Some((15.0, 75.0, 1.50)));
        assert_eq!(price("claude-fable-5-1"), Some((10.0, 50.0, 0.25)));
        assert_eq!(price("gpt-6-astra"), None);
    }
}
