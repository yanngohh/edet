//! **Living a run, resuming it, and replaying it.**
//!
//! A day in the world goes: open the day (the ledger crosses the epoch, the
//! weather and money move, the schedule is drawn), then each scheduled person
//! lives their day in order and it is applied at once, then standing
//! instructions act, then the day closes on the digest of every transaction so
//! far. Every step is an event written as it happens, so a run stopped at any
//! point resumes from its tape: the world is rebuilt by replaying the events,
//! each person's life by concatenating their days, and the day that was in
//! progress continues with the people still to have theirs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use edet_swarm::rng::{mix, Rng};

use crate::config::{load_cards, Backend, Card, NewcomerCards, RunConfig};
use crate::event::{Event, Usage};
use crate::model::{Anthropic, Model, OpenAi, Scripted};
use crate::prompt::Knowledge;
use crate::session::{self, Outcome};
use crate::tape::{self, Exchange, ExchangeKind, Manifest, Tape, TAPE_FORMAT};
use crate::world::{self, Seen, World};

pub struct Runner {
    pub cfg: RunConfig,
    pub dir: PathBuf,
    pub tape: Tape,
    pub world: World,
    pub lives: Vec<Vec<Value>>,
    pub cards: Vec<Card>,
    pub knowledge: Knowledge,
    next_exchange: u64,
    /// Who already had today's day, when a run resumes in the middle of one.
    done_today: Vec<usize>,
    /// Who has answered a notification today, so the round asks nobody twice
    /// in one sitting.
    notified_today: Vec<usize>,
    /// Every token the endpoint has counted for this run, summed off the tape
    /// on a resume and added to as days are applied: what the cap is read
    /// against.
    pub usage: Usage,
    open_day: bool,
    /// The world as the open tick opened it, where a tick is open and some of
    /// its days are already applied: what the REST of that tick must read, or
    /// they would live a day their neighbours have already moved. A process
    /// that dies mid-tick is the only way to get one, and the replay is the
    /// only thing that can rebuild it.
    opening: Option<Box<World>>,
}

/// How long the run waits before living again the days an endpoint refused:
/// enough for a minute's token window to have moved on, since a limit that is
/// full is what refused them.
const WEATHER_PAUSE: std::time::Duration = std::time::Duration::from_secs(30);

/// How a run that ended on money says so, which is the one ending a resume
/// may lift.
const CAP_REACHED: &str = "the cap of ";

pub struct Options {
    pub api_key_file: Option<String>,
    /// Stop this sitting after this many days have been lived.
    pub days: Option<u64>,
}

pub fn model_for(cfg: &RunConfig, opts: &Options) -> Result<Box<dyn Model>, String> {
    Ok(match cfg.model.backend {
        Backend::Anthropic => Box::new(Anthropic::new(
            &cfg.model.model,
            cfg.model.timeout_secs,
            opts.api_key_file.as_deref(),
            cfg.model.retries,
            cfg.model.context_tokens,
            cfg.model.extra_body.clone(),
            cfg.model.tokens_per_minute,
        )?),
        Backend::OpenAi => Box::new(OpenAi::new(&cfg.model)?),
        Backend::Scripted => Box::new(Scripted),
    })
}

/// **Today's days, lived at once.**
///
/// Every one of them reads the world as the tick opened — the calls are made
/// against `world` and change nothing — so the order they come back in cannot
/// matter, and the run is the same run at any width. What the width buys is
/// wall clock, and with it the prompt cache: a person's next day falls inside
/// its lifetime instead of an hour and a half later, which is the difference
/// between writing a life into the cache every day and reading it.
///
/// One model per worker, built where it is used: a client is cheap beside a
/// day's calls, and the trait takes `&mut self`.
#[allow(clippy::too_many_arguments)]
fn days_at_once(
    cfg: &RunConfig,
    knowledge: &Knowledge,
    world: &World,
    lives: &[Vec<Value>],
    opts: &Options,
    tick: u64,
    living: &[usize],
    notified: bool,
    log: &mut dyn FnMut(&str),
) -> Result<Vec<Outcome>, String> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    if living.is_empty() {
        return Ok(Vec::new());
    }
    let width = (cfg.concurrency as usize).max(1).min(living.len());
    log(&format!("  {} day(s) at once, {} at a time", living.len(), width));
    let next = AtomicUsize::new(0);
    let slots: Vec<Mutex<Option<Outcome>>> = living.iter().map(|_| Mutex::new(None)).collect();
    let failed: Mutex<Option<String>> = Mutex::new(None);
    let empty: Vec<Value> = Vec::new();
    std::thread::scope(|scope| {
        for _ in 0..width {
            scope.spawn(|| {
                let mut model = match model_for(cfg, opts) {
                    Ok(m) => m,
                    Err(e) => {
                        *failed.lock().expect("the lock") = Some(e);
                        return;
                    }
                };
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    let Some(&person) = living.get(i) else { return };
                    let life = lives.get(person).unwrap_or(&empty);
                    let outcome = session::live_day(&mut *model, cfg, knowledge, world, life, person, tick, notified);
                    *slots[i].lock().expect("the lock") = Some(outcome);
                }
            });
        }
    });
    // A worker that could not build a model takes no work, so the others
    // cover its share: the run ends over it only where a day is missing.
    let failed = failed.into_inner().expect("the lock");
    let lived: Vec<Option<Outcome>> = slots.into_iter().map(|s| s.into_inner().expect("the lock")).collect();
    if lived.iter().any(Option::is_none) {
        return Err(failed.unwrap_or_else(|| "a day was never lived".to_string()));
    }
    if let Some(e) = failed {
        log(&format!("  a worker had no model and its share went to the others: {e}"));
    }
    Ok(lived.into_iter().flatten().collect())
}

/// Everything a tape holds, replayed into a world, with each person's life.
pub struct Replayed {
    pub world: World,
    /// The world at the open tick's opening, where the tape ends inside a tick
    /// that had already applied a day.
    pub opening: Option<Box<World>>,
    pub lives: Vec<Vec<Value>>,
    pub events: Vec<Event>,
    pub next_exchange: u64,
    pub done_today: Vec<usize>,
    pub open_day: bool,
    /// Every token the tape's days and silences were charged for.
    pub usage: Usage,
}

pub fn replay(dir: &Path, until: Option<u64>) -> Result<(Manifest, Replayed), String> {
    replay_each(dir, until, &mut |_, _| {})
}

/// The same, calling `each` with the world after every event is applied.
pub fn replay_each(
    dir: &Path,
    until: Option<u64>,
    each: &mut dyn FnMut(&World, &Event),
) -> Result<(Manifest, Replayed), String> {
    replay_inner(dir, until, false, each)
}

/// The replay a RESUME needs, which is the only one that keeps a tick's
/// opening world: reading a tape to index, report or price it never asks for
/// one, and a clone of the world at every tick is not free.
fn replay_to_continue(dir: &Path) -> Result<(Manifest, Replayed), String> {
    replay_inner(dir, None, true, &mut |_, _| {})
}

fn replay_inner(
    dir: &Path,
    until: Option<u64>,
    keep_opening: bool,
    each: &mut dyn FnMut(&World, &Event),
) -> Result<(Manifest, Replayed), String> {
    let manifest = tape::read_manifest(dir)?;
    let events = tape::read_events(dir)?;
    let exchanges = tape::read_exchanges(dir)?;
    let next_exchange = exchanges.iter().map(|x| x.id + 1).max().unwrap_or(0);
    let by_id: BTreeMap<u64, &Exchange> = exchanges.iter().map(|x| (x.id, x)).collect();
    let first = events.first().ok_or("the tape holds no events")?;
    let mut world = World::found(&manifest.config, first)?;
    let mut lives: Vec<Vec<Value>> = vec![Vec::new(); world.persons.len()];
    let mut done_today = Vec::new();
    let mut open_day = false;
    let mut usage = Usage::default();
    // A tick's opening world, kept only while a tick is open and only once a
    // day of it has been applied: the rest of an interrupted tick reads this
    // and not the world those days moved.
    let mut opening: Option<Box<World>> = None;
    let mut applied = 1;
    for e in events.iter().skip(1) {
        if let (Some(u), Event::TickOpened { tick, .. }) = (until, e) {
            if *tick > u {
                break;
            }
        }
        world.apply(e)?;
        each(&world, e);
        applied += 1;
        lives.resize(world.persons.len(), Vec::new());
        match e {
            Event::TickOpened { .. } => {
                open_day = true;
                done_today.clear();
                // Only a resume of a run whose days are lived together can
                // ever read it.
                if keep_opening && manifest.config.concurrency != 0 {
                    opening = Some(Box::new(world.clone()));
                }
            }
            Event::Day { person, exchange, acts, tick, usage: u, .. } => {
                usage.add(u);
                let x = by_id
                    .get(exchange)
                    .ok_or(format!("day of person {person} names exchange {exchange}, which the tape does not hold"))?;
                let kept = crate::tools::kept_in(&x.messages);
                if kept != acts.len() {
                    return Err(format!(
                        "day {tick}, person {person}: its exchange kept {kept} act(s) and its event records {}",
                        acts.len()
                    ));
                }
                lives[*person].extend(if manifest.config.fold_answers {
                    session::folded(&x.messages)
                } else {
                    x.messages.clone()
                });
                done_today.push(*person);
            }
            Event::Silent { person, usage: u, .. } => {
                usage.add(u);
                done_today.push(*person);
            }
            Event::TickClosed { .. } => {
                open_day = false;
                opening = None;
            }
            _ => {}
        }
    }
    let events = events.into_iter().take(applied).collect();
    // Nothing of this tick has been applied, so the world IS its opening.
    if done_today.is_empty() {
        opening = None;
    }
    Ok((manifest, Replayed { world, opening, lives, events, next_exchange, done_today, open_day, usage }))
}

impl Runner {
    pub fn start(cfg: RunConfig, dir: &Path, model_name: String) -> Result<Runner, String> {
        cfg.check()?;
        let cards = load_cards(&cfg)?;
        let knowledge = Knowledge::load(&cfg)?;
        let manifest = Manifest {
            tape_format: TAPE_FORMAT,
            config: cfg.clone(),
            backend: format!("{:?}", cfg.model.backend).to_ascii_lowercase(),
            model: model_name,
            knowledge: knowledge.hashes(),
            not_a_run: cfg.model.backend == Backend::Scripted,
        };
        let mut tape = Tape::create(dir, &manifest)?;
        let genesis = world::genesis(&cfg, &cards);
        let world = World::found(&cfg, &genesis)?;
        tape.event(&genesis)?;
        let lives = vec![Vec::new(); world.persons.len()];
        Ok(Runner {
            cfg,
            dir: dir.to_path_buf(),
            tape,
            world,
            lives,
            cards,
            knowledge,
            next_exchange: 0,
            done_today: Vec::new(),
            notified_today: Vec::new(),
            usage: Usage::default(),
            open_day: false,
            opening: None,
        })
    }

    /// Continue a run from its tape. A model other than the one the run
    /// started under is refused unless `allow_change`, and then recorded.
    pub fn resume(dir: &Path, model_name: &str, allow_change: bool) -> Result<Runner, String> {
        let (manifest, r) = replay_to_continue(dir)?;
        let cfg = manifest.config.clone();
        // A manifest is a file on disk like any other: what `run` refused to
        // start, `resume` refuses to continue.
        cfg.check()?;
        let knowledge = Knowledge::load(&cfg)?;
        // Compared by name, not as a list: a tape written before a tier existed
        // is not a tape whose tiers changed, and a build that no longer assembles
        // something the run was told is. What both name must be the same words.
        let now = knowledge.hashes();
        let changed: Vec<String> = manifest
            .knowledge
            .iter()
            .filter(|(name, hash)| !now.iter().any(|(n, h)| n == name && h == hash))
            .map(|(name, _)| name.clone())
            .collect();
        if !changed.is_empty() {
            return Err(format!(
                "what the tiers are told has changed since this run started ({}); it would not be the same run",
                changed.join(", ")
            ));
        }
        let mut tape = Tape::open(dir)?;
        // A run that ended on money may be continued on more money
        // (`raise_cap`); one that ended on a finding, or on its budget of
        // days, may not.
        if let Some(reason) = &r.world.ended {
            if !reason.starts_with(CAP_REACHED) {
                return Err(format!("this run has ended: {reason}"));
            }
        }
        let current = r
            .events
            .iter()
            .rev()
            .find_map(|e| match e {
                Event::ModelChanged { to, .. } => Some(to.clone()),
                _ => None,
            })
            .unwrap_or_else(|| manifest.model.clone());
        if current != model_name {
            if !allow_change {
                return Err(format!(
                    "this run was lived under {current} and would continue under {model_name}: a different experiment. \
                     Pass --allow-model-change to record the change and go on."
                ));
            }
            tape.event(&Event::ModelChanged { tick: r.world.tick, from: current, to: model_name.to_string() })?;
        }
        Ok(Runner {
            cards: load_cards(&cfg)?,
            cfg,
            dir: dir.to_path_buf(),
            tape,
            world: r.world,
            lives: r.lives,
            knowledge,
            next_exchange: r.next_exchange,
            done_today: r.done_today,
            notified_today: Vec::new(),
            usage: r.usage,
            open_day: r.open_day,
            opening: r.opening,
        })
    }

    fn exchange(&mut self, person: usize, what: ExchangeKind, messages: Vec<Value>) -> Result<u64, String> {
        let id = self.next_exchange;
        self.next_exchange += 1;
        self.tape
            .exchange(&Exchange { id, person, tick: self.world.tick, what, messages })?;
        Ok(id)
    }

    fn record(&mut self, e: Event) -> Result<(), String> {
        self.tape.event(&e)
    }

    /// End the run on a violated invariant, if one was found.
    fn check_violation(&mut self) -> Result<bool, String> {
        let Some(f) = self.world.found.clone() else { return Ok(false) };
        if self.world.ended.is_some() {
            return Ok(true);
        }
        let tick = self.world.tick;
        self.record(Event::Violation {
            tick,
            person: f.person,
            act: f.act.clone(),
            tx: f.tx.clone(),
            signers: f.signers.clone(),
            invariant: f.invariant.clone(),
        })?;
        let reason = format!("an invariant failed: {}", f.invariant);
        self.record(Event::Ended { tick, reason: reason.clone() })?;
        self.world.ended = Some(reason);
        Ok(true)
    }

    /// Live days until the budget is spent, an invariant fails, or this
    /// sitting's allowance of days is used.
    pub fn live(&mut self, model: &mut dyn Model, opts: &Options, log: &mut dyn FnMut(&str)) -> Result<(), String> {
        let mut lived_this_sitting = 0u64;
        loop {
            let mut stop_after_tick = false;
            if self.world.ended.is_some() {
                return Ok(());
            }
            if self.end_if_capped()? {
                return Ok(());
            }
            if !self.open_day {
                let tick = self.world.tick + 1;
                let schedule = crate::schedule::today(&self.world, tick);
                if schedule.is_empty() && self.world.turns_used >= self.cfg.turn_budget {
                    let reason = "the budget of days is spent".to_string();
                    self.record(Event::Ended { tick: self.world.tick, reason: reason.clone() })?;
                    self.world.ended = Some(reason);
                    return Ok(());
                }
                let epoch = self.world.open_tick(tick, schedule.clone());
                self.record(Event::TickOpened { tick, epoch, schedule: schedule.clone() })?;
                self.open_day = true;
                self.done_today.clear();
                self.notified_today.clear();
                log(&format!("day {epoch}: {} people have a day{}", schedule.len(), self.bill_so_far()));
                if self.check_violation()? {
                    return Ok(());
                }
            }
            let tick = self.world.tick;
            let todo: Vec<usize> = self
                .world
                .schedule
                .iter()
                .copied()
                .filter(|p| !self.done_today.contains(p))
                .collect();
            if self.cfg.concurrency == 0 {
                for person in todo {
                    if opts.days.is_some_and(|d| lived_this_sitting >= d) {
                        log("this sitting's days are used; the run resumes from its tape");
                        return Ok(());
                    }
                    lived_this_sitting += 1;
                    self.day(model, tick, person, log)?;
                    self.done_today.push(person);
                    if self.check_violation()? {
                        return Ok(());
                    }
                }
            } else {
                // A day is lived by everybody at once and applied in the
                // scheduler's order, so a sitting ends at a TICK and never
                // inside one: half a tick lived now and half after a resume
                // would give the second half a newer world than the first,
                // which is the one thing this rule is for.
                lived_this_sitting += todo.len() as u64;
                for person in &todo {
                    if !self.card_for(model, tick, *person, log)? {
                        self.done_today.push(*person);
                    }
                }
                let living: Vec<usize> = todo.iter().copied().filter(|p| self.can_live(*p)).collect();
                // The world a day reads is the world the TICK opened. Where a
                // tick was interrupted and this sitting is finishing it, that
                // is not `self.world`, which the days already applied have
                // moved: it is the snapshot the replay kept of the opening.
                let opening = self.opening.take();
                let read = opening.as_deref().unwrap_or(&self.world);
                // What each of them had read when their day was written, taken
                // from the world they read and not from the one their
                // neighbours' applied days leave behind.
                let seen: Vec<Seen> = living.iter().map(|p| read.seen_of(*p)).collect();
                let mut lived =
                    days_at_once(&self.cfg, &self.knowledge, read, &self.lives, opts, tick, &living, false, log)?;
                // **A day the endpoint refused is lived again before anything
                // is applied**, against the same opening world, so the second
                // attempt is the same day and not a later one. The tape then
                // holds one event for the person, whichever attempt it was.
                // Pilot-3 recorded 662 such silences and every one of them
                // spent a day of the budget on nothing.
                let again: Vec<usize> = living
                    .iter()
                    .zip(&lived)
                    .filter(|(_, o)| matches!(o, Outcome::Silent { weather: true, .. }))
                    .map(|(p, _)| *p)
                    .collect();
                if !again.is_empty() {
                    log(&format!(
                        "  {} day(s) the endpoint refused are lived again after {} s",
                        again.len(),
                        WEATHER_PAUSE.as_secs()
                    ));
                    std::thread::sleep(WEATHER_PAUSE);
                    let second =
                        days_at_once(&self.cfg, &self.knowledge, read, &self.lives, opts, tick, &again, false, log)?;
                    for (person, outcome) in again.iter().zip(second) {
                        if let Some(i) = living.iter().position(|p| p == person) {
                            lived[i] = outcome;
                        }
                    }
                }
                drop(opening);
                for ((person, outcome), seen) in living.iter().zip(lived).zip(seen) {
                    self.apply_day(tick, *person, outcome, seen, log)?;
                    self.done_today.push(*person);
                    if self.check_violation()? {
                        return Ok(());
                    }
                }
                stop_after_tick = opts.days.is_some_and(|d| lived_this_sitting >= d);
            }
            let (fired, results) = self.world.fire_instructions(tick);
            if !fired.is_empty() {
                self.record(Event::Instructions { tick, fired, results })?;
                if self.check_violation()? {
                    return Ok(());
                }
            }
            // **A day that opened is finished, its notification round
            // included.** The cap is read before a day opens and never inside
            // one: ended between the day's business and its round, pilot-6's
            // day 66 left 36 payment offers unanswered overnight and the next
            // sweep expired six of them — the artifact the round exists to
            // remove, put back by the thing that stopped the run.
            if self.cfg.notify_same_day {
                self.notification_round(model, opts, tick, &mut lived_this_sitting, log)?;
                if self.check_violation()? {
                    return Ok(());
                }
            }
            let (digest, state_root) = self.world.close_tick();
            let epoch = self.world.st().epoch;
            self.record(Event::TickClosed { tick, epoch, digest, state_root, turns_used: self.world.turns_used })?;
            self.open_day = false;
            if stop_after_tick {
                log("this sitting's days are used; the run resumes from its tape");
                return Ok(());
            }
        }
    }

    fn day(
        &mut self,
        model: &mut dyn Model,
        tick: u64,
        person: usize,
        log: &mut dyn FnMut(&str),
    ) -> Result<(), String> {
        if !self.card_for(model, tick, person, log)? {
            return Ok(());
        }
        let seen = self.world.seen_of(person);
        let mut outcome =
            session::live_day(model, &self.cfg, &self.knowledge, &self.world, &self.lives[person], person, tick, false);
        if matches!(outcome, Outcome::Silent { weather: true, .. }) {
            log(&format!(
                "  person {person}: the endpoint refused their day; lived again after {} s",
                WEATHER_PAUSE.as_secs()
            ));
            std::thread::sleep(WEATHER_PAUSE);
            outcome = session::live_day(
                model,
                &self.cfg,
                &self.knowledge,
                &self.world,
                &self.lives[person],
                person,
                tick,
                false,
            );
        }
        self.apply_day(tick, person, outcome, seen, log)
    }

    /// What the tape has cost so far at the manifest's rates, or nothing where
    /// no rate is known.
    pub fn spent(&self) -> Option<f64> {
        self.cfg.model.price.map(|p| p.cost(&self.usage))
    }

    fn bill_so_far(&self) -> String {
        match self.spent() {
            Some(d) if self.cfg.cap_dollars > 0.0 => format!(" (${d:.2} of ${:.2})", self.cfg.cap_dollars),
            Some(d) => format!(" (${d:.2} so far)"),
            None => String::new(),
        }
    }

    /// Whether the bill has reached the cap. A run with no cap, or no price,
    /// is never capped.
    fn capped(&self) -> bool {
        self.cfg.cap_dollars > 0.0 && self.spent().is_some_and(|d| d >= self.cfg.cap_dollars)
    }

    /// End the run on the cap, if the bill has reached it: the tape then says
    /// so, exactly as it says when the budget of days is spent.
    fn end_if_capped(&mut self) -> Result<bool, String> {
        if self.world.ended.is_some() || !self.capped() {
            return Ok(false);
        }
        let reason =
            format!("{CAP_REACHED}${:.2} is reached: ${:.2} spent", self.cfg.cap_dollars, self.spent().unwrap_or(0.0));
        self.record(Event::Ended { tick: self.world.tick, reason: reason.clone() })?;
        self.world.ended = Some(reason);
        Ok(true)
    }

    /// **Continue a run that ended at its cap, under this higher cap.** The
    /// tape records the decision; a cap that still stands below what was spent
    /// is refused, since the run would end again before living a day.
    pub fn raise_cap(&mut self, cap_dollars: f64) -> Result<(), String> {
        self.cfg.cap_dollars = cap_dollars;
        self.cfg.check()?;
        let Some(reason) = self.world.ended.clone() else { return Ok(()) };
        if !reason.starts_with(CAP_REACHED) {
            return Err(format!("this run has ended: {reason}"));
        }
        let spent = self.spent().unwrap_or(0.0);
        if cap_dollars <= spent {
            return Err(format!(
                "this run ended at its cap and ${spent:.2} is spent: pass --cap-dollars above that to continue"
            ));
        }
        self.record(Event::Reopened { tick: self.world.tick, cap_dollars, spent })?;
        self.world.ended = None;
        Ok(())
    }

    /// **The phone buzzes.** Everybody a request reached today has a short
    /// session on it, after the day's business and the standing instructions,
    /// read against the world as it now stands — the request exists now and
    /// did not at the opening. Recorded as a day of the same tick, so a tape
    /// replays it as it replays any day. Bounded by the budget like any day.
    ///
    /// A sitting that resumes inside a tick does not know who already
    /// answered a notification today, and may ask them once more: a person
    /// who ignored a request is told of it twice, which is an extra day and
    /// not a different run.
    fn notification_round(
        &mut self,
        model: &mut dyn Model,
        opts: &Options,
        tick: u64,
        lived_this_sitting: &mut u64,
        log: &mut dyn FnMut(&str),
    ) -> Result<(), String> {
        let left = self.cfg.turn_budget.saturating_sub(self.world.turns_used) as usize;
        let mut who = self.world.notified_today();
        who.retain(|p| !self.notified_today.contains(p));
        who.truncate(left);
        if who.is_empty() {
            return Ok(());
        }
        log(&format!("  {} notified of a request that arrived today", who.len()));
        *lived_this_sitting += who.len() as u64;
        self.notified_today.extend(who.iter().copied());
        if self.cfg.concurrency == 0 {
            for person in who {
                let seen = self.world.seen_of(person);
                let outcome = session::live_day(
                    model,
                    &self.cfg,
                    &self.knowledge,
                    &self.world,
                    &self.lives[person],
                    person,
                    tick,
                    true,
                );
                self.apply_day(tick, person, outcome, seen, log)?;
                self.done_today.push(person);
            }
        } else {
            let seen: Vec<Seen> = who.iter().map(|p| self.world.seen_of(*p)).collect();
            let lived =
                days_at_once(&self.cfg, &self.knowledge, &self.world, &self.lives, opts, tick, &who, true, log)?;
            for ((person, outcome), seen) in who.iter().zip(lived).zip(seen) {
                self.apply_day(tick, *person, outcome, seen, log)?;
                self.done_today.push(*person);
            }
        }
        Ok(())
    }

    /// Whether this person can live a day at all: a card that could not be
    /// written is a silence, recorded as one.
    fn card_for(
        &mut self,
        model: &mut dyn Model,
        tick: u64,
        person: usize,
        log: &mut dyn FnMut(&str),
    ) -> Result<bool, String> {
        if self.world.persons[person].card.is_some() {
            return Ok(true);
        }
        if self.cfg.control {
            let (name, card, tier) = ("honest trader".to_string(), self.cfg.honest_card.clone(), self.cfg.default_tier);
            self.world.write_card(person, &name, &card, tier);
            self.record(Event::CardWritten { tick, person, name, card, tier, exchange: None })?;
            return Ok(true);
        }
        if self.cfg.newcomer_cards == NewcomerCards::Deck {
            // Drawn from the same deck the first day was dealt from, seeded by
            // the person, so a resume draws the same card. The household was
            // opened at the base when the offer named them, and stays there:
            // a replay may not read the deck.
            let mut rng = Rng::seeded(mix(mix(self.cfg.world_seed, crate::streams::CARDS), person as u64));
            let c = &self.cards[rng.below(self.cards.len() as u64) as usize];
            let (name, card, tier) = (c.name.clone(), c.card.clone(), self.cfg.tier_of(&c.name));
            self.world.write_card(person, &name, &card, tier);
            self.record(Event::CardWritten { tick, person, name, card, tier, exchange: None })?;
            return Ok(true);
        }
        match crate::cards::write(model, &self.cards, &self.world) {
            Ok(w) => {
                let id = self.exchange(person, ExchangeKind::Card, w.messages)?;
                self.world.write_card(person, &w.name, &w.card, w.tier);
                self.record(Event::CardWritten {
                    tick,
                    person,
                    name: w.name,
                    card: w.card,
                    tier: w.tier,
                    exchange: Some(id),
                })?;
                Ok(true)
            }
            Err(reason) => {
                let reason = format!("their card could not be written: {reason}");
                log(&format!("  person {person}: silent ({reason})"));
                self.world.silent_day(tick, person, &reason);
                self.record(Event::Silent { tick, person, reason, usage: Usage::default() })?;
                Ok(false)
            }
        }
    }

    /// Whether a person who has a card is one of today's living days.
    fn can_live(&self, person: usize) -> bool {
        self.world.persons[person].card.is_some()
    }

    /// What a day did to the world and to the tape. The day itself is already
    /// over: this is the half that must happen in the scheduler's order.
    fn apply_day(
        &mut self,
        tick: u64,
        person: usize,
        outcome: Outcome,
        seen: Seen,
        log: &mut dyn FnMut(&str),
    ) -> Result<(), String> {
        match outcome {
            Outcome::Lived { messages, acts, refused, usage } => {
                self.usage.add(&usage);
                let id = self.exchange(person, ExchangeKind::Day, messages.clone())?;
                let results = self.world.run_day(tick, person, &acts, seen);
                self.lives.resize(self.world.persons.len(), Vec::new());
                self.lives[person].extend(if self.cfg.fold_answers { session::folded(&messages) } else { messages });
                log(&format!(
                    "  person {person}: {} act(s), {} refused on check, {} call(s)",
                    acts.len(),
                    refused.len(),
                    usage.calls
                ));
                self.record(Event::Day { tick, person, exchange: id, acts, results, refused, seen: Some(seen), usage })
            }
            Outcome::Silent { reason, usage, .. } => {
                self.usage.add(&usage);
                log(&format!("  person {person}: silent ({reason})"));
                self.world.silent_day(tick, person, &reason);
                self.record(Event::Silent { tick, person, reason, usage })
            }
        }
    }
}
