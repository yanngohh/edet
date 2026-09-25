//! **The tools a person's wallet gives them**, in the shape an MCP server
//! declares tools: a name, a description, a JSON input schema.
//!
//! Reads answer from the world as it stands, through the node's own
//! disclosure rules with the person as the viewer, so what a person sees of
//! anybody else is what a member sees over a node. Acts are tried at once on a
//! copy of the world that carries the day's earlier acts, answer with what they
//! came to, and are kept to be applied when the day ends. **No tool takes an
//! acting member**: the dispatcher is bound to one person, and every act is
//! theirs.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use edet_state::state::State;
use edet_state::types::{ContractStatus, MemberId, ParamKey, Party, ProposalKind};

use crate::acts::{Act, ActResult, Ask, Instruction, PaidWith, Side};
use crate::world::{nonce_for, World};
use crate::{fmt_minor, hex, parse_amount};

/// The whole tool list, as the model API takes it.
pub fn definitions() -> Vec<Value> {
    let t = |name: &str, description: &str, properties: Value, required: &[&str]| {
        json!({
            "name": name,
            "description": description,
            "input_schema": { "type": "object", "properties": properties, "required": required },
        })
    };
    let s = |d: &str| json!({ "type": "string", "description": d });
    let i = |d: &str| json!({ "type": "integer", "description": d });
    vec![
        t("my_account", "Your own account as the ledger shows it to you, your cash, and the standing instructions you have left.", json!({}), &[]),
        t("members", "The community's members, fifty at a time, as the ledger shows them to you, with the risk figure your wallet computes for each.", json!({ "after": i("Show members after this member number.") }), &[]),
        t("neighbours", "The people you know: the ones you have dealt with or grew up around, and whether each of them uses edet at all.", json!({}), &[]),
        t("member", "One member in detail, as the ledger shows them to you.", json!({ "member": i("The member number.") }), &["member"]),
        t("contract", "One contract, as the ledger shows it to you.", json!({ "contract": i("The contract number.") }), &["contract"]),
        t("offers", "Offers waiting for your signature, and offers of yours waiting for somebody else's.", json!({}), &[]),
        t("rules", "The community's governed parameters and the fees its writes reserve.", json!({}), &[]),
        t("proposals", "Every governance proposal and who has assented.", json!({}), &[]),
        t("economy", "Your cash and bills, and the published figures on prices and incomes.", json!({}), &[]),
        t("square", "The most recent posts on the public square.", json!({}), &[]),
        t("mail", "Your private messages, sent and received.", json!({}), &[]),
        t("offer_credit", "Offer a loan on the ledger, as lender or as borrower. It waits for the other person to sign. Name a member by number, or anybody in town by address: a neighbour with no account yet is seated by this first trade, which is bonded against your own standing. \"new\" is somebody with no account whose address you do not have.",
          json!({ "counterparty": s("A member number, an address, or \"new\"."), "you_are": s("\"lender\" or \"borrower\"."), "amount": s("The amount, e.g. \"120.50\"."), "days": i("Days until it falls due.") }),
          &["counterparty", "you_are", "amount", "days"]),
        t("offer_sale", "Offer a sale on credit on the ledger, as seller or buyer: the buyer owes the seller. It waits for the other person to sign. Name a member by number, or anybody in town by address: a neighbour with no account yet is seated by this first trade, which is bonded against your own standing. \"new\" is somebody with no account whose address you do not have.",
          json!({ "counterparty": s("A member number, an address, or \"new\"."), "you_are": s("\"seller\" or \"buyer\"."), "amount": s("The amount."), "days": i("Days until it falls due.") }),
          &["counterparty", "you_are", "amount", "days"]),
        t("pay_debt", "Offer to pay what you owe on a contract, in part or in full. The lender must sign. A contract already past due is cured rather than settled. Say whether you are paying in cash or by delivering something of value.",
          json!({ "contract": i("The contract number."), "amount": s("The amount."), "paid_with": s("\"cash\" or \"value\".") }),
          &["contract", "amount", "paid_with"]),
        t("extend_contract", "Offer to move a contract's due day. Both parties must sign.", json!({ "contract": i("The contract number."), "new_due_day": i("The new due day.") }), &["contract", "new_due_day"]),
        t("transfer_debt", "Offer to hand a debt you owe to another member, who must sign.", json!({ "contract": i("The contract number."), "new_debtor": i("The member taking it on.") }), &["contract", "new_debtor"]),
        t("sign_offer", "Sign an offer waiting for you.", json!({ "ref": s("The offer's reference, as the offers list shows it.") }), &["ref"]),
        t("decline_offer", "Decline an offer waiting for you, or withdraw one of yours.", json!({ "ref": s("The offer's reference.") }), &["ref"]),
        t("declare_supply", "As an underwriter, lower the supply you stand behind. It can never be raised this way.", json!({ "supply": s("The new, lower supply.") }), &["supply"]),
        t("propose", "Put a governance proposal to the community.",
          json!({ "kind": s("\"param_change\", \"redenominate\", \"suspend\", \"unsuspend\" or \"seed_amendment\"."),
                  "key": s("For a param_change: RiskK, SealAmounts, BondFraction, StakeDecay, SeedRate or InsuredHorizon."),
                  "value": s("For a param_change: the new value."),
                  "num": i("For a redenomination: the numerator."), "den": i("For a redenomination: the denominator."),
                  "member": i("For a suspension: the member."), "amount": s("For a seed amendment: the supply you commit.") }),
          &["kind"]),
        t("assent", "Assent to a governance proposal.", json!({ "proposal": i("The proposal number.") }), &["proposal"]),
        t("support", "List the members whose debts a share of what is repaid to you may clear, with weights.", json!({ "beneficiaries": { "type": "array", "items": { "type": "object", "properties": { "member": { "type": "integer" }, "weight": { "type": "number" } }, "required": ["member", "weight"] } } }), &["beneficiaries"]),
        t("approve_supporter", "Approve, or withdraw approval of, a member who lists you as a beneficiary.", json!({ "supporter": i("The member."), "approved": { "type": "boolean" } }), &["supporter", "approved"]),
        t("register_guardians", "Name the members who may together recover your account.", json!({ "guardians": { "type": "array", "items": { "type": "integer" } }, "threshold": i("How many of them must agree.") }), &["guardians", "threshold"]),
        t("leave_ledger", "Leave the ledger for good. This cannot be undone.", json!({}), &[]),
        t("pay_cash", "Pay somebody in money, outside the ledger.", json!({ "to": s("A member number or an address."), "amount": s("The amount."), "note": s("What it is for.") }), &["to", "amount"]),
        t("post", "Say something on the public square, where everybody can read it.", json!({ "text": s("What you say.") }), &["text"]),
        t("message", "Send a private message.", json!({ "to": s("A member number or an address."), "text": s("What you say.") }), &["to", "text"]),
        t("diary", "Write in your own diary: who you are becoming, and why. Nobody else reads it.", json!({ "text": s("The entry.") }), &["text"]),
        t("set_instruction", "Leave a standing instruction your wallet carries out while you are away: pay a contract the day before it falls due, or co-sign a loan, sale or payment one member offers you, up to an amount.",
          json!({ "kind": s("\"pay_at_maturity\" or \"accept_from\"."), "contract": i("For pay_at_maturity."), "paid_with": s("For pay_at_maturity: \"cash\" or \"value\"."), "member": i("For accept_from."), "max_amount": s("For accept_from.") }),
          &["kind"]),
        t("revoke_instruction", "Remove a standing instruction.", json!({ "id": i("The instruction's number.") }), &["id"]),
        t("end_day", "You are done for today.", json!({}), &[]),
    ]
}

/// The tools that read. Every other tool but `end_day` is an act.
pub const READS: &[&str] = &[
    "my_account",
    "members",
    "member",
    "neighbours",
    "contract",
    "offers",
    "rules",
    "proposals",
    "economy",
    "square",
    "mail",
];

/// How many acts a day's exchange says were kept: tool calls that are acts,
/// answered without error, before the day ended. The tape's event for that day
/// must record exactly this many — the check that the two layers of a tape
/// describe the same day.
pub fn kept_in(messages: &[Value]) -> usize {
    let mut names = std::collections::BTreeMap::new();
    let mut kept = 0;
    for m in messages {
        for b in m["content"].as_array().into_iter().flatten() {
            match b["type"].as_str() {
                Some("tool_use") => {
                    names.insert(b["id"].to_string(), b["name"].as_str().unwrap_or_default().to_string());
                }
                Some("tool_result") => {
                    let name = names.get(&b["tool_use_id"].to_string()).cloned().unwrap_or_default();
                    let over = b["content"].as_str() == Some(DAY_OVER);
                    let act = !READS.contains(&name.as_str()) && name != "end_day";
                    if act && !over && b["is_error"] != true {
                        kept += 1;
                    }
                }
                _ => {}
            }
        }
    }
    kept
}

/// What every tool call after the day ended is answered with.
pub const DAY_OVER: &str = "Your day is over.";

/// A day's dispatcher: bound to one person, reading the world, trying acts on
/// its copy.
pub struct Day<'a> {
    pub world: &'a World,
    /// The wallet's English for a refusal, by code, to put beside the code a
    /// refusal answers with. A member's wallet says this much on the screen
    /// that refused them.
    pub errors: &'a BTreeMap<String, String>,
    pub scratch: World,
    pub person: usize,
    pub tick: u64,
    pub acts: Vec<Act>,
    /// Acts the check refused, kept for the tape and never applied.
    pub refused: Vec<(Act, ActResult)>,
    pub ended: bool,
    tried: u64,
}

pub struct Answer {
    pub text: String,
    pub is_error: bool,
}

/// A view, as a person's wallet would answer them. Written COMPACT: pretty
/// printing this tree is 62% whitespace measured over a scripted run's 1,200
/// answers, and every character of it is carried for the rest of that person's
/// life.
fn ok(v: Value) -> Answer {
    Answer { text: serde_json::to_string(&readable(v)).unwrap_or_default(), is_error: false }
}

/// A party as a wallet puts it on the screen: an address, or a member by
/// number. The ledger's own encoding of a key is thirty-two numbers, which is
/// sixteen per cent of everything a person is told, measured over a scripted
/// run, and no wallet has ever shown one.
fn readable(v: Value) -> Value {
    match v {
        Value::Object(map) => {
            // A party and nothing else: the ledger writes a key as thirty-two
            // bytes and a member as a whole number, and anything shaped
            // otherwise is left exactly as it came.
            if map.len() == 1 {
                if let Some(Value::Array(bytes)) = map.get("Key") {
                    let raw: Vec<u8> = bytes
                        .iter()
                        .filter_map(|b| b.as_u64().filter(|n| *n < 256).map(|n| n as u8))
                        .collect();
                    if raw.len() == bytes.len() && raw.len() == 32 {
                        return json!(edet_view::disclose::address_hex(&edet_view::disclose::key_address_bytes(&raw)));
                    }
                }
                if let Some(Value::Number(n)) = map.get("Member") {
                    if n.is_u64() {
                        return json!(format!("member {n}"));
                    }
                }
            }
            Value::Object(map.into_iter().map(|(k, x)| (k, readable(x))).collect())
        }
        Value::Array(xs) => Value::Array(xs.into_iter().map(readable).collect()),
        other => other,
    }
}

fn no(reason: impl Into<String>) -> Answer {
    Answer { text: reason.into(), is_error: true }
}

impl<'a> Day<'a> {
    pub fn new(world: &'a World, errors: &'a BTreeMap<String, String>, person: usize, tick: u64) -> Self {
        Day {
            world,
            errors,
            scratch: world.clone(),
            person,
            tick,
            acts: Vec::new(),
            refused: Vec::new(),
            ended: false,
            tried: 0,
        }
    }

    fn me(&self) -> Option<MemberId> {
        self.world.persons[self.person].member
    }

    pub fn call(&mut self, name: &str, input: &Value) -> Answer {
        let r = match name {
            "my_account" => self.my_account(),
            "members" => self.members(input),
            "member" => self.member(input),
            "neighbours" => self.neighbours(),
            "contract" => self.contract(input),
            "offers" => self.offers(),
            "rules" => Ok(ok(edet_view::disclose::params(self.world.st()))),
            "proposals" => Ok(ok(edet_view::disclose::proposals(self.world.st()))),
            "economy" => self.economy(),
            "square" => self.square(),
            "mail" => self.mail(),
            "end_day" => {
                self.ended = true;
                Ok(Answer { text: DAY_OVER.into(), is_error: false })
            }
            _ => self.act(name, input),
        };
        match r {
            Ok(a) => a,
            Err(reason) => no(reason),
        }
    }

    // -------------------------------------------------------------- reads --

    fn my_account(&self) -> Result<Answer, String> {
        let w = self.world;
        let p = &w.persons[self.person];
        let purse = &w.economy.purses[self.person];
        let instructions: Vec<Value> = w
            .instructions
            .iter()
            .filter(|(_, (o, _))| *o == self.person)
            .map(|(id, (_, i))| json!({ "id": id, "instruction": i }))
            .collect();
        let mut out = json!({
            "day": w.st().epoch,
            "address": p.address(),
            "cash": fmt_minor(purse.cash),
            "unpaid_bills": fmt_minor(purse.arrears),
            "standing_instructions": instructions,
        });
        match p.member {
            Some(id) => {
                out["account"] = edet_view::disclose::member(w.st(), id, Some(id));
                // The wallet's own derived line beside the ledger's two raw
                // figures it is made of, exactly as `MyWallet` shows it.
                if let Some(n) = crate::prompt::seats_left(w, self.person) {
                    out["newcomers_you_can_bring_in"] = json!(n);
                }
            }
            None => {
                out["account"] =
                    json!("You have no account on the ledger yet. A member's first trade with you opens one.")
            }
        }
        Ok(ok(out))
    }

    fn with_risk(&self, mut row: Value) -> Value {
        let st = self.world.st();
        let f = |k: &str| row.get(k).and_then(Value::as_f64);
        if let (Some(cap), Some(debt)) = (f("capacity"), f("debt")) {
            let r = edet_kernel::risk::member_risk(
                cap,
                debt,
                f("d_in").unwrap_or(0.0),
                f("d_out").unwrap_or(0.0),
                st.params.get(ParamKey::RiskK),
                st.params.v_base,
            );
            row["risk"] = json!((r * 100.0).round() / 100.0);
        }
        row
    }

    fn members(&self, input: &Value) -> Result<Answer, String> {
        let st = self.world.st();
        let after = input.get("after").and_then(Value::as_u64);
        let start = after.map(|a| a + 1).unwrap_or(0);
        let rows: Vec<Value> = st
            .members
            .range(start..)
            .take(50)
            .map(|(_, m)| {
                let (row, _) = edet_view::disclose::member_row(st, m, self.me(), &|id| Some(st.capacity_of(id)));
                self.with_risk(row)
            })
            .collect();
        Ok(ok(json!({ "members": rows, "total": st.members.len() })))
    }

    fn member(&self, input: &Value) -> Result<Answer, String> {
        let id = input.get("member").and_then(Value::as_u64).ok_or("which member?")?;
        let st = self.world.st();
        if !st.members.contains_key(&id) {
            return Err(format!("there is no member {id}"));
        }
        let view = edet_view::disclose::member(st, id, self.me());
        let mut out = self.with_risk(view);
        if let Some(p) = self.world.person_of_member(id) {
            out["address"] = json!(self.world.persons[p].address());
        }
        Ok(ok(out))
    }

    fn contract(&self, input: &Value) -> Result<Answer, String> {
        let id = input.get("contract").and_then(Value::as_u64).ok_or("which contract?")?;
        let st = self.world.st();
        let c = st.contracts.get(&id).ok_or(format!("there is no contract {id}"))?;
        Ok(ok(edet_view::disclose::contract_view(c, st, self.me())))
    }

    fn offers(&self) -> Result<Answer, String> {
        let w = self.world;
        let party = w.persons[self.person].party();
        let mut view = edet_view::disclose::pending_for(w.st(), &w.pool, party);
        // **An offer past its window is shown as gone, not as waiting.** The
        // pool keeps the entry, and the ledger answers a signature on it with
        // `ET-TX-002`; shown as waiting, pilot-3's newcomers signed the same
        // expired first-trade offer day after day — 425 refusals on the
        // check, 159 stale entries by day 100 — and read it as the ledger
        // refusing THEM.
        let today = w.st().epoch;
        let mut expired = 0usize;
        for list in ["awaiting_me", "mine"] {
            if let Some(Value::Array(rows)) = view.get_mut(list) {
                let before = rows.len();
                rows.retain(|row| row["not_after_epoch"].as_u64().is_none_or(|e| e >= today));
                expired += before - rows.len();
                for row in rows {
                    let digest = row["digest"].as_str().unwrap_or_default().to_string();
                    row["ref"] = json!(&digest[..12.min(digest.len())]);
                    // What a wallet shows and what a co-signer's client needs
                    // are not the same list: the digest a person quotes is the
                    // reference, and the nonce and window exist to reproduce a
                    // signature the run makes for them.
                    if let Some(secs) = row["created_secs"].as_u64() {
                        row["opened_on_day"] = json!(secs / edet_kernel::constants::EPOCH_SECS);
                    }
                    if let Some(epoch) = row["not_after_epoch"].as_u64() {
                        row["good_until_day"] = json!(epoch);
                    }
                    if let Some(o) = row.as_object_mut() {
                        for machinery in ["digest", "nonce", "not_after_epoch", "initiator", "created_secs"] {
                            o.remove(machinery);
                        }
                    }
                    if let Some(meta) = crate::unhex::<32>(&digest).and_then(|d| w.offers.get(&d)) {
                        row["what"] = json!(meta.what);
                        row["from"] = json!(w.name_of(meta.opener));
                        if let Some(pw) = meta.paid_with {
                            row["paid_with"] = json!(pw);
                        }
                    }
                }
            }
        }
        if expired > 0 {
            view["expired_and_gone"] = json!(expired);
        }
        Ok(ok(view))
    }

    fn economy(&self) -> Result<Answer, String> {
        let w = self.world;
        let purse = &w.economy.purses[self.person];
        Ok(ok(json!({
            "cash": fmt_minor(purse.cash),
            "unpaid_bills": fmt_minor(purse.arrears),
            "income_every_days": purse.income_period,
            "income_cut": purse.cut_until.is_some(),
            "published": w.economy.indicator(),
        })))
    }

    /// Who this person knows. Not a ledger view: the town knows who its
    /// people are whether or not they hold an account, and this says which of
    /// them do.
    fn neighbours(&self) -> Result<Answer, String> {
        let w = self.world;
        if !w.social.on() {
            return Ok(ok(json!({ "everybody": true, "note": "you know everybody in this town" })));
        }
        let rows: Vec<Value> = w
            .social
            .neighbours(self.person)
            .into_iter()
            .map(|i| {
                let p = &w.persons[i];
                json!({ "who": w.name_of(i), "member": p.member, "uses_edet": p.member.is_some() })
            })
            .collect();
        Ok(ok(json!({ "neighbours": rows })))
    }

    fn square(&self) -> Result<Answer, String> {
        let w = self.world;
        let posts: Vec<Value> = w
            .square
            .iter()
            .rev()
            .take(40)
            .rev()
            .map(|s| json!({ "day": w.genesis_epoch + s.tick, "from": w.name_of(s.person), "text": s.text }))
            .collect();
        Ok(ok(json!({ "posts": posts })))
    }

    fn mail(&self) -> Result<Answer, String> {
        let w = self.world;
        let all: Vec<&crate::world::Mail> =
            w.mail.iter().filter(|m| m.to == self.person || m.from == self.person).collect();
        let mine: Vec<Value> = all[all.len().saturating_sub(60)..]
            .iter()
            .map(|m| {
                json!({
                    "day": w.genesis_epoch + m.tick,
                    "from": if m.from == self.person { "you".to_string() } else { w.name_of(m.from) },
                    "to": if m.to == self.person { "you".to_string() } else { w.name_of(m.to) },
                    "text": m.text,
                })
            })
            .collect();
        Ok(ok(json!({ "messages": mine })))
    }

    // --------------------------------------------------------------- acts --

    fn act(&mut self, name: &str, input: &Value) -> Result<Answer, String> {
        let act = self.build(name, input)?;
        self.tried += 1;
        let mut note = String::new();
        if let Act::Offer { ask, required, newcomer, .. } = &act {
            if let Err(code) = preview(&self.scratch, ask, required) {
                // **An offer that would open somebody's account is not sent
                // when its sponsor cannot pay for the seat today.** The seat
                // is bonded against the SPONSOR's headroom and reach, so the
                // newcomer can do nothing about it, and an offer left waiting
                // is one they will try to sign every day it stands: pilot-3's
                // six sponsors opened 126 of them, seven were ever seated, and
                // 1,137 signatures were refused on the check for the same
                // code the sponsor had been warned of at the moment of
                // sending. Refused here, where the person who can act on it
                // is the one told.
                // A "new" key and a neighbour named by address who has no
                // row yet are the same seating, and the pre-check must read
                // the PARTIES and not the flag: pilot-4's first two offers to
                // wallet-holding neighbours went out under a note where a
                // fresh key's would have been refused.
                let seats = required
                    .iter()
                    .any(|p| matches!(p, Party::Key(k) if self.world.st().member_of_key(k).is_none()));
                if (newcomer.is_some() || seats) && code.starts_with("ET-BND-") {
                    let r = ActResult::Rejected {
                        reason: format!(
                            "an offer to somebody with no account is bonded against your own headroom and seat \
                             reach, and today the ledger would refuse it: {code}.{} Nothing was sent. Settle in \
                             cash today, or free some capacity and offer again another day.",
                            self.says(&code)
                        ),
                    };
                    let text = r.describe();
                    self.refused.push((act, r));
                    return Ok(Answer { text, is_error: true });
                }
                note = format!(
                    " If everybody signs as it stands today, the ledger would refuse it: {code}.{}",
                    self.says(&code)
                );
            }
        }
        let r = self.scratch.run_act(self.tick, self.person, &act);
        let kept = !matches!(r, ActResult::Refused { .. } | ActResult::Rejected { .. });
        let said = match &r {
            ActResult::Refused { code } => self.says(code),
            _ => String::new(),
        };
        let text = format!(
            "{}{}{}{}",
            r.describe(),
            said,
            note,
            if kept { " It takes effect when your day ends, if the ledger still allows it then." } else { "" }
        );
        if kept {
            self.acts.push(act);
        } else {
            self.refused.push((act, r));
        }
        Ok(Answer { text, is_error: !kept })
    }

    /// What the wallet puts on the screen beside a refusal code, where it has
    /// words for that one.
    fn says(&self, code: &str) -> String {
        match self.errors.get(code) {
            Some(text) => format!(" Your wallet says: {text}"),
            None => String::new(),
        }
    }

    fn build(&self, name: &str, input: &Value) -> Result<Act, String> {
        let s = |k: &str| input.get(k).and_then(Value::as_str).map(str::trim).unwrap_or("");
        let n = |k: &str| input.get(k).and_then(Value::as_u64);
        let amount = |k: &str| {
            let raw = input.get(k).map(|v| match v {
                Value::String(x) => x.clone(),
                other => other.to_string(),
            });
            raw.as_deref()
                .and_then(parse_amount)
                .filter(|a| *a > 0)
                .ok_or(format!("\"{k}\" is not an amount"))
        };
        let me = || self.me().ok_or("you have no account on the ledger yet".to_string());
        let paid = |k: &str| match s(k) {
            "cash" => Ok(PaidWith::Cash),
            "value" => Ok(PaidWith::Value),
            other => Err(format!("paid_with must be \"cash\" or \"value\", not {other:?}")),
        };
        let person_to = |k: &str| {
            let raw = input.get(k).map(|v| match v {
                Value::String(x) => x.clone(),
                other => other.to_string(),
            });
            raw.as_deref()
                .and_then(|r| self.world.resolve(r))
                .ok_or(format!("nobody is known by that {k}"))
        };
        match name {
            "offer_credit" | "offer_sale" => {
                let me = me()?;
                let other = self.side(s("counterparty"))?;
                let days = n("days").ok_or("days until it falls due?")?;
                let amount_minor = amount("amount")?;
                let mine = Side::Member(me);
                let ask = match (name, s("you_are")) {
                    ("offer_credit", "lender") => Ask::Lend { creditor: mine, debtor: other, amount_minor, term: days },
                    ("offer_credit", "borrower") => {
                        Ask::Lend { creditor: other, debtor: mine, amount_minor, term: days }
                    }
                    ("offer_sale", "seller") => Ask::Sell { seller: mine, buyer: other, amount_minor, term: days },
                    ("offer_sale", "buyer") => Ask::Sell { seller: other, buyer: mine, amount_minor, term: days },
                    (_, who) => return Err(format!("you_are {who:?} does not fit this offer")),
                };
                self.ledger_act(me, ask, None)
            }
            "pay_debt" => {
                let me = me()?;
                let contract = n("contract").ok_or("which contract?")?;
                let c = self
                    .world
                    .st()
                    .contracts
                    .get(&contract)
                    .ok_or(format!("there is no contract {contract}"))?;
                let amount_minor = amount("amount")?;
                let ask = if c.status == ContractStatus::Expired {
                    Ask::Cure { contract, amount_minor }
                } else {
                    Ask::Settle { contract, amount_minor }
                };
                self.ledger_act(me, ask, Some(paid("paid_with")?))
            }
            "extend_contract" => {
                let me = me()?;
                let contract = n("contract").ok_or("which contract?")?;
                let new_maturity_epoch = n("new_due_day").ok_or("which day?")?;
                self.ledger_act(me, Ask::Extend { contract, new_maturity_epoch }, None)
            }
            "transfer_debt" => {
                let me = me()?;
                let contract = n("contract").ok_or("which contract?")?;
                let new_debtor = n("new_debtor").ok_or("to whom?")?;
                self.ledger_act(me, Ask::Transfer { contract, new_debtor }, None)
            }
            "sign_offer" | "decline_offer" => {
                let digest = self.digest_of(s("ref"), name == "decline_offer")?;
                Ok(if name == "sign_offer" { Act::Sign { digest } } else { Act::Decline { digest } })
            }
            "declare_supply" => {
                let me = me()?;
                self.ledger_act(me, Ask::Declare { supply_minor: amount("supply")? }, None)
            }
            "propose" => {
                let me = me()?;
                let proposal = match s("kind") {
                    "param_change" => {
                        let key = match s("key") {
                            "RiskK" => ParamKey::RiskK,
                            "SealAmounts" => ParamKey::SealAmounts,
                            "BondFraction" => ParamKey::BondFraction,
                            "StakeDecay" => ParamKey::StakeDecay,
                            "SeedRate" => ParamKey::SeedRate,
                            "InsuredHorizon" => ParamKey::InsuredHorizon,
                            other => return Err(format!("no parameter {other:?}")),
                        };
                        let raw = input.get("value").map(|v| match v {
                            Value::String(x) => x.clone(),
                            other => other.to_string(),
                        });
                        let value: f64 = raw.as_deref().and_then(|x| x.trim().parse().ok()).ok_or("what value?")?;
                        ProposalKind::ParamChange { key, value }
                    }
                    "redenominate" => ProposalKind::Redenominate {
                        num: n("num").ok_or("the numerator?")?,
                        den: n("den").ok_or("the denominator?")?,
                    },
                    "suspend" => ProposalKind::Suspend { member: n("member").ok_or("which member?")? },
                    "unsuspend" => ProposalKind::Unsuspend { member: n("member").ok_or("which member?")? },
                    "seed_amendment" => ProposalKind::SeedAmendment { amount: State::from_minor(amount("amount")?) },
                    other => return Err(format!("no proposal of kind {other:?}")),
                };
                self.ledger_act(me, Ask::Propose { proposal }, None)
            }
            "assent" => {
                let me = me()?;
                self.ledger_act(me, Ask::Assent { proposal: n("proposal").ok_or("which proposal?")? }, None)
            }
            "support" => {
                let me = me()?;
                let entries: Vec<(MemberId, f64)> = input
                    .get("beneficiaries")
                    .and_then(Value::as_array)
                    .ok_or("whom?")?
                    .iter()
                    .filter_map(|e| Some((e.get("member")?.as_u64()?, e.get("weight")?.as_f64()?)))
                    .collect();
                self.ledger_act(me, Ask::ListBeneficiaries { entries }, None)
            }
            "approve_supporter" => {
                let me = me()?;
                let supporter = n("supporter").ok_or("which member?")?;
                let approved = input.get("approved").and_then(Value::as_bool).ok_or("approved or not?")?;
                self.ledger_act(me, Ask::ApproveSupporter { supporter, approved }, None)
            }
            "register_guardians" => {
                let me = me()?;
                let guardians: Vec<MemberId> = input
                    .get("guardians")
                    .and_then(Value::as_array)
                    .ok_or("whom?")?
                    .iter()
                    .filter_map(Value::as_u64)
                    .collect();
                let threshold = n("threshold").ok_or("how many?")? as u32;
                let veto_window_epochs = self.world.st().params.min_maturity_epochs;
                self.ledger_act(me, Ask::RegisterGuardians { guardians, threshold, veto_window_epochs }, None)
            }
            "leave_ledger" => self.ledger_act(me()?, Ask::Exit, None),
            "pay_cash" => Ok(Act::PayCash {
                to: person_to("to")?,
                amount_minor: amount("amount")?,
                note: s("note").chars().take(500).collect(),
            }),
            "post" => Ok(Act::Post { text: text(s("text"))? }),
            "message" => Ok(Act::Message { to: person_to("to")?, text: text(s("text"))? }),
            "diary" => Ok(Act::Diary { text: text(s("text"))? }),
            "set_instruction" => {
                let instruction = match s("kind") {
                    "pay_at_maturity" => Instruction::PayAtMaturity {
                        contract: n("contract").ok_or("which contract?")?,
                        paid_with: paid("paid_with")?,
                    },
                    // A payment to you is acknowledged, never auto-signed:
                    // a wallet that co-signed receipts would confirm cash
                    // it never got. Tapes written when this was offered
                    // still replay through `Instruction::AcceptPayments`.
                    "accept_payments" => {
                        return Err("accept_payments is not something a wallet can do: a payment to you waits for you to confirm you received it".into())
                    }
                    "accept_from" => Instruction::AcceptFrom {
                        member: n("member").ok_or("which member?")?,
                        max_amount_minor: amount("max_amount")?,
                    },
                    other => return Err(format!("no instruction of kind {other:?}")),
                };
                Ok(Act::SetInstruction { instruction })
            }
            "revoke_instruction" => Ok(Act::RevokeInstruction { id: n("id").ok_or("which instruction?")? }),
            other => Err(format!("there is no tool called {other}")),
        }
    }

    /// Whom an offer names. A member by number or address; a person of the
    /// town with no account by address, whom this trade seats; `"new"` for a
    /// stranger the offer itself brings into the town. An address with no
    /// row used to be refused here as "that person has no account yet",
    /// against a note that said a trade recorded with them would open one,
    /// and every seat of three pilots was a minted stranger for it.
    fn side(&self, who: &str) -> Result<Side, String> {
        if who.eq_ignore_ascii_case("new") {
            let p = &self.scratch.persons[self.person];
            return Ok(Side::Newcomer { owner: self.person, n: p.minted });
        }
        let i = self.world.resolve(who).ok_or(format!("nobody is known as {who:?}"))?;
        if i == self.person {
            return Err("that is you".into());
        }
        Ok(match self.scratch.persons[i].member {
            Some(id) => Side::Member(id),
            None => Side::Person(i),
        })
    }

    /// The act a ledger ask becomes: a transaction only the person signs goes
    /// straight to the ledger, and one that needs anybody else waits in the
    /// pool for them.
    fn ledger_act(&self, me: MemberId, ask: Ask, paid_with: Option<PaidWith>) -> Result<Act, String> {
        let st = self.scratch.st();
        let composed = self.scratch.compose(me, &ask);
        let fresh: Vec<Party> = composed
            .signers
            .iter()
            .filter(|k| st.member_of_key(k).is_none())
            .map(|k| Party::Key(*k))
            .collect();
        if composed.asks.is_empty() && fresh.is_empty() {
            return Ok(Act::Solo { ask });
        }
        let mut required = vec![Party::Member(me)];
        for a in composed.asks.iter().filter(|a| a.required) {
            let p = Party::Member(a.ask.member);
            if !required.contains(&p) {
                required.push(p);
            }
        }
        required.extend(fresh);
        let optional: Vec<Party> = composed
            .asks
            .iter()
            .filter(|a| !a.required)
            .map(|a| Party::Member(a.ask.member))
            .filter(|p| !required.contains(p))
            .collect();
        if !optional.is_empty() && preview(&self.scratch, &ask, &required).is_err() {
            let mut wider = required.clone();
            wider.extend(optional);
            if preview(&self.scratch, &ask, &wider).is_ok() {
                required = wider;
            }
        }
        let newcomer = ask.newcomer().map(|_| self.scratch.persons.len());
        let epoch = st.epoch;
        Ok(Act::Offer {
            ask,
            required,
            nonce: nonce_for(self.tick, self.person, self.tried),
            not_after_epoch: epoch + edet_kernel::constants::MAX_TX_LIFETIME_EPOCHS,
            newcomer,
            paid_with,
        })
    }

    /// The full digest of an offer the person may sign or decline, from the
    /// reference they were shown.
    fn digest_of(&self, reference: &str, mine_too: bool) -> Result<String, String> {
        let reference = reference.trim().trim_start_matches("0x").to_ascii_lowercase();
        if reference.len() < 8 {
            return Err("that reference is too short".into());
        }
        let w = &self.scratch;
        let (awaiting, mine) = w.pool.for_party(w.st(), w.persons[self.person].party());
        let mut all: Vec<[u8; 32]> = awaiting.iter().map(|(d, _)| *d).collect();
        if mine_too {
            all.extend(mine.iter().map(|(d, _)| *d));
        }
        let hits: Vec<String> = all.iter().map(|d| hex(d)).filter(|h| h.starts_with(&reference)).collect();
        match hits.as_slice() {
            [one] => Ok(one.clone()),
            [] => Err("no offer with that reference is waiting for you".into()),
            _ => Err("that reference matches more than one offer".into()),
        }
    }
}

fn text(t: &str) -> Result<String, String> {
    if t.is_empty() {
        return Err("say something".into());
    }
    Ok(t.chars().take(4000).collect())
}

/// What the ledger would say to this ask today if every party it names signed:
/// the check a wallet runs for a party before it asks anybody.
pub fn preview(world: &World, ask: &Ask, required: &[Party]) -> Result<(), String> {
    let st = world.st();
    let Some(me) = required.first().and_then(|p| match p {
        Party::Member(id) => Some(*id),
        Party::Key(_) => None,
    }) else {
        return Err("no member to act".into());
    };
    let composed = world.compose(me, ask);
    let mut signers: Vec<edet_state::types::Key> = required
        .iter()
        .filter_map(|p| match p {
            Party::Member(id) => st.members.get(id).and_then(|m| m.keys.first()).copied(),
            Party::Key(k) => Some(*k),
        })
        .collect();
    signers.sort_by_key(|k| st.member_of_key(k).unwrap_or(MemberId::MAX));
    signers.dedup();
    let mut copy = st.clone();
    let now = copy.last_begin_secs;
    let id = edet_state::root::value_digest(b"civitas-preview");
    let not_after = copy.epoch + 1;
    edet_state::apply(&mut copy, composed.tx, id, not_after, &signers, now).map_err(|e| e.0.to_string())
}
