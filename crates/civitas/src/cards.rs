//! **A newcomer's card, written when somebody first offers them a trade.**
//!
//! In the style of the twelve: two or three sentences of circumstance and
//! attitude, never a strategy. The writer is told what the town is like, not
//! who introduced the newcomer — a card the introducer implied would let a
//! member seed people like themselves. It also says what the person has read
//! about edet, which is what they will be told. Nobody reviews a written card
//! before its person acts; the tape keeps it. In a control run every newcomer
//! carries the honest card and nothing is written.

use serde_json::{json, Value};

use crate::config::{Card, Tier};
use crate::event::Usage;
use crate::model::{CallError, Model, Request};
use crate::world::World;

pub struct Written {
    pub name: String,
    pub card: String,
    pub tier: Tier,
    pub messages: Vec<Value>,
    pub usage: Usage,
}

pub fn write(model: &mut dyn Model, cards: &[Card], world: &World) -> Result<Written, String> {
    let examples: String = cards.iter().map(|c| format!("- {}: {}\n", c.name, c.card)).collect();
    let ind = world.economy.indicator();
    let members = world.st().members.len();
    let system = vec![json!({
        "type": "text",
        "text": "You write short character cards for ordinary people, in the second person, in the style of the \
    examples: two or three sentences about a person's circumstances and attitude, never a plan or a strategy. \
    Each card also says what the person has read about a new mutual-credit ledger called edet: \"paper\" if they \
    are the kind of person who would read its technical paper, \"readme\" if they would read the project's own \
    explanation, \"wallet\" if they would only ever read what the app on their phone tells them.",
    })];
    // **A card written for a town where the ledger is untried says so.** The
    // writer is given the record and not a stance: how far this person trusts
    // a thing they have watched nobody use is theirs, in their own terms, and
    // a town that has seen a hundred debts paid is not the town of the first
    // week.
    let untried = if world.cfg.trust.wary {
        format!(
            " Edet is new here and mostly untried: {}. A person's card may say, in their own terms, how far \
             they trust a thing like that before they have seen it work.",
            crate::prompt::ledger_record(world).trim_end_matches(['\n', '.'])
        )
    } else {
        String::new()
    };
    let user = json!({
        "role": "user",
        "content": [{ "type": "text", "text": format!(
            "Examples:\n{examples}\nThe town has {} households, {members} of them with an account on edet. Prices \
             stand at {} of where they began and moved {} over the last month.{untried} Write the card of one more \
             person who lives there and is about to be offered their first trade on edet by somebody they know.",
            ind.households, ind.price_index, ind.change_over_last_30_days
        ) }],
    });
    let tools = vec![json!({
        "name": "write_card",
        "description": "The card.",
        "input_schema": {
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "A short plain description, like the examples' names." },
                "card": { "type": "string", "description": "Two or three sentences, second person." },
                "tier": { "type": "string", "enum": ["paper", "readme", "wallet"] },
            },
            "required": ["name", "card", "tier"],
        },
    })];
    let messages = vec![user.clone()];
    let req = Request {
        system: &system,
        messages: &messages,
        tools: &tools,
        max_tokens: 1024,
        tool_choice: Some(json!({ "type": "tool", "name": "write_card" })),
    };
    let resp = model.call(&req).map_err(|e| match e {
        CallError::ContextFull(m) | CallError::Weather(m) | CallError::Failed(m) => m,
    })?;
    let input = resp
        .content
        .iter()
        .find(|b| b["type"] == "tool_use" && b["name"] == "write_card")
        .map(|b| b["input"].clone())
        .ok_or("the card writer returned no card")?;
    let name = input["name"].as_str().unwrap_or("").trim().to_string();
    let card = input["card"].as_str().unwrap_or("").trim().to_string();
    let tier = input["tier"].as_str().and_then(Tier::parse).ok_or("the card names no tier")?;
    if name.is_empty() || card.is_empty() || card.len() > 1200 {
        return Err("the card writer returned an empty or overlong card".into());
    }
    let reply = json!({ "role": "assistant", "content": resp.content });
    Ok(Written { name, card, tier, messages: vec![user, reply], usage: resp.usage })
}
