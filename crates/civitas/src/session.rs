//! **One person's day, as a conversation.**
//!
//! A person's context is the whole of their life so far: every day they have
//! had, exactly as it went, rebuilt from the tape on a resume. A day opens with
//! the note of what happened since, the model calls tools until it ends the
//! day or runs out of calls, and every act is checked as it is made and kept.
//! A day whose call fails is a silence: none of its acts apply and none of its
//! words enter the person's life.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::acts::{Act, ActResult};
use crate::config::RunConfig;
use crate::event::Usage;
use crate::model::{CallError, Model, Request};
use crate::prompt::{self, Knowledge};
use crate::tools::{self, Day};
use crate::world::{World, CONTEXT_FULL};

pub enum Outcome {
    /// The day happened: what it added to the person's conversation, the acts
    /// to apply, and the acts its checks refused.
    Lived { messages: Vec<Value>, acts: Vec<Act>, refused: Vec<(Act, ActResult)>, usage: Usage },
    Silent {
        reason: String,
        usage: Usage,
        /// The silence was the endpoint's — a limit or an overload that
        /// outlasted every wait — and not the person's or the conversation's,
        /// so the day may be lived again later in the tick.
        weather: bool,
    },
}

/// **A life as it is sent: at least the last `kept` days whole, and every
/// day before them as the person kept it.** The tape and the life on disk
/// hold every word; this is read off them at the moment of the call, so a
/// resume sends the same conversation a sitting would have.
///
/// A day older than the window becomes one turn of the person's own: what they
/// wrote in their diary that day, what they said on the square, and each thing
/// they did with what it came to — the memory of a day, in their words and the
/// wallet's short answers, and nothing a model wrote about them afterwards.
/// Zero for `kept` is every day whole.
///
/// **Days are folded in blocks of `kept`, not one a day.** A window that
/// slides by one day every day rewrites the prompt at the point it slid, and
/// everything after that point — the whole days behind today — is sent at
/// the fresh rate again: pilot-6's median person-day sent 5,600 fresh tokens
/// whatever the size of the life, and fresh input was $36 of its $58. Folded
/// a block at a time the prefix stands for `kept` days, and a day sends what
/// it adds. Between blocks a person carries up to twice `kept` days whole,
/// which the cache reads.
///
/// A day begins at the note that opens it, which is the one user turn whose
/// text starts "Day N." — a tool result never does — so the boundaries are
/// read off the life itself and the tape needs no marker.
pub fn compact(life: &[Value], kept: u32) -> Vec<Value> {
    if kept == 0 {
        return life.to_vec();
    }
    let starts: Vec<usize> = life
        .iter()
        .enumerate()
        .filter(|(_, m)| opens_a_day(m))
        .map(|(i, _)| i)
        .collect();
    let kept = kept as usize;
    if starts.len() <= kept {
        return life.to_vec();
    }
    let folded = (starts.len() - kept) / kept * kept;
    if folded == 0 {
        return life.to_vec();
    }
    let whole_from = starts[folded];
    let mut out = Vec::with_capacity(life.len());
    for (n, &at) in starts[..folded].iter().enumerate() {
        let end = starts.get(n + 1).copied().unwrap_or(whole_from);
        out.push(json!({ "role": "user", "content": [{ "type": "text", "text": remembered(&life[at..end]) }] }));
    }
    out.extend_from_slice(&life[whole_from..]);
    out
}

fn opens_a_day(m: &Value) -> bool {
    m["role"] == "user"
        && m["content"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|b| b["type"] == "text" && b["text"].as_str().is_some_and(|t| t.starts_with("Day ")))
}

/// How much of a said thing a memory keeps.
const REMEMBERED_WORDS: usize = 400;
const REMEMBERED_OUTCOME: usize = 160;

/// One day's messages as the line-per-thing memory of them.
fn remembered(day: &[Value]) -> String {
    let heading = day
        .first()
        .and_then(|m| m["content"].as_array())
        .and_then(|a| a.iter().find_map(|b| b["text"].as_str()))
        .and_then(|t| t.lines().next())
        .unwrap_or("A day")
        .trim_end_matches('.')
        .to_string();
    let mut uses: BTreeMap<String, (String, Value)> = BTreeMap::new();
    let mut lines: Vec<String> = Vec::new();
    for m in day {
        for b in m["content"].as_array().into_iter().flatten() {
            match b["type"].as_str() {
                Some("tool_use") => {
                    let name = b["name"].as_str().unwrap_or_default().to_string();
                    uses.insert(b["id"].to_string(), (name, b["input"].clone()));
                }
                Some("tool_result") => {
                    let Some((name, input)) = uses.get(&b["tool_use_id"].to_string()) else { continue };
                    if tools::READS.contains(&name.as_str()) || name == "end_day" || b["is_error"] == true {
                        continue;
                    }
                    let text = b["content"].as_str().unwrap_or_default();
                    if text == tools::DAY_OVER {
                        continue;
                    }
                    let said = |k: &str| input[k].as_str().unwrap_or_default();
                    lines.push(match name.as_str() {
                        "diary" => {
                            format!("- you wrote in your diary: \"{}\"", prompt::cut(said("text"), REMEMBERED_WORDS))
                        }
                        "post" => {
                            format!("- you said on the square: \"{}\"", prompt::cut(said("text"), REMEMBERED_WORDS))
                        }
                        "message" => format!(
                            "- you wrote to {}: \"{}\"",
                            said("to"),
                            prompt::cut(said("text"), REMEMBERED_WORDS)
                        ),
                        _ => format!(
                            "- {name} {} → {}",
                            input,
                            prompt::cut(text.split(" It takes effect").next().unwrap_or(text), REMEMBERED_OUTCOME)
                        ),
                    });
                }
                _ => {}
            }
        }
    }
    if lines.is_empty() {
        return format!("{heading}, as you remember it: you looked at your wallet and did nothing.");
    }
    format!("{heading}, as you remember it:\n{}", lines.join("\n"))
}

/// What a person keeps of a day that is over.
///
/// Their own words and their own acts stay exactly as they were; the wallet's
/// long answers become the line a memory of them would be — what they looked
/// at and what it came to. A person can always look again, and the answer they
/// get is today's rather than a stale one.
///
/// What is folded is a VIEW, which is why the test is whether the answer is
/// one — a tree the wallet drew — and not how long it is. An act's outcome and
/// a refusal are prose and are events: five of the wallet's own refusal
/// sentences run past two hundred characters by themselves, and a rule on
/// length alone turned the reason a person's act was refused into "you looked
/// at offer_credit that day", which they cannot look at again.
///
/// Folded when the day CLOSES and never again, so the prefix of a life never
/// changes under the cache. The tape keeps every word either way.
pub fn folded(messages: &[Value]) -> Vec<Value> {
    const KEPT_WHOLE: usize = 240;
    let mut asked: BTreeMap<String, String> = BTreeMap::new();
    for m in messages {
        for b in m["content"].as_array().into_iter().flatten() {
            if b["type"] == "tool_use" {
                if let (Some(id), Some(name)) = (b["id"].as_str(), b["name"].as_str()) {
                    asked.insert(id.to_string(), name.to_string());
                }
            }
        }
    }
    let mut out = messages.to_vec();
    for m in &mut out {
        for b in m["content"].as_array_mut().into_iter().flatten() {
            if b["type"] != "tool_result" {
                continue;
            }
            let Some(text) = b["content"].as_str() else { continue };
            if text.len() <= KEPT_WHOLE {
                continue;
            }
            let what = b["tool_use_id"]
                .as_str()
                .and_then(|id| asked.get(id))
                .cloned()
                .unwrap_or_default();
            if let Some(line) = stands_for(&what, text) {
                b["content"] = json!(line);
            }
        }
    }
    out
}

/// The line a long view leaves behind: what was asked, and the shape of what
/// came back, so a person knows they looked and knows to look again. `None`
/// for an answer that is not a view: prose is kept as it was said.
fn stands_for(what: &str, text: &str) -> Option<String> {
    let shape = match serde_json::from_str::<Value>(text).ok()? {
        Value::Object(map) => map
            .iter()
            .map(|(k, v)| match v {
                Value::Array(xs) => format!("{k} {}", xs.len()),
                Value::Object(_) => k.clone(),
                other => format!("{k} {other}"),
            })
            .collect::<Vec<_>>()
            .join(", "),
        Value::Array(xs) => format!("{} of them", xs.len()),
        _ => return None,
    };
    let shape: String = shape.chars().take(200).collect();
    Some(format!("(you looked at {what} that day: {shape}. Look again if you need what it says now.)"))
}

/// Merge consecutive messages of one role into one, as the API reads a
/// conversation: a day's note follows the previous day's closing tool results
/// in the same user turn.
pub fn merged(messages: &[Value]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::with_capacity(messages.len());
    for m in messages {
        let content = match &m["content"] {
            Value::String(s) => vec![json!({ "type": "text", "text": s })],
            Value::Array(a) => a.clone(),
            _ => Vec::new(),
        };
        match out.last_mut() {
            Some(prev) if prev["role"] == m["role"] => {
                if let Some(a) = prev["content"].as_array_mut() {
                    a.extend(content);
                }
            }
            _ => out.push(json!({ "role": m["role"], "content": content })),
        }
    }
    out
}

fn mark(block: &mut Value, ttl: &str) {
    block["cache_control"] = json!({ "type": "ephemeral", "ttl": ttl });
}

/// The request's conversation with cache breakpoints on the day's opening and
/// on the newest message, so the life before today and the day so far are both
/// read from the cache.
fn with_breakpoints(messages: &[Value], opening: usize, ttl: &str) -> Vec<Value> {
    let mut out = merged(messages);
    let opening_at = merged(&messages[..=opening.min(messages.len().saturating_sub(1))])
        .len()
        .saturating_sub(1);
    let last = out.len().saturating_sub(1);
    for i in [opening_at, last] {
        if let Some(block) = out
            .get_mut(i)
            .and_then(|m| m["content"].as_array_mut())
            .and_then(|a| a.last_mut())
        {
            mark(block, ttl);
        }
    }
    out
}

pub fn system_for(cfg: &RunConfig, k: &Knowledge, world: &World, person: usize) -> Vec<Value> {
    let p = &world.persons[person];
    let mut blocks =
        prompt::system(k, p.card.as_deref().unwrap_or_default(), p.tier.unwrap_or(cfg.default_tier), cfg.trust.wary);
    if let Some(last) = blocks.last_mut() {
        mark(last, &cfg.model.cache_ttl);
    }
    blocks
}

#[allow(clippy::too_many_arguments)]
pub fn live_day(
    model: &mut dyn Model,
    cfg: &RunConfig,
    k: &Knowledge,
    world: &World,
    life: &[Value],
    person: usize,
    tick: u64,
    notified: bool,
) -> Outcome {
    let system = system_for(cfg, k, world, person);
    let tools = tools::definitions();
    let mut conversation: Vec<Value> = compact(life, cfg.days_kept_whole);
    let mut added: Vec<Value> = Vec::new();
    let note = if notified { prompt::notified_note(world, person) } else { prompt::day_note(world, person) };
    let opening = json!({ "role": "user", "content": [{ "type": "text", "text": note }] });
    conversation.push(opening.clone());
    added.push(opening);
    let opening_at = conversation.len() - 1;
    let mut day = Day::new(world, &k.errors, person, tick);
    let mut usage = Usage::default();

    for call in 0..=cfg.model.max_calls_per_turn {
        let messages = with_breakpoints(&conversation, opening_at, &cfg.model.cache_ttl);
        let req = Request {
            system: &system,
            messages: &messages,
            tools: &tools,
            max_tokens: cfg.model.max_tokens,
            tool_choice: None,
        };
        let resp = match model.call(&req) {
            Ok(r) => r,
            Err(CallError::ContextFull(m)) => {
                return Outcome::Silent { reason: format!("{CONTEXT_FULL}: {m}"), usage, weather: false }
            }
            Err(CallError::Weather(m)) => return Outcome::Silent { reason: m, usage, weather: true },
            Err(CallError::Failed(m)) => return Outcome::Silent { reason: m, usage, weather: false },
        };
        usage.add(&resp.usage);
        let reply = json!({ "role": "assistant", "content": resp.content });
        conversation.push(reply.clone());
        added.push(reply);
        let uses: Vec<Value> = resp.content.iter().filter(|b| b["type"] == "tool_use").cloned().collect();
        if uses.is_empty() {
            if resp.stop_reason == "max_tokens" {
                return Outcome::Silent { reason: "the reply ran past its token limit".into(), usage, weather: false };
            }
            break;
        }
        let out_of_calls = call == cfg.model.max_calls_per_turn;
        let results: Vec<Value> = uses
            .iter()
            .map(|u| {
                let id = u["id"].clone();
                let name = u["name"].as_str().unwrap_or_default();
                let (text, is_error) = if day.ended || out_of_calls {
                    (tools::DAY_OVER.to_string(), false)
                } else {
                    let a = day.call(name, &u["input"]);
                    (a.text, a.is_error)
                };
                json!({ "type": "tool_result", "tool_use_id": id, "content": text, "is_error": is_error })
            })
            .collect();
        let answer = json!({ "role": "user", "content": results });
        conversation.push(answer.clone());
        added.push(answer);
        if day.ended || out_of_calls {
            break;
        }
    }
    Outcome::Lived { messages: added, acts: day.acts, refused: day.refused, usage }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(day: u64) -> Value {
        json!({ "role": "user", "content": [{ "type": "text", "text": format!("Day {day}.\nMoney: you hold 10.00 in cash.\n") }] })
    }
    fn used(id: &str, name: &str, input: Value) -> Value {
        json!({ "role": "assistant", "content": [{ "type": "tool_use", "id": id, "name": name, "input": input }] })
    }
    fn answered(id: &str, text: &str, is_error: bool) -> Value {
        json!({ "role": "user", "content": [{ "type": "tool_result", "tool_use_id": id, "content": text, "is_error": is_error }] })
    }
    fn a_day(day: u64) -> Vec<Value> {
        vec![
            note(day),
            used("a", "offers", json!({})),
            answered("a", "{\"awaiting_me\":[]}", false),
            used("b", "offer_sale", json!({ "counterparty": "7", "you_are": "seller", "amount": "150.47", "days": 30 })),
            answered("b", "offer sent, waiting for the others (ref add95e1aeb25) It takes effect when your day ends, if the ledger still allows it then.", false),
            used("c", "sign_offer", json!({ "ref": "6978df" })),
            answered("c", "refused by the ledger: ET-TX-002", true),
            used("d", "diary", json!({ "text": format!("Day {day}: an ordinary day.") })),
            answered("d", "done", false),
            used("e", "end_day", json!({})),
            answered("e", tools::DAY_OVER, false),
        ]
    }

    #[test]
    fn a_short_life_and_a_zero_window_are_sent_whole() {
        let life: Vec<Value> = (1..=3).flat_map(a_day).collect();
        assert_eq!(compact(&life, 0), life);
        assert_eq!(compact(&life, 3), life);
        assert_eq!(compact(&life, 4), life);
    }

    #[test]
    fn a_day_past_the_window_is_the_person_s_own_memory_of_it() {
        let life: Vec<Value> = (1..=4).flat_map(a_day).collect();
        let sent = compact(&life, 2);
        // Two remembered turns, then days 3 and 4 as they were.
        assert_eq!(sent.len(), 2 + 2 * a_day(1).len());
        let first = sent[0]["content"][0]["text"].as_str().unwrap();
        assert!(first.starts_with("Day 1, as you remember it:\n"), "{first}");
        // The act kept and what it came to, cut before the standing sentence.
        assert!(first.contains("- offer_sale {"), "{first}");
        assert!(first.contains("→ offer sent, waiting for the others (ref add95e1aeb25)"), "{first}");
        assert!(!first.contains("It takes effect"), "{first}");
        // The refused signature, the view and the end of the day leave nothing.
        assert!(!first.contains("sign_offer"), "{first}");
        assert!(!first.contains("offers {"), "{first}");
        assert!(!first.contains("end_day"), "{first}");
        // Their own words, whole.
        assert!(first.contains("- you wrote in your diary: \"Day 1: an ordinary day.\""), "{first}");
        assert_eq!(&sent[2..], &life[2 * a_day(1).len()..]);
        // A remembered day is a user turn and never an orphaned tool result.
        for m in &sent[..2] {
            assert_eq!(m["role"], "user");
            assert_eq!(m["content"][0]["type"], "text");
        }
    }

    #[test]
    fn a_day_of_looking_says_so() {
        let life = vec![note(1), used("a", "my_account", json!({})), answered("a", "{}", false), note(2)];
        let sent = compact(&life, 1);
        assert_eq!(
            sent[0]["content"][0]["text"],
            "Day 1, as you remember it: you looked at your wallet and did nothing."
        );
        assert_eq!(sent[1], note(2));
    }

    #[test]
    fn the_same_life_compacts_the_same_way_on_a_resume() {
        let life: Vec<Value> = (1..=6).flat_map(a_day).collect();
        assert_eq!(compact(&life, 3), compact(&life.clone(), 3));
    }

    /// The prefix moves once a block: day 11 to 19 are sent as day 10 was,
    /// day 20 folds ten days, and day 29 is sent as day 20 was.
    #[test]
    fn days_fold_in_blocks_so_the_prefix_stands_between_them() {
        let remembered = |days: u64, kept: u32| {
            let life: Vec<Value> = (1..=days).flat_map(a_day).collect();
            compact(&life, kept)
                .iter()
                .filter(|m| {
                    m["content"][0]["text"]
                        .as_str()
                        .is_some_and(|t| t.contains(", as you remember it"))
                })
                .count()
        };
        assert_eq!(remembered(10, 10), 0);
        assert_eq!(remembered(11, 10), 0);
        assert_eq!(remembered(19, 10), 0);
        assert_eq!(remembered(20, 10), 10);
        assert_eq!(remembered(29, 10), 10);
        assert_eq!(remembered(30, 10), 20);
        // The prefix of day 19's conversation is day 11's, whole.
        let life: Vec<Value> = (1..=19).flat_map(a_day).collect();
        let earlier: Vec<Value> = (1..=11).flat_map(a_day).collect();
        assert_eq!(&compact(&life, 10)[..earlier.len()], &compact(&earlier, 10)[..]);
    }
}
