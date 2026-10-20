//! Live model pricing from `https://models.dev/api.json`, cached locally for
//! 24h, with fuzzy model-key matching and user-defined price overrides.
//!
//! Implementation notes:
//! - `FAMILY_DEFS` tests are hand-rolled string matchers (no regex
//!   dependency), including word boundaries and the `gpt-4o(?!-mini)`
//!   negative lookahead.
//! - The in-memory index lives in [`PricingState`] (per-process).
//! - `match_record` returns an index into [`PricingState::live_records`]
//!   rather than a record reference, to satisfy the borrow checker.

use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const PRICING_SOURCE_URL: &str = "https://models.dev/api.json";
pub const PRICING_SOURCE_LABEL: &str = "models.dev";
pub const PRICING_TTL_MS: i64 = 24 * 60 * 60 * 1000;
pub const PRICING_FETCH_TIMEOUT_MS: u64 = 8000;
pub const PRICING_SOURCE_CUSTOM: &str = "custom";

/// Mean chars-per-token divisor used when a session reports no actual token
/// usage.
pub const TOKEN_CHARS_PER_TOKEN: f64 = 3.8;

/// Origin providers win when several providers list the same model id.
/// Providers absent from this list sort last.
const CANONICAL_PROVIDERS: [&str; 14] = [
    "anthropic",
    "openai",
    "google",
    "deepseek",
    "google-vertex",
    "xai",
    "meta",
    "mistral",
    "alibaba",
    "qwen",
    "moonshotai",
    "minimax",
    "zhipu",
    "morph",
];

// ---------------------------------------------------------------------------
// Family fallbacks
// ---------------------------------------------------------------------------

/// Built-in fallback rate for one model family.
#[derive(Debug, Clone, Copy)]
pub struct FamilyDef {
    pub id: &'static str,
    pub display_name: &'static str,
    pub in_per_1m: f64,
    pub out_per_1m: f64,
    pub cache_per_1m: f64,
}

/// Model family definitions. The `test` patterns are implemented by
/// [`family_matches`] (no regex crate needed).
pub static FAMILY_DEFS: &[FamilyDef] = &[
    FamilyDef {
        id: "claude-opus",
        display_name: "Claude Opus (3 / 4)",
        in_per_1m: 15.0,
        out_per_1m: 75.0,
        cache_per_1m: 1.5,
    },
    FamilyDef {
        id: "claude-sonnet",
        display_name: "Claude 3.7 / 3.5 Sonnet",
        in_per_1m: 3.0,
        out_per_1m: 15.0,
        cache_per_1m: 0.3,
    },
    FamilyDef {
        id: "claude-haiku",
        display_name: "Claude 3.5 Haiku",
        in_per_1m: 0.8,
        out_per_1m: 4.0,
        cache_per_1m: 0.08,
    },
    FamilyDef {
        id: "o-series",
        display_name: "OpenAI o1 / o3",
        in_per_1m: 15.0,
        out_per_1m: 60.0,
        cache_per_1m: 3.75,
    },
    FamilyDef {
        id: "o-mini",
        display_name: "OpenAI o1 / o3 Mini",
        in_per_1m: 1.1,
        out_per_1m: 4.4,
        cache_per_1m: 0.55,
    },
    FamilyDef {
        id: "gpt-4o",
        display_name: "GPT-4o",
        in_per_1m: 2.5,
        out_per_1m: 10.0,
        cache_per_1m: 1.25,
    },
    FamilyDef {
        id: "gpt-4o-mini",
        display_name: "GPT-4o Mini",
        in_per_1m: 0.15,
        out_per_1m: 0.6,
        cache_per_1m: 0.075,
    },
    FamilyDef {
        id: "gpt-5-6",
        display_name: "GPT-5 / GPT-6 Series",
        in_per_1m: 2.0,
        out_per_1m: 8.0,
        cache_per_1m: 0.5,
    },
    FamilyDef {
        id: "gpt-4-legacy",
        display_name: "GPT-4 Series",
        in_per_1m: 10.0,
        out_per_1m: 30.0,
        cache_per_1m: 2.5,
    },
    FamilyDef {
        id: "gemini-flash",
        display_name: "Gemini 2.0 / 3.5 Flash",
        in_per_1m: 0.1,
        out_per_1m: 0.4,
        cache_per_1m: 0.025,
    },
    FamilyDef {
        id: "gemini-pro",
        display_name: "Gemini 1.5 / 2.5 Pro",
        in_per_1m: 1.25,
        out_per_1m: 5.0,
        cache_per_1m: 0.3,
    },
    FamilyDef {
        id: "deepseek",
        display_name: "DeepSeek V3 / R1 / Flash",
        in_per_1m: 0.14,
        out_per_1m: 0.28,
        cache_per_1m: 0.014,
    },
    FamilyDef {
        id: "qwen",
        display_name: "Qwen 2.5 / 3 / 3.6",
        in_per_1m: 0.2,
        out_per_1m: 0.6,
        cache_per_1m: 0.05,
    },
    FamilyDef {
        id: "muse-spark",
        display_name: "Muse Spark",
        in_per_1m: 0.5,
        out_per_1m: 1.5,
        cache_per_1m: 0.1,
    },
    FamilyDef {
        id: "minimax-mimo",
        display_name: "MiniMax / MiMo / Step",
        in_per_1m: 0.4,
        out_per_1m: 1.2,
        cache_per_1m: 0.1,
    },
    FamilyDef {
        id: "free-tier",
        display_name: "Free / Local Models",
        in_per_1m: 0.0,
        out_per_1m: 0.0,
        cache_per_1m: 0.0,
    },
    FamilyDef {
        id: "standard",
        display_name: "Standard Coding Model",
        in_per_1m: 1.5,
        out_per_1m: 6.0,
        cache_per_1m: 0.375,
    },
];

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Regex-style `\b<needle>\b` match on an already-lowercased haystack.
fn contains_word(hay: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let bytes = hay.as_bytes();
    let mut start = 0;
    while start + needle.len() <= hay.len() {
        match hay[start..].find(needle) {
            Some(rel) => {
                let s = start + rel;
                let e = s + needle.len();
                let left_ok = s == 0 || !is_word_byte(bytes[s - 1]);
                let right_ok = e >= bytes.len() || !is_word_byte(bytes[e]);
                if left_ok && right_ok {
                    return true;
                }
                start = s + 1;
            }
            None => break,
        }
    }
    false
}

fn gemini_then(hay: &str, second: &str) -> bool {
    match hay.find("gemini") {
        Some(pos) => hay[pos + "gemini".len()..].contains(second),
        None => false,
    }
}

/// Evaluates `FAMILY_DEFS[*].test` for one family. `model` may be any case;
/// matching is case-insensitive.
pub fn family_matches(family_id: &str, model: &str) -> bool {
    let hay = model.to_lowercase();
    match family_id {
        "claude-opus" => hay.contains("opus"),
        "claude-sonnet" => hay.contains("sonnet"),
        "claude-haiku" => hay.contains("haiku"),
        // /\b(o1|o3)\b/i
        "o-series" => contains_word(&hay, "o1") || contains_word(&hay, "o3"),
        // /\b(o1-mini|o3-mini)\b/i
        "o-mini" => contains_word(&hay, "o1-mini") || contains_word(&hay, "o3-mini"),
        // /gpt-4o(?!-mini)/i
        "gpt-4o" => hay.contains("gpt-4o") && !hay.contains("gpt-4o-mini"),
        // /(gpt-4o-mini|gpt-4\.1-mini|mini.*free)/i
        "gpt-4o-mini" => {
            hay.contains("gpt-4o-mini")
                || hay.contains("gpt-4.1-mini")
                || (hay.contains("mini")
                    && hay
                        .find("mini")
                        .map(|p| hay[p..].contains("free"))
                        .unwrap_or(false))
        }
        // /(gpt-5|gpt-6|sol|terra|luna|astra)/i
        "gpt-5-6" => {
            hay.contains("gpt-5")
                || hay.contains("gpt-6")
                || hay.contains("sol")
                || hay.contains("terra")
                || hay.contains("luna")
                || hay.contains("astra")
        }
        "gpt-4-legacy" => hay.contains("gpt-4"),
        // /gemini.*flash/i
        "gemini-flash" => gemini_then(&hay, "flash"),
        // /gemini.*pro/i
        "gemini-pro" => gemini_then(&hay, "pro"),
        "deepseek" => hay.contains("deepseek"),
        "qwen" => hay.contains("qwen"),
        "muse-spark" => hay.contains("muse-spark"),
        // /(minimax|mimo|stepfun|step)/i
        "minimax-mimo" => {
            hay.contains("minimax")
                || hay.contains("mimo")
                || hay.contains("stepfun")
                || hay.contains("step")
        }
        // /(:free|\bfree\b|ollama|nomic)/i
        "free-tier" => {
            hay.contains(":free")
                || contains_word(&hay, "free")
                || hay.contains("ollama")
                || hay.contains("nomic")
        }
        "standard" => true,
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Serde structs (camelCase wire format)
// ---------------------------------------------------------------------------

/// One flattened models.dev record.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveRecord {
    pub provider: String,
    pub model_id: String,
    pub name: String,
    pub family: String,
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub last_updated: String,
}

/// Resolved pricing for a session model key.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPricing {
    pub id: String,
    pub display_name: String,
    #[serde(rename = "inPer1M", alias = "inPer1m")]
    pub in_per_1m: f64,
    #[serde(rename = "outPer1M", alias = "outPer1m")]
    pub out_per_1m: f64,
    #[serde(rename = "cachePer1M", alias = "cachePer1m")]
    pub cache_per_1m: f64,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

/// One row of the curated pricing matrix.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PricingReferenceRow {
    pub id: String,
    pub name: String,
    #[serde(rename = "inPer1M", alias = "inPer1m")]
    pub in_per_1m: f64,
    #[serde(rename = "outPer1M", alias = "outPer1m")]
    pub out_per_1m: f64,
    #[serde(rename = "cachePer1M", alias = "cachePer1m")]
    pub cache_per_1m: f64,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

/// Cache status: `{source, updatedAt, modelCount, stale, overrideCount}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PricingStatus {
    pub source: String,
    pub source_url: String,
    pub updated_at: Option<String>,
    pub model_count: usize,
    pub stale: bool,
    pub ttl_hours: f64,
    pub override_count: usize,
}

/// Public override shape (`provider` + `modelId` primary key).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceOverride {
    pub provider: String,
    pub model_id: String,
    pub display_name: String,
    #[serde(rename = "inPer1M", alias = "inPer1m")]
    pub in_per_1m: f64,
    #[serde(rename = "outPer1M", alias = "outPer1m")]
    pub out_per_1m: f64,
    #[serde(rename = "cachePer1M", alias = "cachePer1m")]
    pub cache_per_1m: f64,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
struct OverrideRow {
    provider: String,
    model_id: String,
    display_name: String,
    in_per_1m: f64,
    out_per_1m: f64,
    cache_per_1m: f64,
    updated_at: String,
}

/// In-memory live index + overrides (per-process).
#[derive(Default)]
pub struct PricingState {
    pub live_records: Vec<LiveRecord>,
    pub live_by_id: HashMap<String, usize>,
    pub live_updated_at: Option<String>,
    pub match_memo: HashMap<String, Option<usize>>,
    custom_overrides: Vec<OverrideRow>,
    custom_wildcard_by_model: HashMap<String, usize>,
    custom_specific_by_key: HashMap<String, usize>,
    custom_by_model: HashMap<String, Vec<usize>>,
}

impl PricingState {
    pub fn new() -> Self {
        Self::default()
    }
}

// ---------------------------------------------------------------------------
// Index helpers
// ---------------------------------------------------------------------------

fn provider_rank(provider: &str) -> usize {
    let lower = provider.to_lowercase();
    CANONICAL_PROVIDERS
        .iter()
        .position(|p| *p == lower)
        .unwrap_or(CANONICAL_PROVIDERS.len())
}

fn normalize_key(model_key: &str) -> String {
    model_key.trim().to_lowercase()
}

/// Flatten the models.dev payload into priced records, deduped by model id
/// with origin providers preferred. Pure function (no DB, no network).
pub fn build_pricing_index(raw: &Value) -> (Vec<LiveRecord>, HashMap<String, usize>) {
    let mut best: HashMap<String, LiveRecord> = HashMap::new();
    let Some(obj) = raw.as_object() else {
        return (Vec::new(), HashMap::new());
    };
    for (provider_id, provider) in obj {
        let Some(models) = provider.get("models").and_then(|m| m.as_object()) else {
            continue;
        };
        for (model_id, model) in models {
            let Some(cost) = model.get("cost") else {
                continue;
            };
            let input = cost
                .get("input")
                .and_then(|v| v.as_f64())
                .unwrap_or(f64::NAN);
            let output = cost
                .get("output")
                .and_then(|v| v.as_f64())
                .unwrap_or(f64::NAN);
            if !input.is_finite() || !output.is_finite() {
                continue;
            }
            let cache_read = cost
                .get("cache_read")
                .and_then(|v| v.as_f64())
                .unwrap_or(f64::NAN);
            let record = LiveRecord {
                provider: provider_id.clone(),
                model_id: model_id.clone(),
                name: model
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or(model_id)
                    .to_string(),
                family: model
                    .get("family")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                input,
                output,
                cache_read: if cache_read.is_finite() {
                    cache_read
                } else {
                    0.0
                },
                last_updated: model
                    .get("last_updated")
                    .and_then(|v| v.as_str())
                    .or_else(|| model.get("release_date").and_then(|v| v.as_str()))
                    .unwrap_or("")
                    .to_string(),
            };
            let key = record.model_id.to_lowercase();
            let replace = match best.get(&key) {
                None => true,
                Some(prev) => provider_rank(&record.provider) < provider_rank(&prev.provider),
            };
            if replace {
                best.insert(key, record);
            }
        }
    }
    let records: Vec<LiveRecord> = best.into_values().collect();
    let mut by_id = HashMap::new();
    for (i, r) in records.iter().enumerate() {
        by_id.insert(r.model_id.to_lowercase(), i);
    }
    (records, by_id)
}

pub fn set_live_index(
    state: &mut PricingState,
    records: Vec<LiveRecord>,
    updated_at: Option<String>,
) {
    let mut by_id = HashMap::new();
    for (i, r) in records.iter().enumerate() {
        by_id.insert(r.model_id.to_lowercase(), i);
    }
    state.live_records = records;
    state.live_by_id = by_id;
    state.live_updated_at = updated_at;
    state.match_memo.clear();
}

fn compare_records(records: &[LiveRecord], a: usize, b: usize) -> std::cmp::Ordering {
    let rank = provider_rank(&records[a].provider).cmp(&provider_rank(&records[b].provider));
    if rank != std::cmp::Ordering::Equal {
        return rank;
    }
    let (au, bu) = (&records[a].last_updated, &records[b].last_updated);
    if au != bu {
        return if au < bu {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Less
        };
    }
    std::cmp::Ordering::Equal
}

fn compare_family(records: &[LiveRecord], a: usize, b: usize) -> std::cmp::Ordering {
    let rank = provider_rank(&records[a].provider).cmp(&provider_rank(&records[b].provider));
    if rank != std::cmp::Ordering::Equal {
        return rank;
    }
    let len = records[a].model_id.len().cmp(&records[b].model_id.len());
    if len != std::cmp::Ordering::Equal {
        return len;
    }
    let (au, bu) = (&records[a].last_updated, &records[b].last_updated);
    if au != bu {
        return if au < bu {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Less
        };
    }
    std::cmp::Ordering::Equal
}

fn find_family_record(state: &PricingState, family_id: &str) -> Option<usize> {
    // Test the model id first so key-style patterns (e.g. gpt-4o vs -mini,
    // mini.*free, sol/terra/luna) can't over-match display names; only fall
    // back to name/family text when no id matches.
    type Haystack = Box<dyn Fn(&LiveRecord) -> String>;
    let hays: [Haystack; 2] = [
        Box::new(|r| r.model_id.clone()),
        Box::new(|r| format!("{} {} {}", r.model_id, r.name, r.family)),
    ];
    for hay in &hays {
        let mut best: Option<usize> = None;
        for (i, r) in state.live_records.iter().enumerate() {
            if !family_matches(family_id, &hay(r)) {
                continue;
            }
            let better = match best {
                None => true,
                Some(cur) => {
                    compare_family(&state.live_records, i, cur) == std::cmp::Ordering::Less
                }
            };
            if better {
                best = Some(i);
            }
        }
        if best.is_some() {
            return best;
        }
    }
    None
}

/// Best live record for a session model key. Pure given the index.
pub fn match_record(state: &PricingState, model_key: &str) -> Option<usize> {
    let norm = normalize_key(model_key);
    if norm.is_empty() || state.live_records.is_empty() {
        return None;
    }
    let mut candidates = vec![norm.clone()];
    if let Some(slash) = norm.rfind('/') {
        if slash < norm.len() - 1 {
            candidates.push(norm[slash + 1..].to_string());
        }
    }
    // 1. Exact id match (with or without a provider prefix).
    for c in &candidates {
        if let Some(&hit) = state.live_by_id.get(c) {
            return Some(hit);
        }
    }
    // 2. Known id inside a versioned key ("claude-sonnet-4-5-20250929"):
    // longest id wins, ties prefer origin providers.
    for c in &candidates {
        let mut sub: Option<usize> = None;
        for (i, r) in state.live_records.iter().enumerate() {
            let id = r.model_id.to_lowercase();
            if !c.contains(&id) {
                continue;
            }
            let better = match sub {
                None => true,
                Some(cur) => {
                    id.len() > state.live_records[cur].model_id.len()
                        || (id.len() == state.live_records[cur].model_id.len()
                            && compare_records(&state.live_records, i, cur)
                                == std::cmp::Ordering::Less)
                }
            };
            if better {
                sub = Some(i);
            }
        }
        if let Some(s) = sub {
            return Some(s);
        }
    }
    // 3. Short key inside a known id ("gpt-4o" vs "gpt-4o-2024-08-06").
    for c in &candidates {
        if c.len() < 4 {
            continue;
        }
        let mut rev: Option<usize> = None;
        for (i, r) in state.live_records.iter().enumerate() {
            let id = r.model_id.to_lowercase();
            if !id.contains(c) {
                continue;
            }
            let better = match rev {
                None => true,
                Some(cur) => {
                    let ord = compare_records(&state.live_records, i, cur);
                    ord == std::cmp::Ordering::Less
                        || (ord == std::cmp::Ordering::Equal
                            && id.len() < state.live_records[cur].model_id.len())
                }
            };
            if better {
                rev = Some(i);
            }
        }
        if let Some(v) = rev {
            return Some(v);
        }
    }
    // 4. Family keyword match: find the key's own family first, then the best
    // live member of that same family (never another family's record).
    let key_family = FAMILY_DEFS
        .iter()
        .find(|f| f.id != "standard" && f.id != "free-tier" && family_matches(f.id, &norm));
    let key_family = key_family?;
    find_family_record(state, key_family.id)
}

// ---------------------------------------------------------------------------
// Overrides (in-memory index over the DB table)
// ---------------------------------------------------------------------------

fn reload_overrides_memory(state: &mut PricingState, rows: Vec<OverrideRow>) {
    state.custom_overrides = rows;
    state.custom_wildcard_by_model.clear();
    state.custom_specific_by_key.clear();
    state.custom_by_model.clear();
    for (i, row) in state.custom_overrides.iter().enumerate() {
        let model_lower = row.model_id.to_lowercase();
        let provider_lower = row.provider.to_lowercase();
        if model_lower.is_empty() {
            continue;
        }
        if provider_lower.is_empty() {
            state
                .custom_wildcard_by_model
                .entry(model_lower.clone())
                .or_insert(i);
        } else {
            state
                .custom_specific_by_key
                .insert(format!("{provider_lower}/{model_lower}"), i);
        }
        state
            .custom_by_model
            .entry(model_lower)
            .or_default()
            .push(i);
    }
    state.match_memo.clear();
}

fn to_public_override(row: &OverrideRow) -> PriceOverride {
    PriceOverride {
        provider: row.provider.clone(),
        model_id: row.model_id.clone(),
        display_name: if row.display_name.is_empty() {
            row.model_id.clone()
        } else {
            row.display_name.clone()
        },
        in_per_1m: row.in_per_1m,
        out_per_1m: row.out_per_1m,
        cache_per_1m: row.cache_per_1m,
        updated_at: row.updated_at.clone(),
    }
}

/// Precedence: provider-scoped exact match, then the all-providers wildcard,
/// then a lone provider-specific row for that model, then the most recently
/// updated row (deterministic).
pub fn find_custom_override(state: &PricingState, model_key: &str) -> Option<usize> {
    let norm = normalize_key(model_key);
    if norm.is_empty() || state.custom_overrides.is_empty() {
        return None;
    }
    let slash = norm.rfind('/');
    let bare = match slash {
        Some(s) if s < norm.len() - 1 => norm[s + 1..].to_string(),
        _ => norm.clone(),
    };
    if slash.map(|s| s > 0).unwrap_or(false) {
        if let Some(&i) = state.custom_specific_by_key.get(&norm) {
            return Some(i);
        }
    }
    if let Some(&i) = state.custom_wildcard_by_model.get(&bare) {
        return Some(i);
    }
    if bare != norm {
        if let Some(&i) = state.custom_wildcard_by_model.get(&norm) {
            return Some(i);
        }
    }
    let list: Vec<usize> = state
        .custom_by_model
        .get(&bare)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|&i| !state.custom_overrides[i].provider.trim().is_empty())
        .collect();
    if list.is_empty() {
        return None;
    }
    let mut sorted = list;
    sorted.sort_by(|&a, &b| {
        state.custom_overrides[b]
            .updated_at
            .cmp(&state.custom_overrides[a].updated_at)
    });
    sorted.into_iter().next()
}

/// Resolve pricing for a session model key: user override first, then the
/// live match, then the built-in fallback table.
pub fn get_model_pricing(state: &mut PricingState, model_key: &str) -> ModelPricing {
    let key = model_key.to_string();
    if let Some(i) = find_custom_override(state, &key) {
        let row = state.custom_overrides[i].clone();
        let label = if row.provider.is_empty() {
            row.model_id.clone()
        } else {
            format!("{}/{}", row.provider, row.model_id)
        };
        let name = if row.display_name.is_empty() {
            label.clone()
        } else {
            row.display_name.clone()
        };
        return ModelPricing {
            id: format!("custom:{label}"),
            display_name: name,
            in_per_1m: row.in_per_1m,
            out_per_1m: row.out_per_1m,
            cache_per_1m: row.cache_per_1m,
            source: PRICING_SOURCE_CUSTOM.to_string(),
            matched_model: Some(label),
            provider: if row.provider.is_empty() {
                None
            } else {
                Some(row.provider)
            },
        };
    }
    if family_matches("free-tier", &key) {
        return ModelPricing {
            id: "free-tier".into(),
            display_name: "Free / Local Models".into(),
            in_per_1m: 0.0,
            out_per_1m: 0.0,
            cache_per_1m: 0.0,
            source: "fallback".into(),
            matched_model: None,
            provider: None,
        };
    }
    if !state.live_records.is_empty() {
        let hit = match state.match_memo.get(&key) {
            Some(&cached) => cached,
            None => {
                let m = match_record(state, &key);
                state.match_memo.insert(key.clone(), m);
                m
            }
        };
        if let Some(hit) = hit {
            let r = state.live_records[hit].clone();
            return ModelPricing {
                id: format!("live:{}/{}", r.provider, r.model_id),
                display_name: r.name.clone(),
                in_per_1m: r.input,
                out_per_1m: r.output,
                cache_per_1m: r.cache_read,
                source: PRICING_SOURCE_LABEL.to_string(),
                matched_model: Some(format!("{}/{}", r.provider, r.model_id)),
                provider: Some(r.provider),
            };
        }
    }
    let fam = FAMILY_DEFS
        .iter()
        .find(|f| family_matches(f.id, &key))
        .unwrap_or(&FAMILY_DEFS[FAMILY_DEFS.len() - 1]);
    ModelPricing {
        id: fam.id.to_string(),
        display_name: fam.display_name.to_string(),
        in_per_1m: fam.in_per_1m,
        out_per_1m: fam.out_per_1m,
        cache_per_1m: fam.cache_per_1m,
        source: "fallback".to_string(),
        matched_model: None,
        provider: None,
    }
}

/// Prorate cached input tokens at the cache rate. Rounded to 4 decimals.
pub fn calculate_cost(
    state: &mut PricingState,
    model_key: &str,
    in_tokens: i64,
    out_tokens: i64,
    cached_tokens: i64,
) -> f64 {
    let p = get_model_pricing(state, model_key);
    let cached_input = (cached_tokens.max(0) as f64).min(in_tokens.max(0) as f64);
    let cost = ((in_tokens.max(0) as f64 - cached_input) * p.in_per_1m
        + (out_tokens.max(0) as f64) * p.out_per_1m
        + cached_input * p.cache_per_1m)
        / 1_000_000.0;
    (cost * 10_000.0).round() / 10_000.0
}

/// Estimate token counts from text length when `token_usage_json` is missing.
/// Input from user + 0.7×tool chars, output from
/// assistant chars, at [`TOKEN_CHARS_PER_TOKEN`] chars per token.
pub fn estimate_tokens_from_chars(
    user_chars: i64,
    assistant_chars: i64,
    tool_chars: i64,
) -> (i64, i64) {
    let input =
        ((user_chars as f64 + tool_chars as f64 * 0.7) / TOKEN_CHARS_PER_TOKEN).round() as i64;
    let output = (assistant_chars as f64 / TOKEN_CHARS_PER_TOKEN).round() as i64;
    (input.max(0), output.max(0))
}

/// Curated pricing matrix: one row per known family, resolved live when the
/// cache has a family member, otherwise the built-in fallback rate.
pub fn get_pricing_reference(state: &PricingState) -> Vec<PricingReferenceRow> {
    FAMILY_DEFS
        .iter()
        .map(|fam| {
            let live =
                if state.live_records.is_empty() || fam.id == "standard" || fam.id == "free-tier" {
                    None
                } else {
                    find_family_record(state, fam.id)
                };
            if let Some(idx) = live {
                let rec = &state.live_records[idx];
                let overridden =
                    find_custom_override(state, &format!("{}/{}", rec.provider, rec.model_id))
                        .or_else(|| find_custom_override(state, &rec.model_id));
                if let Some(oi) = overridden {
                    let row = &state.custom_overrides[oi];
                    let label = if row.provider.is_empty() {
                        row.model_id.clone()
                    } else {
                        format!("{}/{}", row.provider, row.model_id)
                    };
                    return PricingReferenceRow {
                        id: fam.id.to_string(),
                        name: fam.display_name.to_string(),
                        in_per_1m: row.in_per_1m,
                        out_per_1m: row.out_per_1m,
                        cache_per_1m: row.cache_per_1m,
                        source: PRICING_SOURCE_CUSTOM.to_string(),
                        matched_model: Some(label),
                        provider: Some(if row.provider.is_empty() {
                            rec.provider.clone()
                        } else {
                            row.provider.clone()
                        }),
                    };
                }
                return PricingReferenceRow {
                    id: fam.id.to_string(),
                    name: fam.display_name.to_string(),
                    in_per_1m: rec.input,
                    out_per_1m: rec.output,
                    cache_per_1m: rec.cache_read,
                    source: PRICING_SOURCE_LABEL.to_string(),
                    matched_model: Some(format!("{}/{}", rec.provider, rec.model_id)),
                    provider: Some(rec.provider.clone()),
                };
            }
            PricingReferenceRow {
                id: fam.id.to_string(),
                name: fam.display_name.to_string(),
                in_per_1m: fam.in_per_1m,
                out_per_1m: fam.out_per_1m,
                cache_per_1m: fam.cache_per_1m,
                source: "fallback".to_string(),
                matched_model: None,
                provider: None,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Cache persistence
// ---------------------------------------------------------------------------

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn parse_rfc3339_millis(text: &str) -> Option<i64> {
    use chrono::DateTime;
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

fn status_from_meta(state: &PricingState) -> PricingStatus {
    let stale = match &state.live_updated_at {
        None => true,
        Some(ts) => match parse_rfc3339_millis(ts) {
            Some(millis) => now_millis() - millis > PRICING_TTL_MS,
            None => true,
        },
    };
    PricingStatus {
        source: if state.live_records.is_empty() {
            "fallback".to_string()
        } else {
            PRICING_SOURCE_LABEL.to_string()
        },
        source_url: PRICING_SOURCE_URL.to_string(),
        updated_at: state.live_updated_at.clone(),
        model_count: state.live_records.len(),
        stale,
        ttl_hours: PRICING_TTL_MS as f64 / 3_600_000.0,
        override_count: state.custom_overrides.len(),
    }
}

fn read_override_rows(db: &Connection) -> Result<Vec<OverrideRow>, String> {
    let mut stmt = db.prepare("SELECT provider, model_id, display_name, in_per_1m, out_per_1m, cache_per_1m, updated_at FROM model_price_overrides")
        .map_err(|e| format!("Could not read price overrides: {e}"))?;
    let mapped = stmt
        .query_map([], |row| {
            Ok(OverrideRow {
                provider: row.get(0)?,
                model_id: row.get(1)?,
                display_name: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                in_per_1m: row.get(3)?,
                out_per_1m: row.get(4)?,
                cache_per_1m: row.get(5)?,
                updated_at: row.get(6)?,
            })
        })
        .map_err(|e| format!("Could not read price overrides: {e}"))?;
    mapped
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| format!("Could not decode price overrides: {e}"))
}

/// Load the cached payload into memory. Sync, local DB only — never touches
/// the network, so it is safe on every analytics query.
pub fn load_pricing_cache(
    db: &Connection,
    state: &mut PricingState,
) -> Result<PricingStatus, String> {
    let overrides = read_override_rows(db)?;
    let snapshot: Option<(String, i64, String)> = db
        .query_row(
            "SELECT payload_json, model_count, updated_at FROM model_pricing_cache WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|e| format!("Could not read the pricing cache: {e}"))?;
    let Some((payload_json, model_count, updated_at)) = snapshot else {
        reload_overrides_memory(state, overrides);
        set_live_index(state, Vec::new(), None);
        return Ok(status_from_meta(state));
    };
    // Skip re-parsing when this process already holds this snapshot.
    if state.live_updated_at.as_deref() == Some(updated_at.as_str())
        && model_count >= 0
        && state.live_records.len() == model_count as usize
    {
        reload_overrides_memory(state, overrides);
        return Ok(status_from_meta(state));
    }
    let raw: Value = serde_json::from_str(&payload_json)
        .map_err(|e| format!("Could not decode the pricing cache: {e}"))?;
    let (records, _) = build_pricing_index(&raw);
    if records.is_empty() {
        return Err(
            "The pricing cache contains no priced models. Sync prices to replace it.".into(),
        );
    }
    reload_overrides_memory(state, overrides);
    set_live_index(state, records, Some(updated_at));
    Ok(status_from_meta(state))
}

pub fn get_pricing_status(
    db: &Connection,
    state: &mut PricingState,
) -> Result<PricingStatus, String> {
    load_pricing_cache(db, state)
}

// ---------------------------------------------------------------------------
// Override CRUD (stored separately so models.dev syncs never clobber them)
// ---------------------------------------------------------------------------

/// List overrides ordered by provider then model id.
pub fn list_price_overrides(
    db: &Connection,
    state: &mut PricingState,
) -> Result<Vec<PriceOverride>, String> {
    load_pricing_cache(db, state)?;
    let mut rows: Vec<OverrideRow> = state.custom_overrides.clone();
    rows.sort_by(|a, b| {
        a.provider
            .cmp(&b.provider)
            .then_with(|| a.model_id.cmp(&b.model_id))
    });
    Ok(rows.iter().map(to_public_override).collect())
}

fn invalid_override_message(input: &Value) -> Option<String> {
    let model = input
        .get("modelId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if model.is_empty() {
        return Some("Invalid override: model id is required.".to_string());
    }
    if model.len() > 200 {
        return Some("Invalid override: model id is too long (max 200 chars).".to_string());
    }
    let provider = input
        .get("provider")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if provider.len() > 100 {
        return Some("Invalid override: provider is too long (max 100 chars).".to_string());
    }
    for (label, key, required) in [
        ("input price", "inPer1M", true),
        ("output price", "outPer1M", true),
        ("cache price", "cachePer1M", false),
    ] {
        let missing = match input.get(key) {
            None | Some(Value::Null) => true,
            Some(Value::String(s)) => s.is_empty(),
            _ => false,
        };
        let num = if missing {
            if required {
                f64::NAN
            } else {
                0.0
            }
        } else {
            input.get(key).and_then(|v| v.as_f64()).unwrap_or(f64::NAN)
        };
        if !num.is_finite() || num < 0.0 {
            return Some(format!("Invalid override: {label} must be a number >= 0."));
        }
        if num > 100000.0 {
            return Some(format!(
                "Invalid override: {label} looks unreasonable (max 100000)."
            ));
        }
    }
    None
}

/// Upsert by `(provider, modelId)`. Provider is lowercased; empty means
/// "all providers" (wildcard).
pub fn upsert_price_override(
    db: &Connection,
    state: &mut PricingState,
    input: &Value,
) -> Result<PriceOverride, String> {
    if let Some(err) = invalid_override_message(input) {
        return Err(err);
    }
    let provider = input
        .get("provider")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_lowercase();
    let model_id = input
        .get("modelId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let display_name = {
        let d = input
            .get("displayName")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if d.is_empty() {
            model_id.clone()
        } else {
            d
        }
    };
    let in_per_1m = input
        .get("inPer1M")
        .and_then(|v| v.as_f64())
        .unwrap_or(f64::NAN);
    let out_per_1m = input
        .get("outPer1M")
        .and_then(|v| v.as_f64())
        .unwrap_or(f64::NAN);
    let cache_per_1m = match input.get("cachePer1M") {
        None | Some(Value::Null) => 0.0,
        Some(Value::String(s)) if s.is_empty() => 0.0,
        Some(v) => v.as_f64().unwrap_or(0.0),
    };
    db.execute(
        "INSERT INTO model_price_overrides
           (provider, model_id, display_name, in_per_1m, out_per_1m, cache_per_1m, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, CURRENT_TIMESTAMP)
         ON CONFLICT(provider, model_id) DO UPDATE SET
           display_name = excluded.display_name, in_per_1m = excluded.in_per_1m,
           out_per_1m = excluded.out_per_1m, cache_per_1m = excluded.cache_per_1m,
           updated_at = CURRENT_TIMESTAMP",
        rusqlite::params![
            provider,
            model_id,
            display_name,
            in_per_1m,
            out_per_1m,
            cache_per_1m
        ],
    )
    .map_err(|e| format!("Could not save the price override: {e}"))?;
    load_pricing_cache(db, state)?;
    db.query_row(
        "SELECT provider, model_id, display_name, in_per_1m, out_per_1m, cache_per_1m, updated_at
         FROM model_price_overrides WHERE provider = ?1 AND model_id = ?2",
        rusqlite::params![provider, model_id],
        |row| {
            Ok(PriceOverride {
                provider: row.get(0)?,
                model_id: row.get(1)?,
                display_name: row
                    .get::<_, Option<String>>(2)?
                    .unwrap_or_else(|| model_id.clone()),
                in_per_1m: row.get(3)?,
                out_per_1m: row.get(4)?,
                cache_per_1m: row.get(5)?,
                updated_at: row.get(6)?,
            })
        },
    )
    .map_err(|e| format!("Could not read the price override: {e}"))
}

/// Reset scopes: `{provider, modelId}` -> one row; `{provider}` -> whole
/// provider; `{modelId}` -> that model across providers (case-insensitive);
/// `{}` -> all.
pub fn delete_price_overrides(
    db: &Connection,
    state: &mut PricingState,
    scope: &Value,
) -> Result<i64, String> {
    let provider = scope
        .get("provider")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_lowercase();
    let model_id = scope
        .get("modelId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let deleted = if !provider.is_empty() && !model_id.is_empty() {
        db.execute(
            "DELETE FROM model_price_overrides WHERE provider = ?1 AND model_id = ?2",
            rusqlite::params![provider, model_id],
        )
    } else if !provider.is_empty() {
        db.execute(
            "DELETE FROM model_price_overrides WHERE provider = ?1",
            [provider],
        )
    } else if !model_id.is_empty() {
        db.execute(
            "DELETE FROM model_price_overrides WHERE lower(model_id) = lower(?1)",
            [model_id],
        )
    } else {
        db.execute("DELETE FROM model_price_overrides", [])
    }
    .map_err(|e| format!("Could not reset the price overrides: {e}"))?;
    load_pricing_cache(db, state)?;
    Ok(deleted as i64)
}

// ---------------------------------------------------------------------------
// models.dev fetch / cache (24h TTL via reqwest)
// ---------------------------------------------------------------------------

pub fn save_pricing_payload(
    db: &Connection,
    state: &mut PricingState,
    raw: &Value,
    updated_at: &str,
) -> Result<(usize, String), String> {
    let (records, _) = build_pricing_index(raw);
    if records.is_empty() {
        return Err("models.dev payload contained no priced models.".to_string());
    }
    let count = records.len();
    db.execute(
        "INSERT INTO model_pricing_cache (id, payload_json, model_count, source_url, updated_at)
         VALUES (1, ?1, ?2, ?3, ?4)
         ON CONFLICT(id) DO UPDATE SET
           payload_json = excluded.payload_json, model_count = excluded.model_count,
           source_url = excluded.source_url, updated_at = excluded.updated_at",
        rusqlite::params![
            serde_json::to_string(raw).unwrap_or_default(),
            count as i64,
            PRICING_SOURCE_URL,
            updated_at,
        ],
    )
    .map_err(|e| format!("Could not save the pricing cache: {e}"))?;
    set_live_index(state, records, Some(updated_at.to_string()));
    Ok((count, updated_at.to_string()))
}

/// Fetch `https://models.dev/api.json` with an 8s timeout.
pub async fn fetch_models_dev_payload() -> Result<Value, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(PRICING_FETCH_TIMEOUT_MS))
        .build()
        .map_err(|e| format!("Could not create the HTTP client: {e}"))?;
    let res = client
        .get(PRICING_SOURCE_URL)
        .header("accept", "application/json")
        .send()
        .await
        .map_err(|e| format!("models.dev request failed: {e}"))?;
    let status = res.status();
    if !status.is_success() {
        return Err(format!("models.dev request failed: HTTP {status}"));
    }
    // NOTE: uses `.text()` + `serde_json::from_str` instead of
    // `res.json()` so no `reqwest/json` feature is required.
    let text = res
        .text()
        .await
        .map_err(|e| format!("Could not read the models.dev response: {e}"))?;
    serde_json::from_str::<Value>(&text)
        .map_err(|e| format!("Could not parse the models.dev response: {e}"))
}
