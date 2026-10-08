//! Audit prompts (spec `market/auditor-agent` "审计 prompt"; design D8 of m6-auditor-agent): a
//! built-in generator that combines task types, topics and phrasings at random, and an optional
//! bank of operator-supplied conversations mixed in at a configured ratio. Nothing marks a
//! prompt as an audit.
//!
//! Only prompts whose token count is in the thresholds' audit length band are sent
//! (m6-toploc-gpu-calibration design D9): bank entries are screened per model at start-up, and a
//! generated prompt below the band is lengthened with background sentences drawn at random
//! ([`lengthen`]); the agent counts with its re-check engine.

use std::collections::BTreeMap;
use std::path::Path;

use ac_market_proto::toploc::PromptBand;
use ac_primitives::market::ModelId;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use super::rand::Rand;

/// Bounds of the output token limit a request asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputRange {
    /// Smallest `max_tokens`.
    pub min: u32,
    /// Largest `max_tokens`.
    pub max: u32,
}

impl Default for OutputRange {
    fn default() -> Self {
        Self { min: 32, max: 256 }
    }
}

const TOPICS: &[&str] = &[
    "tidal energy",
    "medieval trade routes",
    "the water cycle",
    "sourdough baking",
    "volcanic islands",
    "public libraries",
    "migratory birds",
    "the history of maps",
    "urban gardening",
    "glaciers",
    "jazz improvisation",
    "solar sails",
    "coral reefs",
    "early printing presses",
    "desert ecosystems",
    "chess openings",
    "railway timetables",
    "bee colonies",
    "lighthouses",
    "tea ceremonies",
    "suspension bridges",
    "deep-sea vents",
    "folk music",
    "weather balloons",
    "paper making",
    "river deltas",
    "night skies",
    "board game design",
    "the immune system",
    "wind turbines",
    "ancient calendars",
    "mountain huts",
];

const TASKS: &[&str] = &[
    "Explain {topic} to a curious {age}-year-old.",
    "Write a short poem about {topic}.",
    "List {n} surprising facts about {topic}.",
    "Summarize the main ideas of {topic} in {n} sentences.",
    "Give {name} {n} practical tips related to {topic}.",
    "Describe a day in the life of someone who works with {topic}.",
    "What are common misconceptions about {topic}?",
    "Compare {topic} with {other} in a few sentences.",
    "Write a dialogue between {name} and a friend about {topic}.",
    "Draft a short blog introduction about {topic}, dated {date}.",
    "Suggest {n} questions a student might ask about {topic}.",
    "Explain how {topic} might change by the year {year}.",
    "Write a haiku sequence ({n} haiku) on {topic}.",
    "Translate this sentence into French and Spanish: \"{name} studies {topic}.\"",
    "In Python, write a small function that returns {n} facts about {topic} as a list.",
    "If a town had {n} hundred people, how might {topic} affect them? Reason step by step.",
];

const OPENERS: &[&str] = &[
    "",
    "Hi! ",
    "Quick question. ",
    "I'm {name} and I'm curious. ",
    "For a school project: ",
    "My friend {name} asked me this. ",
    "Please help. ",
    "Out of interest: ",
    "I'm preparing a talk for {n}0 people. ",
    "Hello, ",
];

const STYLES: &[&str] = &[
    "",
    " Keep it under {n}0 words.",
    " Use simple language.",
    " Be specific.",
    " Answer in {n} short paragraphs.",
    " Add one example.",
    " Avoid jargon.",
    " Use bullet points.",
    " Make it fun for {name}.",
    " Mention the year {year}.",
];

const SYSTEMS: &[&str] = &[
    "You are a helpful assistant.",
    "You are a concise assistant. Keep answers short.",
    "You answer like a friendly teacher.",
    "You are an assistant that writes clearly and avoids jargon.",
];

const FOLLOW_UPS: &[&str] = &[
    "Can you make it shorter?",
    "Now add one more example.",
    "Explain that last point differently.",
    "Thanks! What should I read next about it?",
    "Could you turn that into a numbered list?",
];

// Background sentences that lengthen a prompt: each one filled at random like the tasks, so no
// text recurs across audit prompts (spec "审计 prompt": nothing repeated to make up length).
const BACKGROUND: &[&str] = &[
    "Last spring {name} read {n} books about {topic}.",
    "A museum near {name}'s home opened an exhibit on {topic} in {year}.",
    "Our club meets every {n} weeks and we often end up discussing {other}.",
    "My cousin, who is {age}, keeps asking me about {topic}.",
    "I once watched a documentary that connected {topic} and {other}.",
    "The local paper ran a story on {other} dated {date}.",
    "{name} says that {topic} is underrated, and I partly agree.",
    "We visited a place famous for {other} about {n} years ago.",
    "In class we spent {n} lessons on {topic}, but it went by quickly.",
    "There was a heated debate about {other} at dinner yesterday.",
    "I keep a notebook with {n}0 pages of notes on {topic}.",
    "A friend of mine, {name}, works on projects involving {other}.",
    "Some people I know think {topic} will matter more by {year}.",
    "Our neighbour {name} started a small hobby around {other}.",
    "I tried explaining {topic} to {name} and got confused myself.",
    "The library has a shelf on {other} that I have not finished.",
    "On a trip in {year} I saw a talk where {topic} came up {n} times.",
    "My grandmother remembers when {other} was a new idea.",
    "I am writing an essay of about {n}00 words that touches on {topic}.",
    "{name} and I disagree about how {other} relates to everyday life.",
    "A podcast episode from {date} mentioned {topic} in passing.",
    "Back in school I made a poster about {other} with {n} drawings.",
    "Lately I have been reading forum threads about {topic}.",
    "Someone at work, {name}, brought up {other} over lunch.",
];

const LEADS: &[&str] = &[
    "",
    "Some context: ",
    "A bit about me: ",
    "Background: ",
    "First, ",
    "For what it's worth, ",
    "To explain why I ask: ",
];

const SYLLABLES: &[&str] = &[
    "ka", "lo", "mi", "ren", "sa", "tor", "vi", "el", "na", "dor", "lin", "ma", "ra", "zu", "ket",
    "an",
];

fn pick<'a>(rng: &mut dyn Rand, items: &'a [&'a str]) -> &'a str {
    let i = usize::try_from(rng.below(items.len() as u64)).unwrap_or(0);
    items.get(i).copied().unwrap_or_default()
}

fn name(rng: &mut dyn Rand) -> String {
    let parts = 2 + rng.below(2);
    let mut s = String::new();
    for _ in 0..parts {
        s.push_str(pick(rng, SYLLABLES));
    }
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or(s)
}

fn fill(rng: &mut dyn Rand, template: &str) -> String {
    let topic = pick(rng, TOPICS);
    let mut other = pick(rng, TOPICS);
    if other == topic {
        other = pick(rng, TOPICS);
    }
    let date = format!(
        "{}-{:02}-{:02}",
        2020 + rng.below(10),
        1 + rng.below(12),
        1 + rng.below(28)
    );
    template
        .replace("{topic}", topic)
        .replace("{other}", other)
        .replace("{age}", &(6 + rng.below(10)).to_string())
        .replace("{n}", &(2 + rng.below(6)).to_string())
        .replace("{name}", &name(rng))
        .replace("{date}", &date)
        .replace("{year}", &(2030 + rng.below(40)).to_string())
}

/// One generated conversation (Chat Completions `messages`).
#[must_use]
pub fn generate(rng: &mut dyn Rand) -> Value {
    let mut messages = Vec::new();
    if rng.below(2) == 0 {
        messages.push(json!({"role": "system", "content": pick(rng, SYSTEMS)}));
    }
    let template = format!(
        "{}{}{}",
        pick(rng, OPENERS),
        pick(rng, TASKS),
        pick(rng, STYLES)
    );
    messages.push(json!({"role": "user", "content": fill(rng, &template)}));
    // Sometimes a short earlier exchange, so that conversations differ in length and shape.
    if rng.below(3) == 0 {
        let template = pick(rng, TASKS);
        let earlier = fill(rng, template);
        let reply = format!("Here is a brief answer about {}.", pick(rng, TOPICS));
        messages.push(json!({"role": "assistant", "content": reply}));
        messages.push(
            json!({"role": "user", "content": format!("{} {earlier}", pick(rng, FOLLOW_UPS))}),
        );
    }
    Value::Array(messages)
}

/// Words of a conversation's texts: the yardstick [`lengthen`] adds by.
#[must_use]
pub fn words(messages: &Value) -> usize {
    messages.as_array().map_or(0, |a| {
        a.iter()
            .filter_map(|m| m.get("content").and_then(Value::as_str))
            .map(|c| c.split_whitespace().count())
            .sum()
    })
}

/// Puts random background sentences of at least `words` words in front of the last user
/// message (design D9), behind a lead-in drawn at random.
pub fn lengthen(messages: &mut Value, rng: &mut dyn Rand, words: usize) {
    let mut background = pick(rng, LEADS).to_owned();
    let mut added = 0;
    while added < words {
        let template = pick(rng, BACKGROUND);
        let sentence = fill(rng, template);
        added = added.saturating_add(sentence.split_whitespace().count());
        if !background.is_empty() && !background.ends_with(' ') {
            background.push(' ');
        }
        background.push_str(&sentence);
    }
    let last_user = messages.as_array_mut().and_then(|a| {
        a.iter_mut()
            .rev()
            .find(|m| m.get("role").and_then(Value::as_str) == Some("user"))
    });
    if let Some(m) = last_user {
        let content = m.get("content").and_then(Value::as_str).unwrap_or_default();
        let joined = format!("{background}\n\n{content}");
        if let Some(c) = m.get_mut("content") {
            *c = Value::String(joined);
        }
    }
}

/// The next prompt and where it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct Draw {
    /// Chat Completions `messages`.
    pub messages: Value,
    /// The `max_tokens` to ask for.
    pub max_tokens: u32,
    /// For a bank entry, its prompt token count (screened at start-up); `None` for a generated
    /// prompt, which the caller counts.
    pub tokens: Option<u32>,
}

/// Where prompts come from.
#[derive(Clone, Debug, Default)]
pub struct Prompts {
    bank: Vec<Value>,
    /// Share of bank prompts in percent (0–100).
    bank_percent: u8,
    output: OutputRange,
    /// Per model, the bank entries in the audit length band and their prompt token counts
    /// (spec "题库条目不在区间内").
    in_band: BTreeMap<ModelId, Vec<(usize, u32)>>,
}

impl Prompts {
    /// The generator alone.
    #[must_use]
    pub fn generator(output: OutputRange) -> Self {
        Self {
            bank: Vec::new(),
            bank_percent: 0,
            output,
            in_band: BTreeMap::new(),
        }
    }

    /// Mixes in a bank file (JSONL, one `messages` array per line) at `bank_percent`.
    ///
    /// # Errors
    ///
    /// An unreadable file, a line that is not a non-empty array of messages with string
    /// `role` and `content`, an empty bank, or a share above 100.
    pub fn with_bank(mut self, path: &Path, bank_percent: u8) -> Result<Self> {
        if bank_percent > 100 {
            bail!("the bank share is a percentage (0–100)");
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading the prompt bank {}", path.display()))?;
        let mut bank = Vec::new();
        for (i, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let v: Value = serde_json::from_str(line)
                .with_context(|| format!("prompt bank line {}: not JSON", i + 1))?;
            if !valid_messages(&v) {
                bail!(
                    "prompt bank line {}: not a list of messages with role and content",
                    i + 1
                );
            }
            bank.push(v);
        }
        if bank.is_empty() {
            bail!("the prompt bank {} is empty", path.display());
        }
        self.bank = bank;
        self.bank_percent = bank_percent;
        Ok(self)
    }

    /// The bank's conversations.
    #[must_use]
    pub fn bank(&self) -> &[Value] {
        &self.bank
    }

    /// Keeps, for `model`, the bank entries whose prompt token counts (`counts`, in bank order)
    /// are in `band`; returns how many were skipped. Only kept entries are ever drawn for the
    /// model.
    ///
    /// # Errors
    ///
    /// A non-empty bank with no entry in the band (the agent then refuses to start), or counts
    /// that do not match the bank.
    pub fn screen(&mut self, model: ModelId, counts: &[u32], band: PromptBand) -> Result<usize> {
        if counts.len() != self.bank.len() {
            bail!(
                "{} counts for {} bank entries",
                counts.len(),
                self.bank.len()
            );
        }
        let kept: Vec<(usize, u32)> = counts
            .iter()
            .copied()
            .enumerate()
            .filter(|(_, c)| band.contains(*c))
            .collect();
        if kept.is_empty() && !self.bank.is_empty() {
            bail!(
                "no prompt of the bank has {}–{} prompt tokens (the audit length band) for model 0x{}",
                band.min,
                band.max,
                hex::encode(model.0)
            );
        }
        let skipped = self.bank.len().saturating_sub(kept.len());
        self.in_band.insert(model, kept);
        Ok(skipped)
    }

    /// The next conversation for `model`: a bank entry in the band for it (at the bank's share,
    /// when it has any) or a generated one, which the caller fits into the band.
    pub fn next(&self, rng: &mut dyn Rand, model: ModelId) -> Draw {
        let in_band = self
            .in_band
            .get(&model)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let from_bank = !in_band.is_empty() && rng.below(100) < u64::from(self.bank_percent);
        let bank_entry = if from_bank {
            let i = usize::try_from(rng.below(in_band.len() as u64)).unwrap_or(0);
            in_band
                .get(i)
                .and_then(|(i, n)| self.bank.get(*i).map(|m| (m.clone(), *n)))
        } else {
            None
        };
        let (messages, tokens) = match bank_entry {
            Some((m, n)) => (m, Some(n)),
            None => (generate(rng), None),
        };
        let span = u64::from(self.output.max.saturating_sub(self.output.min)) + 1;
        let extra = u32::try_from(rng.below(span)).unwrap_or(0);
        Draw {
            messages,
            max_tokens: self.output.min.saturating_add(extra),
            tokens,
        }
    }
}

fn valid_messages(v: &Value) -> bool {
    v.as_array().is_some_and(|a| {
        !a.is_empty()
            && a.iter().all(|m| {
                m.get("role").and_then(Value::as_str).is_some()
                    && m.get("content").and_then(Value::as_str).is_some()
            })
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)] // Test code.

    use std::collections::BTreeSet;

    use super::*;
    use crate::agent::rand::tests::Seeded;

    fn texts(v: &Value) -> String {
        v.as_array()
            .unwrap()
            .iter()
            .map(|m| m["content"].as_str().unwrap())
            .collect::<Vec<_>>()
            .join("\n")
    }

    // Spec "审计 prompt" / "prompt 多样": 1,000 prompts, at least 990 distinct, no substring of
    // 16 or more characters common to all of them.
    #[test]
    fn prompts_are_diverse() {
        let mut rng = Seeded::new(7);
        let all: Vec<String> = (0..1_000).map(|_| texts(&generate(&mut rng))).collect();
        let distinct: BTreeSet<&String> = all.iter().collect();
        assert!(distinct.len() >= 990, "{} distinct", distinct.len());
        // Every 16-character window of the first prompt must be missing from some prompt.
        let first: Vec<char> = all[0].chars().collect();
        for w in first.windows(16) {
            let s: String = w.iter().collect();
            assert!(
                all.iter().any(|p| !p.contains(&s)),
                "{s:?} is in every prompt"
            );
        }
    }

    #[test]
    fn output_limits_stay_in_range_and_the_bank_is_mixed_in() {
        let dir = std::env::temp_dir().join(format!("ac-auditor-bank-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bank = dir.join("bank.jsonl");
        std::fs::write(
            &bank,
            "[{\"role\":\"user\",\"content\":\"bank-only question\"}]\n\n",
        )
        .unwrap();
        let mut p = Prompts::generator(OutputRange { min: 10, max: 20 })
            .with_bank(&bank, 50)
            .unwrap();
        let model = ModelId([1; 32]);
        p.screen(model, &[5], PromptBand { min: 1, max: 9 })
            .unwrap();
        let mut rng = Seeded::new(1);
        let mut from_bank = 0;
        for _ in 0..400 {
            let d = p.next(&mut rng, model);
            assert!((10..=20).contains(&d.max_tokens));
            if texts(&d.messages) == "bank-only question" {
                assert_eq!(d.tokens, Some(5));
                from_bank += 1;
            }
        }
        assert!((120..=280).contains(&from_bank), "{from_bank}");
        // Another model, for which the bank was not screened, gets generated prompts only.
        assert!((0..50).all(|_| p.next(&mut rng, ModelId([2; 32])).tokens.is_none()));
    }

    // Spec "审计 prompt" / "题库条目不在区间内": entries outside the band are never drawn and
    // are counted; a bank with none in the band is refused.
    #[test]
    fn bank_entries_outside_the_band_are_skipped() {
        let dir = std::env::temp_dir().join(format!("ac-auditor-band-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bank = dir.join("bank.jsonl");
        let lines: Vec<String> = (0..4)
            .map(|i| json!([{"role": "user", "content": format!("entry {i}")}]).to_string())
            .collect();
        std::fs::write(&bank, lines.join("\n")).unwrap();
        let band = PromptBand { min: 10, max: 20 };
        let model = ModelId([1; 32]);
        let mut p = Prompts::generator(OutputRange::default())
            .with_bank(&bank, 100)
            .unwrap();
        assert_eq!(p.screen(model, &[9, 10, 20, 21], band).unwrap(), 2);
        let mut rng = Seeded::new(3);
        let drawn: BTreeSet<String> = (0..200)
            .map(|_| texts(&p.next(&mut rng, model).messages))
            .collect();
        assert_eq!(
            drawn,
            BTreeSet::from(["entry 1".to_owned(), "entry 2".to_owned()])
        );
        assert!(p.screen(model, &[9, 21, 300, 0], band).is_err());
        assert!(p.screen(model, &[10], band).is_err());
    }

    #[test]
    fn lengthening_adds_background_before_the_last_question() {
        let mut rng = Seeded::new(5);
        for _ in 0..100 {
            let mut m = generate(&mut rng);
            let before = words(&m);
            let question = m.as_array().unwrap().last().unwrap()["content"]
                .as_str()
                .unwrap()
                .to_owned();
            lengthen(&mut m, &mut rng, 150);
            assert!(words(&m) >= before + 150);
            let last = m.as_array().unwrap().last().unwrap()["content"]
                .as_str()
                .unwrap()
                .to_owned();
            assert!(last.ends_with(&question), "{last}");
        }
    }

    // Spec "审计 prompt" / "题库无效".
    #[test]
    fn bad_banks_are_refused() {
        let dir = std::env::temp_dir().join(format!("ac-auditor-badbank-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (i, text) in ["not json\n", "{\"role\":\"user\"}\n", "[]\n", "\n"]
            .iter()
            .enumerate()
        {
            let f = dir.join(format!("{i}.jsonl"));
            std::fs::write(&f, text).unwrap();
            assert!(Prompts::default().with_bank(&f, 10).is_err(), "{text:?}");
        }
        assert!(
            Prompts::default()
                .with_bank(&dir.join("missing.jsonl"), 10)
                .is_err()
        );
    }
}
