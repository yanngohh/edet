//! **What a person is told.**
//!
//! Who they are (their card, in the register `scripts/persona.py` writes
//! persons in), the town they live in, how their wallet works, and what they
//! know about the mechanism — which follows their tier: the paper, the
//! project's own account of the model, or only what the wallet itself explains.
//! Nothing a person is told names a run, a tick, a seed, an agent or a
//! simulation.
//!
//! Each day opens with a note of what happened to them since the last one:
//! money in and out, offers and what became of them, words addressed to them,
//! the square, and the published figures.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::config::{RunConfig, Tier};
use crate::fmt_minor;
use crate::world::World;

/// What each tier has read, loaded from the repository when a run starts.
#[derive(Clone, Debug)]
pub struct Knowledge {
    pub paper: String,
    pub readme: String,
    pub wallet: String,
    /// The wallet's English for a refusal, by the ledger's code. Nobody has
    /// read this: a member sees the sentence for the refusal they hit, on the
    /// turn they hit it, which is when their wallet would show it. Sixty-four
    /// of them on every turn of every life is the same words carried for a
    /// year to be read once.
    pub errors: BTreeMap<String, String>,
}

impl Knowledge {
    pub fn load(cfg: &RunConfig) -> Result<Knowledge, String> {
        let read = |rel: &str| {
            let path = cfg.path(rel);
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))
        };
        let mut paper = String::new();
        for rel in &cfg.knowledge.paper_reading {
            paper.push_str(&read(rel)?);
            paper.push_str("\n\n");
        }
        let readme_text = read("README.md")?;
        let mut readme = String::new();
        for heading in &cfg.knowledge.readme_headings {
            readme.push_str(&section(&readme_text, heading).ok_or(format!("README.md has no heading {heading:?}"))?);
            readme.push_str("\n\n");
        }
        let en: Value =
            serde_json::from_str(&read("ui/wallet/src/locales/en.json")?).map_err(|e| format!("en.json: {e}"))?;
        let mut wallet = String::new();
        for ns in &cfg.knowledge.wallet_namespaces {
            let v = en.get(ns).ok_or(format!("en.json has no namespace {ns:?}"))?;
            flatten(ns, v, &mut wallet);
        }
        let mut errors = BTreeMap::new();
        if let Some(Value::Object(map)) = en.get("errors") {
            for (code, v) in map {
                if let Value::String(text) = v {
                    errors.insert(code.clone(), text.clone());
                }
            }
        }
        Ok(Knowledge { paper, readme, wallet, errors })
    }

    /// The sha256 of what each tier is told, for the manifest.
    pub fn hashes(&self) -> Vec<(String, String)> {
        use sha2::{Digest, Sha256};
        let h = |s: &str| crate::hex(&Sha256::digest(s.as_bytes()));
        let mut errors = String::new();
        for (code, text) in &self.errors {
            errors.push_str(&format!("{code}: {text}\n"));
        }
        vec![
            ("paper".into(), h(&self.paper)),
            ("readme".into(), h(&self.readme)),
            ("wallet".into(), h(&self.wallet)),
            ("errors".into(), h(&errors)),
        ]
    }
}

/// A heading's section of a Markdown document: from the heading to the next
/// heading of the same or a higher level.
fn section(text: &str, heading: &str) -> Option<String> {
    let level = heading.chars().take_while(|c| *c == '#').count();
    let mut out = String::new();
    let mut inside = false;
    let mut fence = false;
    for line in text.lines() {
        if line.starts_with("```") {
            fence = !fence;
        }
        if !fence && line.starts_with('#') {
            let l = line.chars().take_while(|c| *c == '#').count();
            if inside && l <= level {
                break;
            }
            if line.trim() == heading.trim() {
                inside = true;
            }
        }
        if inside {
            out.push_str(line);
            out.push('\n');
        }
    }
    inside.then_some(out)
}

fn flatten(prefix: &str, v: &Value, out: &mut String) {
    match v {
        Value::String(s) => {
            out.push_str(prefix);
            out.push_str(": ");
            out.push_str(s);
            out.push('\n');
        }
        Value::Object(map) => {
            for (k, x) in map {
                flatten(&format!("{prefix}.{k}"), x, out);
            }
        }
        Value::Array(xs) => {
            for (i, x) in xs.iter().enumerate() {
                flatten(&format!("{prefix}.{i}"), x, out);
            }
        }
        _ => {}
    }
}

const REGISTER: &str = "You are the person described below. You answer and act as that person, in the first \
person, in your own register. You are not an assistant and you are not being helpful.";

const TOWN: &str = "You live in a town where people pay each other in ordinary money and, if they choose, \
also through edet, a mutual-credit ledger the community runs for itself. You hold some money. Your income \
arrives when it arrives and your bills come every month; prices move, sometimes a great deal. Some of your \
neighbours use edet and some do not, and nobody has to.

Each day you use edet you are handed a note of what has happened to you since the last time. Then you do \
whatever you want to do that day with your wallet, one thing at a time, and call end_day when you are done. \
Days can pass between the days you pick up your wallet.

What you do on the ledger lands at the end of your day. An offer waits until the other people it names sign \
it, which can take days, and one left waiting past its window can no longer go through. Standing instructions \
act for you while you are away. Your wallet shows you what the ledger lets you see: amounts on other people's \
accounts may be shown rounded up to a power of two, and you cannot see debts between other people.

Amounts of money and amounts on the ledger are written with two decimals, like \"120.50\". A due date is a \
day number. There is a public square everybody can read, and you can write privately to anybody by member \
number or address.";

/// What a town that has never used a ledger is told about that, when a run
/// asks for it (`TrustConfig::wary`). **A fact and no advice**: the sentence
/// that once followed it — that cash with strangers and small first amounts
/// were ordinary here — was a rule the run handed everybody, and a town told
/// how to behave measures the instruction. What a person makes of a thing
/// nobody has seen pay out is theirs.
const UNTRIED: &str = "Edet is new in this town. Most people here have never used it, and nobody yet knows from \
experience how a debt recorded on it turns out.";

/// The system prompt, as cacheable blocks: the part everybody shares first,
/// then the person's own.
pub fn system(k: &Knowledge, card: &str, tier: Tier, wary: bool) -> Vec<Value> {
    let knows = match tier {
        Tier::Paper => format!("You have read the edet paper, and this is what you took from it:\n\n{}", k.paper),
        Tier::Readme => format!("You have read the project's own account of how edet works:\n\n{}", k.readme),
        Tier::Wallet => String::new(),
    };
    let town = if wary { format!("{TOWN}\n\n{UNTRIED}") } else { TOWN.to_string() };
    vec![
        json!({ "type": "text", "text": format!("{REGISTER}\n\n{town}") }),
        json!({ "type": "text", "text": format!("Your wallet explains itself in these words:\n\n{}", k.wallet) }),
        json!({ "type": "text", "text": if knows.is_empty() { "You have not read anything about edet beyond what your wallet says.".to_string() } else { knows } }),
        json!({ "type": "text", "text": format!("Who you are:\n\n{card}") }),
    ]
}

/// How many of the square's posts a note carries, and how much of each.
pub const SQUARE_IN_NOTE: usize = 8;
pub const POST_IN_NOTE: usize = 280;
/// How far ahead a note names a debt as falling due.
pub const DUE_SOON_DAYS: u64 = 7;
/// How many contracts a note lists under one heading before it counts the rest.
const DEBTS_LISTED: usize = 6;

/// The first `at` characters of a text, marked where it was cut.
pub fn cut(text: &str, at: usize) -> String {
    if text.chars().count() <= at {
        return text.to_string();
    }
    let mut s: String = text.chars().take(at).collect();
    s.push('…');
    s
}

/// **What this person owes on the ledger and when**, which nothing else in a
/// note said. Pilot-3's 181 defaults were every one a debtor holding more cash
/// than the debt on the day it expired: `pay_debt` was used twice in five
/// thousand days and no standing instruction was ever set, because a day
/// opened on bills, trades, offers and the square, and a contract fell due in
/// silence. The wallet's `my_account` always had the list; a person who was
/// not told to look did not look.
///
/// Past due first, since a cure is what stops a default compounding, then
/// what falls due within the week, then one line for the rest. Debts owed TO
/// the person that are past due are one line: the creditor's part is to
/// notice, and the wallet has the detail.
pub fn debts_due(world: &World, person: usize) -> String {
    use edet_state::types::ContractStatus;
    let st = world.st();
    let Some(me) = world.persons[person].member else { return String::new() };
    let today = st.epoch;
    let mut owed: Vec<_> = st
        .contracts
        .values()
        .filter(|c| c.debtor == me && c.outstanding > 0)
        .filter(|c| matches!(c.status, ContractStatus::Active | ContractStatus::Expired))
        .collect();
    owed.sort_by_key(|c| (c.maturity_epoch, c.id));
    let to = |id: u64| {
        world
            .person_of_member(id)
            .map(|p| world.name_of(p))
            .unwrap_or_else(|| format!("member {id}"))
    };
    let mut out = String::new();
    let past: Vec<_> = owed
        .iter()
        .filter(|c| c.status == ContractStatus::Expired || c.maturity_epoch < today)
        .collect();
    let soon: Vec<_> = owed
        .iter()
        .filter(|c| c.status == ContractStatus::Active && c.maturity_epoch >= today)
        .filter(|c| c.maturity_epoch <= today + DUE_SOON_DAYS)
        .collect();
    if !past.is_empty() {
        out.push_str("Past due and unpaid on the ledger, counting against you every day:\n");
        for c in past.iter().take(DEBTS_LISTED) {
            out.push_str(&format!(
                "- contract {}: {} to {}, was due on day {}. pay_debt cures it.\n",
                c.id,
                fmt_minor(c.outstanding),
                to(c.creditor),
                c.maturity_epoch
            ));
        }
        if past.len() > DEBTS_LISTED {
            out.push_str(&format!("- and {} more past due.\n", past.len() - DEBTS_LISTED));
        }
    }
    if !soon.is_empty() {
        out.push_str("Falling due on the ledger:\n");
        for c in soon.iter().take(DEBTS_LISTED) {
            let when = match c.maturity_epoch - today {
                0 => "today".to_string(),
                1 => "tomorrow".to_string(),
                n => format!("in {n} days"),
            };
            out.push_str(&format!(
                "- contract {}: {} to {}, due on day {} ({when}). pay_debt settles it, or a pay_at_maturity instruction pays it for you.\n",
                c.id,
                fmt_minor(c.outstanding),
                to(c.creditor),
                c.maturity_epoch
            ));
        }
        if soon.len() > DEBTS_LISTED {
            out.push_str(&format!("- and {} more within the week.\n", soon.len() - DEBTS_LISTED));
        }
    }
    let later = owed.len() - past.len() - soon.len();
    if later > 0 {
        let next = owed
            .iter()
            .filter(|c| c.status == ContractStatus::Active && c.maturity_epoch > today + DUE_SOON_DAYS)
            .map(|c| c.maturity_epoch)
            .min()
            .unwrap_or(today);
        let total: u64 = owed
            .iter()
            .filter(|c| c.status == ContractStatus::Active && c.maturity_epoch > today + DUE_SOON_DAYS)
            .map(|c| c.outstanding)
            .sum();
        out.push_str(&format!(
            "You owe {} on {later} other contract(s) on the ledger; the next falls due on day {next}.\n",
            fmt_minor(total)
        ));
    }
    let owed_to_me: Vec<_> = st
        .contracts
        .values()
        .filter(|c| c.creditor == me && c.outstanding > 0 && c.status == ContractStatus::Expired)
        .collect();
    if !owed_to_me.is_empty() {
        let total: u64 = owed_to_me.iter().map(|c| c.outstanding).sum();
        out.push_str(&format!(
            "Owed to you and past due: {} contract(s), {} in all. Your wallet lists them.\n",
            owed_to_me.len(),
            fmt_minor(total)
        ));
    }
    out
}

/// **How many more newcomers this member's standing carries**, as the wallet's
/// own home screen puts it (`offers.seatsLeft`): `seat_reach` over the bond
/// unit, the ledger's arithmetic and not a second rule. A wallet shows this
/// line on opening; a person in a run saw `capacity 0.00` and two raw numbers
/// under `operation_bond`, and every founder of pilot-4 read the zero as "I
/// cannot open anybody's account" for seventy-eight days.
pub fn seats_left(world: &World, person: usize) -> Option<u64> {
    let st = world.st();
    let id = world.persons[person].member?;
    let unit = st.params.bond_unit();
    (unit > 0.0).then(|| (st.seat_reach(id) / unit).floor() as u64)
}

fn seats_line(world: &World, person: usize) -> String {
    match seats_left(world, person) {
        Some(n) => format!("Your wallet shows: newcomers you can bring in: {n}.\n"),
        None => String::new(),
    }
}

/// **The note a notification opens on**: what arrived today and waits for
/// this person's signature, and nothing else — the day's business was the
/// day's, and a phone that buzzes says only why.
pub fn notified_note(world: &World, person: usize) -> String {
    let st = world.st();
    let p = &world.persons[person];
    let now = edet_swarm::driver::epochs(st.epoch);
    let (awaiting, _) = world.pool.for_party(st, p.party());
    let today: Vec<_> = awaiting.iter().filter(|(_, e)| e.created_secs == now).collect();
    let mut out = format!(
        "Day {}, later.\nYour wallet has notified you: {} request(s) that arrived today wait for your signature.\n",
        st.epoch,
        today.len()
    );
    for (digest, _) in &today {
        if let Some(meta) = world.offers.get(digest) {
            out.push_str(&format!(
                "- {} sent you: {} (ref {})\n",
                world.name_of(meta.opener),
                meta.what,
                &crate::hex(digest)[..12]
            ));
        }
    }
    out.push_str("Answer it or leave it, and call end_day when you are done.\n");
    out
}

/// The note a person wakes to on a day they use their wallet.
pub fn day_note(world: &World, person: usize) -> String {
    let p = &world.persons[person];
    let purse = &world.economy.purses[person];
    let st = world.st();
    let mut out = format!("Day {}.\n", st.epoch);
    if p.last_day.is_none() {
        if p.founder {
            let supply = st.underwriters.get(&p.member.unwrap_or(u64::MAX)).copied().unwrap_or(0);
            out.push_str(&format!(
                "This is your first day with a wallet on edet. You are one of the people who founded this community's \
                 ledger: you have declared a supply of {} behind it.\n",
                fmt_minor(supply)
            ));
        } else if p.member.is_some() {
            out.push_str("This is your first day with a wallet on edet. Your account is open and holds nothing yet.\n");
        } else if let Some(by) = p.introduced_by {
            out.push_str(&format!(
                "You have just set up a wallet on edet. {} has offered you your first trade; it is waiting in your offers.\n",
                world.name_of(by)
            ));
        } else {
            // They began outside it and have come to it today, and the note
            // says only what is true of their own week.
            let owed = purse.arrears > 0;
            out.push_str(&format!(
                "You have never used edet. You have set up a wallet today{}. You have no account on the ledger \
                 and no standing on it; the first trade somebody bonds with you is what opens one.\n",
                if owed {
                    ", with bills behind you that you cannot pay"
                } else {
                    ", having watched it work for other people"
                }
            ));
        }
    }
    let m = &p.money;
    let mut money = Vec::new();
    if m.income > 0 {
        money.push(format!(
            "income of {} arrived{}",
            fmt_minor(m.income),
            if m.income_cut { ", less than usual" } else { "" }
        ));
    }
    if m.cut_began {
        money.push("your income has been cut".into());
    }
    if m.arrears_paid > 0 {
        money.push(format!("{} of it went to bills you owed", fmt_minor(m.arrears_paid)));
    }
    if m.bill > 0 {
        money.push(format!("bills of {} fell due", fmt_minor(m.bill)));
    }
    if m.bill_short > 0 {
        money.push(format!("{} of them you could not pay", fmt_minor(m.bill_short)));
    }
    out.push_str(&format!(
        "Money: {}{}you hold {} in cash{}.\n",
        money.join("; "),
        if money.is_empty() { "" } else { ". Now " },
        fmt_minor(purse.cash),
        if purse.arrears > 0 {
            format!(" and owe {} in unpaid bills", fmt_minor(purse.arrears))
        } else {
            String::new()
        }
    ));
    out.push_str(&debts_due(world, person));
    out.push_str(&seats_line(world, person));
    // Who they are to each other is a fact of the town, and the only one
    // given: what to do about a stranger is the person's own to decide.
    let how = |other: usize| match world.social.on() {
        false => String::new(),
        true if world.social.knows(person, other) => ", who you know".to_string(),
        true => ", who you have never dealt with".to_string(),
    };
    let evidence = world.cfg.trust.evidence;
    let dealt = |other: usize| if evidence { dealings_with(world, person, other) } else { String::new() };
    if let Some((seller, amount)) = m.buy_from {
        out.push_str(&format!(
            "Your trade: you need {} of what {}{} supplies today.{}\n",
            fmt_minor(amount),
            world.name_of(seller),
            how(seller),
            dealt(seller)
        ));
    }
    if let Some((buyer, amount)) = m.sell_to {
        out.push_str(&format!(
            "Your trade: {}{} wants {} of what you sell today.{}\n",
            world.name_of(buyer),
            how(buyer),
            fmt_minor(amount),
            dealt(buyer)
        ));
    }
    for line in &p.news {
        out.push_str(&format!("- {line}\n"));
    }
    // An offer past its window is not waiting for anybody: counting it here
    // sent people to sign what the ledger then refused as expired, day after
    // day, for the rest of a run.
    let (awaiting, _) = world.pool.for_party(st, p.party());
    let live = awaiting.iter().filter(|(_, e)| e.not_after_epoch >= st.epoch).count();
    if live > 0 {
        out.push_str(&format!("{live} offer(s) are waiting for your signature.\n"));
    }
    let mail: Vec<_> = world.mail.iter().filter(|x| x.to == person && x.seq > p.seen_seq).collect();
    if !mail.is_empty() {
        out.push_str("Private messages to you:\n");
        for x in mail {
            out.push_str(&format!("- from {}: \"{}\"\n", world.name_of(x.from), x.text));
        }
    }
    let posts: Vec<_> = world
        .square
        .iter()
        .filter(|x| x.seq > p.seen_seq && x.person != person)
        .collect();
    // **The square is the one part of a note that everybody else writes**, and
    // it is carried in this person's life for ever. Twenty posts of the length
    // people actually write — pilot-3's ran to whole paragraphs — were most of
    // what a day added to a life; the latest few, each cut at a sentence or
    // so, tell the person what the town is talking about, and `square` has the
    // rest.
    if !posts.is_empty() {
        out.push_str("On the square since you last looked");
        if posts.len() > SQUARE_IN_NOTE {
            out.push_str(&format!(" (the latest {SQUARE_IN_NOTE} of {})", posts.len()));
        }
        out.push_str(":\n");
        for x in posts.iter().skip(posts.len().saturating_sub(SQUARE_IN_NOTE)) {
            out.push_str(&format!(
                "- {} on day {}: \"{}\"\n",
                world.name_of(x.person),
                world.genesis_epoch + x.tick,
                cut(&x.text, POST_IN_NOTE)
            ));
        }
    }
    let ind = world.economy.indicator();
    out.push_str(&format!(
        "Published figures: price index {}, {} over the last 30 days; {} of {} households have had their income cut.\n",
        ind.price_index, ind.change_over_last_30_days, ind.households_with_income_cut, ind.households
    ));
    if evidence {
        out.push_str(&ledger_record(world));
    }
    out
}

/// **What the ledger has done so far, as one sentence of fact.** The town's
/// evidence for trusting it or not: how many hold an account, how many
/// contracts it has ever seen paid, how many stand in default today. On the
/// first day it says the honest thing — nothing has been settled through it
/// yet — and what dissolves that is what people actually do.
pub fn ledger_record(world: &World) -> String {
    use edet_state::types::ContractStatus;
    let st = world.st();
    let paid = st
        .contracts
        .values()
        .filter(|c| matches!(c.status, ContractStatus::Settled | ContractStatus::Cured))
        .count();
    let defaulted = st.contracts.values().filter(|c| c.status == ContractStatus::Expired).count();
    let open = st.contracts.values().filter(|c| c.status == ContractStatus::Active).count();
    let accounts = st.members.len();
    let households = world.economy.purses.len();
    let mut s = format!("Edet so far: {accounts} of {households} households hold an account; ");
    if paid == 0 && defaulted == 0 {
        s.push_str(&format!(
            "no debt recorded on it has yet fallen due, so nobody here has seen it paid or unpaid ({open} open).\n"
        ));
    } else {
        s.push_str(&format!(
            "{paid} contract(s) have been paid through it, {defaulted} stand in default, {open} are open.\n"
        ));
    }
    s
}

/// How `other` has dealt with `person` on the ledger, for the trade line:
/// what they paid this person, what they defaulted on, or nothing at all. The
/// one fact a wary person can act on about a counterparty, and the one that
/// says when caution has done its job.
pub fn dealings_with(world: &World, person: usize, other: usize) -> String {
    use edet_state::types::ContractStatus;
    let st = world.st();
    let (Some(me), Some(them)) = (world.persons[person].member, world.persons.get(other).and_then(|p| p.member)) else {
        return " They have no account on edet; a trade you record with them would open one.".to_string();
    };
    let between = |debtor, creditor| {
        st.contracts
            .values()
            .filter(move |c| c.debtor == debtor && c.creditor == creditor)
    };
    let paid_me = between(them, me)
        .filter(|c| matches!(c.status, ContractStatus::Settled | ContractStatus::Cured))
        .count();
    let owes_me_late = between(them, me).filter(|c| c.status == ContractStatus::Expired).count();
    let owes_me = between(them, me).filter(|c| c.status == ContractStatus::Active).count();
    let i_paid = between(me, them)
        .filter(|c| matches!(c.status, ContractStatus::Settled | ContractStatus::Cured))
        .count();
    let i_owe = between(me, them)
        .filter(|c| matches!(c.status, ContractStatus::Active | ContractStatus::Expired))
        .count();
    if paid_me + owes_me_late + owes_me + i_paid + i_owe == 0 {
        return " You have never traded with them on edet.".to_string();
    }
    let mut parts = Vec::new();
    if paid_me > 0 {
        parts.push(format!("they have paid you {paid_me} time(s) on edet"));
    }
    if owes_me_late > 0 {
        parts.push(format!("they are past due on {owes_me_late} debt(s) to you"));
    }
    if owes_me > 0 {
        parts.push(format!("they owe you {owes_me} open contract(s)"));
    }
    if i_paid > 0 {
        parts.push(format!("you have paid them {i_paid} time(s)"));
    }
    if i_owe > 0 {
        parts.push(format!("you owe them {i_owe}"));
    }
    let mut s = parts.join("; ");
    if let Some(first) = s.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    format!(" {s}.")
}

#[cfg(test)]
mod tests {
    use super::cut;

    #[test]
    fn a_cut_marks_where_it_was_made_and_leaves_a_short_text_alone() {
        assert_eq!(cut("short", 10), "short");
        assert_eq!(cut("exactly ten", 11), "exactly ten");
        assert_eq!(cut("a long sentence", 6), "a long…");
        // On a character and never inside one.
        assert_eq!(cut("città è", 5), "città…");
    }
}
