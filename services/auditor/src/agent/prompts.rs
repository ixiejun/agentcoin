//! Audit prompts (spec `market/auditor-agent` "审计 prompt"; design D8 of m6-auditor-agent): a
//! built-in generator that combines task types, topics and phrasings at random, and an optional
//! bank of operator-supplied conversations mixed in at a configured ratio. Nothing marks a
//! prompt as an audit.

use std::path::Path;

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

/// Where prompts come from.
#[derive(Clone, Debug, Default)]
pub struct Prompts {
    bank: Vec<Value>,
    /// Share of bank prompts in percent (0–100).
    bank_percent: u8,
    output: OutputRange,
}

impl Prompts {
    /// The generator alone.
    #[must_use]
    pub fn generator(output: OutputRange) -> Self {
        Self {
            bank: Vec::new(),
            bank_percent: 0,
            output,
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

    /// The next conversation and the `max_tokens` to ask for.
    pub fn next(&self, rng: &mut dyn Rand) -> (Value, u32) {
        let from_bank = !self.bank.is_empty() && rng.below(100) < u64::from(self.bank_percent);
        let messages = if from_bank {
            let i = usize::try_from(rng.below(self.bank.len() as u64)).unwrap_or(0);
            self.bank.get(i).cloned().unwrap_or_else(|| generate(rng))
        } else {
            generate(rng)
        };
        let span = u64::from(self.output.max.saturating_sub(self.output.min)) + 1;
        let extra = u32::try_from(rng.below(span)).unwrap_or(0);
        (messages, self.output.min.saturating_add(extra))
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
        let p = Prompts::generator(OutputRange { min: 10, max: 20 })
            .with_bank(&bank, 50)
            .unwrap();
        let mut rng = Seeded::new(1);
        let mut from_bank = 0;
        for _ in 0..400 {
            let (m, max) = p.next(&mut rng);
            assert!((10..=20).contains(&max));
            if texts(&m) == "bank-only question" {
                from_bank += 1;
            }
        }
        assert!((120..=280).contains(&from_bank), "{from_bank}");
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
