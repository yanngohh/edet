//! **The apparatus with no model in it.** A scripted town is lived, its tape
//! replayed, and the two things a person is handed that a model never sees are
//! read off the result: the note that opens a day, and the life the call
//! carries. Nothing here is a claim about what people do — the scripted policy
//! is a fixed policy — only that the harness tells them what it says it tells
//! them.

use edet_civitas::config::{Backend, RunConfig};
use edet_civitas::model::{Model, Scripted};
use edet_civitas::prompt::day_note;
use edet_civitas::run::{replay, Options, Runner};
use edet_civitas::session::compact;

fn config() -> RunConfig {
    let mut cfg = RunConfig::default();
    cfg.model.backend = Backend::Scripted;
    cfg.repo = concat!(env!("CARGO_MANIFEST_DIR"), "/../..").into();
    cfg.population = 12;
    cfg.founders = 4;
    cfg.adopters_at_genesis = Some(6);
    cfg.turn_budget = 400;
    cfg.target_ticks = 45;
    cfg.concurrency = 2;
    cfg.days_kept_whole = 5;
    cfg
}

#[test]
fn a_scripted_town_is_told_what_falls_due_and_carries_its_old_days_as_memory() {
    let dir = std::env::temp_dir().join(format!("edet-civitas-apparatus-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut model = Scripted;
    let mut runner = Runner::start(config(), &dir, model.name()).expect("a scripted run starts");
    let opts = Options { api_key_file: None, days: None };
    runner
        .live(&mut model, &opts, &mut |_| {})
        .expect("the run lives to the end of its budget");
    let (_, r) = replay(&dir, None).expect("the tape replays");
    let world = r.world;

    // **The note names what falls due.** The scripted policy lends for thirty
    // days from the first day, so by the end of the budget some member owes
    // something that is due within the week, past due, or further off — and
    // whichever it is, the note says so where pilot-3's said nothing.
    assert!(world.st().contracts.values().any(|c| c.outstanding > 0), "the scripted town booked credit");
    let notes: Vec<String> = world
        .persons
        .iter()
        .filter(|p| p.member.is_some())
        .map(|p| day_note(&world, p.index))
        .collect();
    let told = notes.iter().any(|n| {
        n.contains("Falling due on the ledger")
            || n.contains("Past due and unpaid on the ledger")
            || n.contains("other contract(s) on the ledger")
    });
    assert!(told, "no member's note names a debt:\n{}", notes.join("\n---\n"));
    // A debtor's line names the tool that answers it.
    if let Some(n) = notes.iter().find(|n| n.contains("Falling due on the ledger")) {
        assert!(n.contains("pay_debt"), "{n}");
    }

    // **The town is told what the ledger has done so far**, and a trade line
    // says how that counterparty has dealt with this person on it. Trust is
    // theirs to give; the evidence is the run's to state.
    assert!(notes.iter().all(|n| n.contains("Edet so far: ")), "a note without the ledger's record");
    let trades: Vec<&String> = notes.iter().filter(|n| n.contains("Your trade: ")).collect();
    assert!(!trades.is_empty(), "no member met a trade");
    assert!(
        // Every trade line ends "today." and a sentence of dealings follows it.
        trades.iter().all(|n| n
            .lines()
            .filter(|l| l.starts_with("Your trade: "))
            .all(|l| { l.contains("today. ") && l.trim_end().ends_with('.') })),
        "a trade line without the counterparty's dealings:\n{}",
        trades.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n")
    );
    // A founder's note carries the wallet's own count of the newcomers their
    // standing can seat, which is the line a real wallet opens on.
    let founders: Vec<String> = world
        .persons
        .iter()
        .filter(|p| p.founder)
        .map(|p| day_note(&world, p.index))
        .collect();
    assert!(
        founders
            .iter()
            .any(|n| n.contains("newcomers you can bring in: ") && !n.contains("newcomers you can bring in: 0.")),
        "no founder is told how many newcomers they can seat:\n{}",
        founders.join("\n---\n")
    );
    // The system prompt carries the town's wariness only when asked for it.
    let k = edet_civitas::prompt::Knowledge::load(&config()).expect("the knowledge loads");
    let wary = edet_civitas::prompt::system(&k, "", edet_civitas::config::Tier::Wallet, true);
    let plain = edet_civitas::prompt::system(&k, "", edet_civitas::config::Tier::Wallet, false);
    assert!(wary[0]["text"].as_str().unwrap().contains("Edet is new in this town"));
    assert!(!plain[0]["text"].as_str().unwrap().contains("Edet is new in this town"));

    // **The phone buzzes.** With `notify_same_day` on, somebody a request
    // reached during the day has a second session on the same tick, and the
    // tape holds it as a day like any other.
    let mut seen = std::collections::BTreeSet::new();
    let mut twice = 0;
    for e in &r.events {
        if let edet_civitas::event::Event::Day { tick, person, .. } = e {
            if !seen.insert((*tick, *person)) {
                twice += 1;
            }
        }
    }
    assert!(twice > 0, "nobody was notified of a request the day it arrived");

    // **A long life is sent as memory before the window and whole inside it.**
    let life = r.lives.iter().max_by_key(|l| l.len()).expect("somebody lived");
    let sent = compact(life, 5);
    assert!(sent.len() < life.len(), "a life of {} messages compacted to {}", life.len(), sent.len());
    let first = sent[0]["content"][0]["text"].as_str().expect("a remembered day is text");
    assert!(first.starts_with("Day ") && first.contains(", as you remember it"), "{first}");
    // Every tool result the call carries answers a call the call carries.
    let mut asked = std::collections::BTreeSet::new();
    for m in &sent {
        for b in m["content"].as_array().into_iter().flatten() {
            match b["type"].as_str() {
                Some("tool_use") => {
                    asked.insert(b["id"].to_string());
                }
                Some("tool_result") => {
                    assert!(asked.contains(&b["tool_use_id"].to_string()), "an orphaned tool result: {b}");
                }
                _ => {}
            }
        }
    }
    // The same life compacts the same way on a resume.
    assert_eq!(sent, compact(life, 5));
    std::fs::remove_dir_all(&dir).ok();
}

/// **A neighbour named by address is seated by that first trade.** Under the
/// older `side` an address with no row was refused at the tool with "that
/// person has no account yet", against a note that said a trade recorded
/// with them would open one; every seat of three pilots was a stranger
/// minted by `new`.
#[test]
fn a_neighbour_named_by_address_is_seated_by_that_first_trade() {
    use edet_civitas::acts::ActResult;
    use edet_civitas::tools::Day;
    use edet_civitas::world::{genesis, World};
    use serde_json::json;

    let cfg = config();
    let cards = edet_civitas::config::load_cards(&cfg).expect("the deck loads");
    let mut world = World::found(&cfg, &genesis(&cfg, &cards)).expect("the town is founded");
    world.open_tick(1, vec![0, 6]);
    let errors = std::collections::BTreeMap::new();
    let outsider = 6;
    assert!(world.persons[outsider].member.is_none(), "person 6 is present without an account");
    let address = world.persons[outsider].address();

    // The founder names them by address, as the note tells them they can.
    let mut day = Day::new(&world, &errors, 0, 1);
    let a =
        day.call("offer_sale", &json!({ "counterparty": address, "you_are": "seller", "amount": "20.00", "days": 30 }));
    assert!(!a.is_error, "{}", a.text);
    let seen = world.seen_of(0);
    let results = world.run_day(1, 0, &day.acts, seen);
    assert!(matches!(results.as_slice(), [ActResult::Opened { .. }]), "{results:?}");
    // The neighbour is told who sent it, by the name the town has for them.
    let news = world.persons[outsider].news.join("\n");
    assert!(news.contains("sent you an offer: member 0 sells to"), "{news}");
    assert!(news.contains(&address), "{news}");

    // The neighbour signs it from their own wallet, and the row is theirs.
    let mut theirs = Day::new(&world, &errors, outsider, 1);
    let offers: serde_json::Value = serde_json::from_str(&theirs.call("offers", &json!({})).text).expect("a view");
    let reference = offers["awaiting_me"][0]["ref"]
        .as_str()
        .expect("the offer waits for them")
        .to_string();
    let s = theirs.call("sign_offer", &json!({ "ref": reference }));
    assert!(!s.is_error, "{}", s.text);
    let seen = world.seen_of(outsider);
    let results = world.run_day(1, outsider, &theirs.acts, seen);
    let seated = match results.as_slice() {
        [ActResult::Applied { seated, .. }] => seated.clone(),
        other => panic!("the trade did not book: {other:?}"),
    };
    assert_eq!(seated.len(), 1, "one row seated");
    assert_eq!(world.persons[outsider].member, Some(seated[0]), "the row is the neighbour's");
    assert!(world.persons[outsider].introduced_by.is_none(), "nobody was minted for it");

    // Named again, they are a member now, and the offer says so.
    let mut again = Day::new(&world, &errors, 0, 1);
    let b = again
        .call("offer_sale", &json!({ "counterparty": address, "you_are": "seller", "amount": "5.00", "days": 30 }));
    assert!(!b.is_error, "{}", b.text);
    assert!(
        again
            .acts
            .iter()
            .any(|a| matches!(a, edet_civitas::acts::Act::Offer { ask, .. } if ask.person().is_none())),
        "a member is named as a member"
    );
}

/// **A minted stranger's card is dealt from the deck, seeded**, when the
/// configuration says so: no model writes it, and the same person draws the
/// same card on a resume.
#[test]
fn a_minted_stranger_s_card_is_dealt_from_the_deck() {
    use edet_civitas::config::NewcomerCards;
    use edet_civitas::event::Event;

    let dir = std::env::temp_dir().join(format!("edet-civitas-deck-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut cfg = config();
    cfg.newcomer_cards = NewcomerCards::Deck;
    cfg.turn_budget = 150;
    let cards = edet_civitas::config::load_cards(&cfg).expect("the deck loads");
    let mut model = Scripted;
    let mut runner = Runner::start(cfg, &dir, model.name()).expect("a scripted run starts");
    let opts = Options { api_key_file: None, days: None };
    runner.live(&mut model, &opts, &mut |_| {}).expect("the run lives");
    let (_, r) = replay(&dir, None).expect("the tape replays");
    let written: Vec<(usize, String, Option<u64>)> = r
        .events
        .iter()
        .filter_map(|e| match e {
            Event::CardWritten { person, name, exchange, .. } => Some((*person, name.clone(), *exchange)),
            _ => None,
        })
        .collect();
    assert!(!written.is_empty(), "the scripted policy minted nobody");
    for (person, name, exchange) in &written {
        assert!(exchange.is_none(), "person {person}'s card was written by a model");
        assert!(cards.iter().any(|c| &c.name == name), "person {person} holds {name:?}, which is not in the deck");
    }
    // Every minted stranger is a person the offer created, at the base
    // household, with a card the deck holds.
    assert!(written.iter().all(|(p, _, _)| r.world.persons[*p].introduced_by.is_some()));
    std::fs::remove_dir_all(&dir).ok();
}
