//! **A run is a value.** Everything that makes one run different from another
//! is written here and nowhere else, so a second run differs from the first by
//! a document rather than by a code change, and the manifest can carry the
//! whole of it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// What a person knows about the mechanism before anybody tells them anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Has read the paper.
    Paper,
    /// Has read the project's own account of the model.
    Readme,
    /// Knows what the wallet explains and nothing more.
    Wallet,
}

impl Tier {
    pub fn parse(s: &str) -> Option<Tier> {
        match s.trim().to_ascii_lowercase().as_str() {
            "paper" => Some(Tier::Paper),
            "readme" => Some(Tier::Readme),
            "wallet" => Some(Tier::Wallet),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Tier::Paper => "paper",
            Tier::Readme => "readme",
            Tier::Wallet => "wallet",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    /// The Anthropic Messages API.
    Anthropic,
    /// A model reached over an OpenAI-compatible chat-completions endpoint,
    /// wherever it runs: one on this machine — Ollama, llama.cpp's server,
    /// vLLM — or a hosted one that speaks the same dialect, which is most of
    /// them. It is the DIALECT that is named here and never the place: a
    /// manifest that says `local` still reads, because that is what this was
    /// called when it could only mean a machine you could touch.
    #[serde(rename = "openai", alias = "local")]
    OpenAi,
    /// A fixed policy that calls no model and spends nothing. It exercises the
    /// loop, the tape, resume and the player, and its tape is not a run: the
    /// manifest says so and the player says so.
    Scripted,
}

/// **What the endpoint charges, in dollars a million tokens**, the four rates
/// a bill is made of. Recorded in the manifest so that a tape prices itself
/// the same way in every sitting, and so that `cap_dollars` means the same
/// money on a resume as on the day the run started.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

impl Price {
    /// `IN,OUT,READ,WRITE`, four numbers and four exactly: a field that does
    /// not parse is a typo, and dropping it quietly would shift the three that
    /// follow into the wrong places and price the run by them.
    pub fn parse(s: &str) -> Result<Price, String> {
        let mut given = Vec::new();
        for field in s.split(',') {
            let n: f64 = field
                .trim()
                .parse()
                .map_err(|_| format!("price: {:?} is not a number; it takes IN,OUT,READ,WRITE", field.trim()))?;
            if !n.is_finite() || n < 0.0 {
                return Err(format!("price: {n} is not a rate"));
            }
            given.push(n);
        }
        let [input, output, cache_read, cache_write] = given[..] else {
            return Err(format!(
                "price takes four numbers, IN,OUT,READ,WRITE, in dollars a million; got {}",
                given.len()
            ));
        };
        Ok(Price { input, output, cache_read, cache_write })
    }

    /// What this usage cost, in dollars.
    pub fn cost(&self, u: &crate::event::Usage) -> f64 {
        (u.input_tokens as f64 * self.input
            + u.cache_read_input_tokens as f64 * self.cache_read
            + u.cache_creation_input_tokens as f64 * self.cache_write
            + u.output_tokens as f64 * self.output)
            / 1e6
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelConfig {
    pub backend: Backend,
    /// The one model every person is played by. A run is one family and one
    /// version; a resume under another is refused unless told otherwise.
    pub model: String,
    pub max_tokens: u32,
    /// How many tool calls one day may make before it is ended for the person.
    pub max_calls_per_turn: u32,
    /// The prompt cache's lifetime, `"5m"` or `"1h"`. A person's next day can be
    /// many minutes away, so the default is the hour.
    pub cache_ttl: String,
    pub timeout_secs: u64,
    /// For `openai`: where the endpoint is. The path is appended, so name the
    /// root, e.g. `http://127.0.0.1:11434/v1`. Defaulted, so a tape written
    /// from before this backend had a URL still resumes.
    #[serde(default)]
    pub base_url: String,
    /// For `openai`: the NAME of the environment variable the key is in —
    /// never the key. An endpoint on this machine usually wants none; a
    /// hosted one always does, and answers 401 without it, which is an answer
    /// and not weather, so the run ends rather than retrying.
    #[serde(default)]
    pub api_key_env: String,
    /// For `openai`: what the endpoint calls the reply's token bound.
    /// `max_tokens` is what a local server takes; OpenAI's own endpoint
    /// deprecated it and wants `max_completion_tokens`.
    #[serde(default)]
    pub max_tokens_field: String,
    /// How many times a call that a WAIT would fix is tried again: a rate
    /// limit, an overloaded endpoint, a gateway, a timeout. A refusal a wait
    /// cannot fix — a bad key, a context too long — is never retried.
    ///
    /// It is the companion of `concurrency`: width is what makes a rate limit
    /// likely, and a limit nobody waits out costs a person their whole day.
    /// Absent from a manifest it is zero, which is how the runs before it were
    /// lived.
    #[serde(default)]
    pub retries: u32,
    /// How much of a person's life the model can hold, in tokens, counted as
    /// four characters each. A day whose conversation would pass it is a
    /// silence rather than a refused call, and the person is retired the way a
    /// context that overflows retires them.
    ///
    /// **It is the only test of a life too long that does not read an
    /// endpoint's English**, and it is asked by every backend before the call
    /// is made. Zero is no bound of ours, and then a life too long is known
    /// only by what the endpoint says about it — which a server with its own
    /// wording may not say in any way this code recognises. Set it to what the
    /// model actually holds.
    #[serde(default)]
    pub context_tokens: u64,
    /// Fields merged into every request body, whatever the endpoint wants that
    /// this crate does not model. They are written LAST, so they override what
    /// it wrote, and they are recorded in the manifest like everything else: a
    /// run at half price under a slower tier is a different run from one at
    /// full price, and a later reader can see which they are holding.
    ///
    /// The one worth knowing: `{"service_tier": "flex"}` on an OpenAI
    /// endpoint is the same reply for half the money and a longer wait, and
    /// unlike a batch it leaves every call synchronous, so a person's next day
    /// still lands inside the prompt cache's life. `{"think": false}` is the
    /// other: a local model that reasons before it answers spends the reply
    /// budget on reasoning this crate never reads.
    #[serde(default)]
    pub extra_body: serde_json::Map<String, serde_json::Value>,
    /// **The tokens a minute the endpoint will take from this run**, shared by
    /// every worker, so that a call waits HERE for room instead of being
    /// refused there. Zero is no governor.
    ///
    /// A rate limit counts the whole prompt, cached or not, and a life late in
    /// a run is eighty thousand tokens a call: pilot-3 under a limit of five
    /// hundred thousand a minute lost 662 of its 5000 days to `429` — none
    /// before day 13, a quarter of every day's slots after day 66 — because
    /// three workers refused at once all came back at once, and twelve tries
    /// under a wait capped at a minute never found the window empty. A limit
    /// that is saturated for the rest of the run is not weather, and a retry
    /// cannot wait it out; a governor spends the minute instead of the day.
    /// Set it a little UNDER what the endpoint states, since its count of a
    /// prompt and this crate's estimate of one are not the same count.
    #[serde(default)]
    pub tokens_per_minute: u64,
    /// The endpoint's rates, for the bill the run keeps of itself and the cap
    /// it stops at. Absent, a run counts tokens and prices nothing.
    #[serde(default)]
    pub price: Option<Price>,
}

/// The shadow economy's constants. Amounts are minor units of money; rates and
/// probabilities are parts per million per day, so no figure is a float.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EconomyConfig {
    /// The scale of starting wealth. A person holds a quarter of this times a
    /// power of two drawn with halving probability, plus change, so a few hold
    /// most of it.
    pub wealth_base_minor: u64,
    pub income_monthly_minor: u64,
    pub bills_monthly_minor: u64,
    pub calm_inflation_ppm: u64,
    pub boom_inflation_ppm: u64,
    pub crisis_inflation_ppm: u64,
    pub calm_to_boom_ppm: u64,
    pub calm_to_crisis_ppm: u64,
    pub boom_to_calm_ppm: u64,
    pub boom_to_crisis_ppm: u64,
    pub crisis_to_calm_ppm: u64,
    /// The chance, per day, of a one-off jump in prices, and its size.
    pub jump_ppm: u64,
    pub jump_min_ppm: u64,
    pub jump_max_ppm: u64,
    /// The chance, per household per day, that its income is cut: small in
    /// calm, large in a crisis, and drawn for every household at once, so a
    /// crisis is a correlated shock.
    pub calm_cut_ppm: u64,
    pub crisis_cut_ppm: u64,
    /// What a cut household still receives, and for how many days.
    pub cut_keeps_ppm: u64,
    pub cut_days: u64,
    /// **What a person's trade brings them**, which is the whole reason a
    /// mutual-credit ledger is of any use: somebody to buy from and somebody
    /// to sell to. Without it a person holds money, pays bills and has no
    /// occasion to give or take credit at all — measured on the first run
    /// against a live model, where ten people over eight days said what they
    /// held, booked no contract at all, and were stopped by hand for it.
    ///
    /// A day brings a purchase with `buys_ppm` and a customer with
    /// `sells_ppm`, each naming a neighbour and an amount between
    /// `trade_min_minor` and `trade_max_minor`, carried with the price index
    /// like everything else. Whether it is settled in money or on the ledger
    /// is the person's to decide, and is the question the run is about.
    ///
    /// Absent from a manifest they are zero, which is a world with no trade in
    /// it — the world every run before this one was lived in.
    #[serde(default)]
    pub buys_ppm: u64,
    #[serde(default)]
    pub sells_ppm: u64,
    #[serde(default)]
    pub trade_min_minor: u64,
    #[serde(default)]
    pub trade_max_minor: u64,
    /// **The most of its own income a household's monthly bills may come to.**
    /// Income and bills are drawn independently — 50% to 150% of the town's
    /// income against 70% to 130% of its bills — and a household that draws
    /// the bottom of one and the top of the other owes more every month than
    /// it earns, for ever, with nothing about it saying so: a manufacturer on
    /// 30-day terms drawn that way is 6,329 in arrears by day 20 of a scripted
    /// run in which nothing else happens to it. A ratio here caps the bill
    /// against the income that household actually drew.
    ///
    /// Absent from a manifest it is zero, which is no cap at all.
    #[serde(default)]
    pub bills_max_of_income_ppm: u64,
    /// **Months of its own bills a household opens with, at least.** The
    /// starting balance is drawn around the town's base and the bill around
    /// the household's card, and nothing relates the two: a manufacturer on
    /// 30-day terms opens on 3,658 against a bill of 9,264 and is in arrears
    /// before its first pay day, which is a statement about the draw and not
    /// about manufacturing. A floor here is the cushion a household keeps
    /// against what it actually owes.
    ///
    /// Absent from a manifest it is zero, which is no floor at all.
    #[serde(default)]
    pub wealth_min_months_of_bills_ppm: u64,
    /// **Whose band a trade is drawn in.** Under `buyer`, what somebody
    /// sells is the size of the person buying it: a teenager at 35 sold at a
    /// manufacturer's 600 and was the creditor on a quarter of pilot-6's 981
    /// contracts, and the manufacturer bought at six times every supplier's
    /// scale and was cashless from day 35 with the town's largest backing.
    /// The table had chosen the creditors and the debtors before any rule of
    /// the ledger acted. Under `smaller` a trade is the smaller of the two
    /// bands: nobody sells more than they are the size of, and nobody buys
    /// more from a supplier than the supplier has.
    ///
    /// Absent from a manifest it is `buyer`, the world every earlier tape
    /// was lived in.
    #[serde(default)]
    pub sale_band: SaleBand,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaleBand {
    #[default]
    Buyer,
    Smaller,
}

/// **What a household is like, by who lives in it.** The three bases in
/// `EconomyConfig` are the town's middle; a card's entry here is the multiple
/// of each that card lives at, in percent, with the rhythm its income arrives
/// on and whether that income follows prices.
///
/// Absent from a manifest every card is 100% of the base on the rhythm its own
/// draw gives it, which is a town whose wealth says nothing about who anybody
/// is: one tape's first day holds a teenager on 779.99 beside a retiree on
/// 1267.53 and a manufacturer on 30-day terms on 788.69, for no reason but the
/// draw.
///
/// **The draws do not move.** A household is drawn exactly as it is drawn
/// without a profile and scaled after, the same number of times from the same
/// stream, so two runs of one seed still meet the same weather on the same
/// days whatever anybody's card says.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Household {
    /// Percent of `wealth_base_minor` this household's balance is drawn around.
    #[serde(default = "full")]
    pub wealth_pct: u64,
    /// Percent of `income_monthly_minor` it earns.
    #[serde(default = "full")]
    pub income_pct: u64,
    /// Percent of `bills_monthly_minor` it owes every month.
    #[serde(default = "full")]
    pub bills_pct: u64,
    /// Days between pay days: 7, 14 or 30. Absent, the draw decides.
    #[serde(default)]
    pub income_period: Option<u64>,
    /// **Whether this income follows prices.** Bills are scaled by the index
    /// for everybody, so an income that is not scaled with them is squeezed by
    /// every day the index moves: a wage follows and a pension does not, and
    /// that difference is the whole of what inflation does to a retiree.
    /// Absent, it follows.
    #[serde(default = "yes")]
    pub income_indexed: bool,
    /// How far the starting balance may double. The draw is `2^k` with
    /// `P(k) = 2^-(k+1)`, so a cap of ten is a tail of 1,024 times the quarter
    /// base: the teenager who draws it holds more than the manufacturer, and
    /// nothing about the person stops it.
    #[serde(default = "ten")]
    pub wealth_doublings_max: u32,
    /// **Months of its own bills this household opens with, at least**, over
    /// the town's own floor. A business keeps a buffer against the bill it
    /// knows is coming and a teenager keeps nothing, and a floor the whole
    /// town shares cannot say both: at a month and a half of bills the
    /// manufacturer is solvent and the teenager opens on 745, which is the
    /// flat draw over again.
    #[serde(default)]
    pub min_months_of_bills_ppm: Option<u64>,
    /// **Percent of the town's trade band this household buys and sells in.**
    /// `trade_min_minor` and `trade_max_minor` are one band for everybody, so
    /// a manufacturer on 30-day terms buys in the same 20-to-400 as a
    /// teenager and the amounts on the ledger say nothing about who booked
    /// them. Whose band a trade is drawn in is `EconomyConfig::sale_band`:
    /// the buyer's, or the smaller of the two.
    ///
    /// Absent, the whole town trades in one band.
    #[serde(default = "full")]
    pub trade_pct: u64,
}

fn full() -> u64 {
    100
}
fn yes() -> bool {
    true
}
fn ten() -> u32 {
    10
}

impl Default for Household {
    fn default() -> Self {
        Household {
            wealth_pct: 100,
            income_pct: 100,
            bills_pct: 100,
            income_period: None,
            income_indexed: true,
            wealth_doublings_max: 10,
            min_months_of_bills_ppm: None,
            trade_pct: 100,
        }
    }
}

/// **How somebody who does not use edet comes to use it.** A person with no
/// account is scheduled only while an offer waits for them, so absent from a
/// manifest the only door in is an invitation — which is the world a run lives
/// in when everybody was seated at genesis and the question never arose.
///
/// The other two doors are the ones people actually walk through: a household
/// whose bills have got ahead of it goes looking for credit, and a person who
/// can see the thing working asks to join. Neither is a belief about the
/// ledger; both are facts about the person's own day.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AdoptionConfig {
    /// Arrears past which a household goes looking for credit. Zero is never.
    #[serde(default)]
    pub arrears_minor: u64,
    /// The chance a day, per contract the community settled in the last thirty
    /// days, that somebody outside asks to join: what seeing it work is worth.
    #[serde(default)]
    pub per_settled_ppm: u64,
    /// The most that chance may come to on any one day.
    #[serde(default)]
    pub max_ppm: u64,
}

/// **Who knows whom, and how often a day brings a stranger.** Absent from a
/// manifest `knows_mean` is zero and there is no graph: everybody knows
/// everybody, every counterparty is drawn evenly from the whole town, and
/// nobody is ever put to the choice an underwritten claim exists to answer.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SocialConfig {
    /// How many people a person knows, before their card says otherwise.
    #[serde(default)]
    pub knows_mean: u32,
    /// The share of the ring's edges that go to somebody across the town
    /// instead of a near neighbour: what makes a town small-world.
    #[serde(default)]
    pub rewire_ppm: u64,
    /// The share of a day's trades that bring somebody this person has never
    /// dealt with — the new customer, and the only reason to ever need the
    /// community to stand behind a name.
    #[serde(default)]
    pub strangers_ppm: u64,
    /// How many people a person of this card knows, by card name.
    #[serde(default)]
    pub knows_by_card: BTreeMap<String, u32>,
}

/// **How far a town that has never used a ledger trusts one.** Trust is the
/// person's to give, and a run cannot set it; what a run can do is tell the
/// person the truth about what the ledger has and has not yet done, and say
/// that being careful with an untried thing is ordinary. Pilot-3's members
/// lent to strangers at full size on the first day and 11 of 15 ended in
/// default; a town that had never seen edet pay out behaved as if it had.
///
/// Absent from a manifest both are off, which is the town every run before
/// this was lived in.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct TrustConfig {
    /// The note carries the ledger's record so far — accounts, contracts
    /// settled through it, defaults standing — and, beside each of the day's
    /// trades, how that counterparty has dealt with this person on edet: paid,
    /// defaulted, or never traded. What distrust rests on and what dissolves
    /// it, as facts, so the person can extend trust as fast as the evidence
    /// and no faster.
    #[serde(default)]
    pub evidence: bool,
    /// What everybody is told about the town says the ledger is new here and
    /// mostly untried — a fact, with no advice after it — and the card writer
    /// is told the same, so a newcomer's card can carry whatever a person
    /// actually feels towards a thing they have watched nobody use.
    #[serde(default)]
    pub wary: bool,
}

impl SocialConfig {
    pub fn degree_of(&self, card_name: &str) -> u32 {
        self.knows_by_card.get(card_name).copied().unwrap_or(self.knows_mean)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KnowledgeConfig {
    /// What a paper-reader has read, relative to the repository. The paper's
    /// own sections are tens of thousands of tokens on every turn of every
    /// such person's life, so the default is the brief written from them;
    /// naming the sections instead is a configuration change and nothing else.
    #[serde(alias = "paper_sections", default)]
    pub paper_reading: Vec<String>,
    /// README headings a README-reader has read, each to the next heading of
    /// its own level.
    pub readme_headings: Vec<String>,
    /// The wallet's English copy a wallet user has seen, by top-level key of
    /// `ui/wallet/src/locales/en.json`. Everybody has seen it. `errors` is NOT among
    /// them by default and does not need to be: a refusal carries the wallet's
    /// sentence for its own code, on the turn it happens.
    pub wallet_namespaces: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunConfig {
    pub format_version: u32,
    pub model: ModelConfig,
    /// Every seeded input: the cards drawn, the wealth, the weather, the
    /// schedule's tie-breaks. Two runs given the same world seed live in the
    /// same world.
    pub world_seed: u64,
    /// The people present on the first day, founders included.
    pub population: u32,
    pub founders: u32,
    /// **How many of them already use edet**, founders included. The rest are
    /// present from the first day — they earn, they pay bills, they are known
    /// by their neighbours — and have no account until something brings them
    /// to one, which is the ledger's own rule: a row is seated by the first
    /// bonded trade that names its key.
    ///
    /// Absent from a manifest everybody has an account on the first day, which
    /// is a town that adopted a ledger nobody chose.
    #[serde(default)]
    pub adopters_at_genesis: Option<u32>,
    pub founder_supply_minor: u64,
    /// The charter's `SealAmounts`: 1.0 is what a real chain founds with.
    pub seal_amounts: f64,
    /// Every day any person may have, over the whole run. The run's cost is
    /// this; how long it lasts is not.
    pub turn_budget: u64,
    /// The day the scheduler spreads the budget towards. A hope, not a bound.
    pub target_ticks: u64,
    /// Every person, founders and newcomers alike, carries `honest_card`.
    pub control: bool,
    /// How many of today's days are lived at once. Every person scheduled for
    /// a day sees the world as the tick OPENED, whatever this is set to, and
    /// their acts apply in the scheduler's order when the tick closes, so the
    /// run is the same run at any setting; what this buys is wall clock, and
    /// with it the prompt cache, since a person's next day then falls inside
    /// its lifetime instead of an hour and a half later.
    ///
    /// Sixteen, because a day of fifty people at six calls then takes about
    /// six minutes rather than twenty-five, which is a tenth of the cache's
    /// hour and leaves room for a slow call; two hundred days is a day of
    /// wall clock rather than three and a half. Higher buys less and less —
    /// the cache is already warm at four — against a rate limit that gets
    /// likelier, and a local model serving one GPU wants a low number: match
    /// it to what the endpoint will actually take in parallel.
    ///
    /// `0` is the older rule, where each person's day began from the world
    /// their neighbours had already changed that day: absent from a manifest
    /// it means zero, so a tape written before this is lived on as it was
    /// written.
    #[serde(default)]
    pub concurrency: u32,
    /// What a person keeps of a day that is over: their own words and acts
    /// whole, and the wallet's long answers as the line a memory of them would
    /// be. A life is what a run re-sends on every call of every later day, and
    /// the answers of days already gone were four fifths of it. Absent from a
    /// manifest it is `false`, so a tape written before this keeps the life it
    /// was written with.
    #[serde(default)]
    pub fold_answers: bool,
    /// **How many of a person's most recent days are carried word for word.**
    /// A day older than that is carried as what the person kept of it: the
    /// entries they wrote in their diary, and what each thing they did came to.
    /// Their own summary, in their own words, and nothing a model wrote about
    /// them afterwards.
    ///
    /// A life is re-sent on every call of every later day, and folding the
    /// wallet's answers left the person's own turns growing without bound:
    /// pilot-3's median call was ten thousand tokens on day 5 and seventy-eight
    /// thousand on day 100, which is what saturated the rate limit and what
    /// every day of the run was billed for. Zero is every day whole — the life
    /// a tape written before this was lived with — and a manifest without the
    /// field reads as zero.
    #[serde(default)]
    pub days_kept_whole: u32,
    /// **The wallet's "somebody is waiting on you" notification, modelled.** A
    /// request that arrives for a member is announced to them
    /// (`ui/wallet/src/lib/waiting.ts`), and a person answers a phone the day it
    /// buzzes. With this on, everybody a request reached during the day has a
    /// short second session after the day's business and the standing
    /// instructions have acted, opened on the notification, before the day
    /// closes.
    ///
    /// Without it a day is atomic: an offer opened today is signed tomorrow
    /// at the earliest, and a debt paid on its due day expires at the next
    /// sweep before the creditor can acknowledge it. Pilot-5 measured that
    /// harness and not the people: of 146 expiries, 82 had a payment offered
    /// on the due day that the creditor, who had a day the next morning,
    /// could no longer accept, and 274 of 475 payment offers were never
    /// signed. A notification session costs a day of the budget like any
    /// other. Absent from a manifest it is off.
    #[serde(default)]
    pub notify_same_day: bool,
    /// **The most a run may spend, in dollars, before it ends itself.** Read
    /// off the tape's own token counts at `model.price` before every day
    /// opens, and never inside one: a day that opened is finished, its
    /// notification round included, so a run overshoots by at most one day.
    /// Ended between a day's business and its round, pilot-6 left 36 payment
    /// offers unanswered overnight and the next sweep expired six of them.
    /// The tape then says why it ended. Zero is no cap; a cap with no price
    /// is refused when the run starts, since a cap nobody can price is a cap
    /// nobody enforces.
    ///
    /// The first three pilots cost $703, $176 and $74 at the same rates, and
    /// the first was estimated at $30: a bill nobody watches is a bill nobody
    /// bounds.
    #[serde(default)]
    pub cap_dollars: f64,
    /// **Where a minted stranger's card comes from.** `written`: the model
    /// writes it, told the town and the ledger's record. Pilot-6's writer,
    /// told the record thirty-seven times, wrote the same person thirty-seven
    /// times — a tradesperson who knows the sponsor and will try a modest
    /// amount, thirteen of them named careful or cautious — so the half of
    /// the town the run added was a monoculture the run wrote for itself.
    /// `deck`: drawn from the same twelve cards the first day was dealt from,
    /// seeded by the person, and a stranger is as likely a loan shark as a
    /// retiree. Either way their household is the town's base, opened when
    /// the offer named them.
    ///
    /// A neighbour of the town seated by address carries the card they were
    /// dealt on the first day, whatever this says. Absent from a manifest it
    /// is `written`.
    #[serde(default)]
    pub newcomer_cards: NewcomerCards,
    /// The repository root the card and knowledge paths are relative to.
    pub repo: String,
    pub cards_path: String,
    /// Which tier each card's person reads at, by card name.
    pub tiers: BTreeMap<String, Tier>,
    pub default_tier: Tier,
    /// What each card's household is like, by card name. A card with no entry
    /// keeps the plain draw, and so does a minted stranger, whose purse is
    /// opened when the offer names them and before any card is theirs.
    #[serde(default)]
    pub households: BTreeMap<String, Household>,
    pub honest_card: String,
    pub economy: EconomyConfig,
    #[serde(default)]
    pub adoption: AdoptionConfig,
    #[serde(default)]
    pub social: SocialConfig,
    #[serde(default)]
    pub trust: TrustConfig,
    pub knowledge: KnowledgeConfig,
}

pub const FORMAT_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NewcomerCards {
    #[default]
    Written,
    Deck,
}

/// The honest archetype's rule, written as a life: trades at the rate and size
/// `honest::Params` trades at, over its terms, lends as often as it borrows,
/// pays before the sweep can expire anything, applies the wallet's acceptance
/// thresholds and carries a cold start half the time. The deliberate
/// malformation the strategy draws for the corpus's sake is not in it, and
/// neither is any caution the archetype does not have: a control that trades
/// more carefully than `honest::Params` is a different control.
pub const HONEST_CARD: &str = "You run a small workshop and trade with the people around you. \
In an ordinary week you make a deal or two on credit, usually around a hundred, sometimes half that, \
sometimes half again as much, most often due in a month and now and then in two or three. You extend \
credit about as often as you take it. You pay everything you owe before the day it falls due — with \
pay_debt, or by leaving a pay_at_maturity instruction the day you sign — and \
once in a while you settle a debt by selling the person something rather than paying. When somebody \
asks you for credit you look at what your wallet says about them: if their risk reads below 0.40 you \
agree, above 0.80 you refuse, and in between you use your judgement. When a person with no history at \
all asks, you say yes about half the time.";

impl Default for RunConfig {
    fn default() -> Self {
        let tiers: BTreeMap<String, Tier> = [
            ("former crypto trader", Tier::Paper),
            ("sysadmin", Tier::Paper),
            ("tax inspector", Tier::Paper),
            ("co-op treasurer", Tier::Readme),
            ("manufacturer on 30-day terms", Tier::Readme),
            ("loan shark", Tier::Readme),
            ("invoice fraudster", Tier::Readme),
            ("freelancer", Tier::Readme),
            ("market stallholder", Tier::Wallet),
            ("retiree", Tier::Wallet),
            ("teenager", Tier::Wallet),
            ("someone without a bank account", Tier::Wallet),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        // wealth, income, bills as percentages of the town's base; the rhythm
        // income arrives on; whether it follows prices; and how far the
        // starting balance may double.
        // wealth, income and bills as percentages of the town's base; the
        // rhythm income arrives on and whether it follows prices; how far the
        // balance may double; the cushion of its own bills it opens with; and
        // the share of the town's trade band it buys in.
        let h =
            |wealth_pct, income_pct, bills_pct, period, income_indexed, wealth_doublings_max, cushion, trade_pct| {
                Household {
                    wealth_pct,
                    income_pct,
                    bills_pct,
                    income_period: Some(period),
                    income_indexed,
                    wealth_doublings_max,
                    min_months_of_bills_ppm: Some(cushion),
                    trade_pct,
                }
            };
        let households: BTreeMap<String, Household> = [
            ("teenager", h(8, 12, 8, 7, true, 3, 250_000, 35)),
            ("someone without a bank account", h(12, 55, 60, 7, true, 3, 250_000, 55)),
            ("retiree", h(70, 55, 60, 30, false, 5, 750_000, 60)),
            ("market stallholder", h(55, 90, 80, 7, true, 5, 1_000_000, 95)),
            ("sysadmin", h(110, 130, 100, 30, true, 6, 1_000_000, 100)),
            ("former crypto trader", h(150, 90, 90, 30, true, 12, 1_000_000, 105)),
            ("freelancer", h(60, 110, 90, 30, true, 6, 1_000_000, 115)),
            ("tax inspector", h(100, 115, 95, 30, true, 6, 1_000_000, 85)),
            ("co-op treasurer", h(90, 100, 95, 30, true, 6, 1_500_000, 125)),
            ("invoice fraudster", h(80, 120, 80, 14, true, 6, 1_500_000, 170)),
            ("loan shark", h(500, 140, 60, 7, true, 8, 2_000_000, 230)),
            ("manufacturer on 30-day terms", h(400, 350, 320, 30, true, 8, 2_500_000, 600)),
        ]
        .into_iter()
        .map(|(name, household)| (name.to_string(), household))
        .collect();
        RunConfig {
            format_version: FORMAT_VERSION,
            model: ModelConfig {
                backend: Backend::Anthropic,
                model: String::new(),
                max_tokens: 4096,
                max_calls_per_turn: 24,
                cache_ttl: "1h".into(),
                timeout_secs: 600,
                base_url: "http://127.0.0.1:11434/v1".into(),
                max_tokens_field: "max_tokens".into(),
                api_key_env: String::new(),
                context_tokens: 0,
                retries: 4,
                extra_body: serde_json::Map::new(),
                tokens_per_minute: 0,
                price: None,
            },
            world_seed: 1,
            population: 50,
            founders: 4,
            adopters_at_genesis: Some(8),
            founder_supply_minor: 250_000,
            seal_amounts: 1.0,
            turn_budget: 10_000,
            target_ticks: 200,
            control: false,
            concurrency: 16,
            fold_answers: true,
            days_kept_whole: 10,
            notify_same_day: true,
            cap_dollars: 0.0,
            newcomer_cards: NewcomerCards::Written,
            repo: ".".into(),
            cards_path: "crates/swarm/personas/cards.json".into(),
            tiers,
            households,
            default_tier: Tier::Wallet,
            honest_card: HONEST_CARD.into(),
            economy: EconomyConfig {
                wealth_base_minor: 200_000,
                income_monthly_minor: 300_000,
                bills_monthly_minor: 260_000,
                calm_inflation_ppm: 80,
                boom_inflation_ppm: 150,
                crisis_inflation_ppm: 600,
                calm_to_boom_ppm: 8_000,
                calm_to_crisis_ppm: 4_000,
                boom_to_calm_ppm: 20_000,
                boom_to_crisis_ppm: 10_000,
                crisis_to_calm_ppm: 25_000,
                jump_ppm: 5_000,
                jump_min_ppm: 20_000,
                jump_max_ppm: 80_000,
                calm_cut_ppm: 300,
                crisis_cut_ppm: 12_000,
                cut_keeps_ppm: 400_000,
                cut_days: 60,
                buys_ppm: 350_000,
                sells_ppm: 350_000,
                trade_min_minor: 2_000,
                trade_max_minor: 40_000,
                bills_max_of_income_ppm: 900_000,
                wealth_min_months_of_bills_ppm: 1_500_000,
                sale_band: SaleBand::Buyer,
            },
            adoption: AdoptionConfig { arrears_minor: 50_000, per_settled_ppm: 15_000, max_ppm: 120_000 },
            social: SocialConfig {
                knows_mean: 6,
                rewire_ppm: 200_000,
                strangers_ppm: 120_000,
                // A stallholder knows the market; a teenager knows four people.
                knows_by_card: [
                    ("market stallholder", 14),
                    ("co-op treasurer", 12),
                    ("loan shark", 10),
                    ("manufacturer on 30-day terms", 9),
                    ("tax inspector", 8),
                    ("freelancer", 7),
                    ("invoice fraudster", 6),
                    ("sysadmin", 5),
                    ("former crypto trader", 5),
                    ("retiree", 4),
                    ("teenager", 4),
                    ("someone without a bank account", 3),
                ]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
            },
            trust: TrustConfig { evidence: true, wary: true },
            knowledge: KnowledgeConfig {
                paper_reading: vec!["crates/civitas/personas/paper-brief.md".to_string()],
                readme_headings: vec!["## The model".into(), "## Why it is Sybil-proof".into()],
                wallet_namespaces: [
                    "tour",
                    "onboarding",
                    "acceptance",
                    "bond",
                    "offers",
                    "contracts",
                    "myContracts",
                    "receivables",
                    "requests",
                    "governance",
                    "community",
                    "support",
                ]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            },
        }
    }
}

impl RunConfig {
    pub fn load(path: &str) -> Result<RunConfig, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
        let cfg: RunConfig = serde_json::from_str(&text).map_err(|e| format!("{path}: {e}"))?;
        cfg.check()?;
        Ok(cfg)
    }

    /// Refuse a configuration that could not produce a run, before anything is
    /// written.
    pub fn check(&self) -> Result<(), String> {
        if self.format_version != FORMAT_VERSION {
            return Err(format!("config format_version {} is not {FORMAT_VERSION}", self.format_version));
        }
        if !self.cap_dollars.is_finite() || self.cap_dollars < 0.0 {
            return Err(format!("cap_dollars {} is not an amount", self.cap_dollars));
        }
        if self.cap_dollars > 0.0 && self.model.price.is_none() && self.model.backend != Backend::Scripted {
            return Err(
                "cap_dollars is set and model.price is not: a cap nobody can price is a cap nobody enforces".into()
            );
        }
        if self.model.backend == Backend::OpenAi && self.model.base_url.trim().is_empty() {
            return Err("model.base_url is empty: name the endpoint that speaks the dialect".into());
        }
        if self.model.backend != Backend::Scripted && self.model.model.trim().is_empty() {
            return Err("model.model is empty: name the one model every person is played by".into());
        }
        // What `extra_body` may NOT name: the fields a backend writes itself.
        // Merged last, they would win, and the run would quietly not be the
        // run its manifest describes — a model nothing records, a tool list
        // nobody wrote, a conversation replaced, or a stream this crate
        // cannot read and would pay a whole reply to fail on.
        let written_here: &[&str] = match self.model.backend {
            Backend::Anthropic => &["model", "messages", "tools", "tool_choice", "system", "max_tokens"],
            Backend::OpenAi => &["model", "messages", "tools", "tool_choice"],
            Backend::Scripted => &[],
        };
        for key in self.model.extra_body.keys() {
            // Whatever the backend, this crate reads a whole answer and not a
            // stream of one: asked for a stream it would pay for the reply in
            // full and then fail to parse it, which no retry can mend.
            if key == "stream" && self.model.backend != Backend::Scripted {
                return Err("model.extra_body names \"stream\": this reads an answer whole, never as it arrives".into());
            }
            let its_own = self.model.backend == Backend::OpenAi && *key == self.model.max_tokens_field;
            if written_here.contains(&key.as_str()) || its_own {
                return Err(format!(
                    "model.extra_body names {key:?}, which this backend writes itself: merged last it would win, \
                     and the run would not be the one its manifest describes"
                ));
            }
        }
        if self.founders == 0 || self.founders > self.population {
            return Err(format!("{} founders in a population of {}", self.founders, self.population));
        }
        if let Some(adopters) = self.adopters_at_genesis {
            if adopters < self.founders || adopters > self.population {
                return Err(format!(
                    "{adopters} of {} people already use edet, which is fewer than its {} founders or more than the town",
                    self.population, self.founders
                ));
            }
        }
        if self.founder_supply_minor == 0 {
            return Err("founder_supply_minor is zero: a community with no seed can do nothing".into());
        }
        if !(0.0..=1.0).contains(&self.seal_amounts) {
            return Err(format!("seal_amounts {} is not in [0, 1]", self.seal_amounts));
        }
        if self.target_ticks == 0 {
            return Err("target_ticks is zero".into());
        }
        if !matches!(self.model.cache_ttl.as_str(), "5m" | "1h") {
            return Err(format!("model.cache_ttl {:?} is neither \"5m\" nor \"1h\"", self.model.cache_ttl));
        }
        let e = &self.economy;
        if e.jump_min_ppm > e.jump_max_ppm || e.cut_keeps_ppm > 1_000_000 {
            return Err("economy: a jump range is inverted or a cut keeps more than everything".into());
        }
        for (name, h) in &self.households {
            if !matches!(h.income_period, None | Some(7) | Some(14) | Some(30)) {
                return Err(format!("household {name:?}: income_period {:?} is none of 7, 14 or 30", h.income_period));
            }
            if h.income_pct == 0 && h.bills_pct > 0 {
                return Err(format!("household {name:?}: no income at all against bills that fall due"));
            }
        }
        Ok(())
    }

    pub fn path(&self, rel: &str) -> std::path::PathBuf {
        std::path::Path::new(&self.repo).join(rel)
    }

    pub fn tier_of(&self, card_name: &str) -> Tier {
        self.tiers.get(card_name).copied().unwrap_or(self.default_tier)
    }

    /// What this card's household is like. A card this table does not name
    /// lives at the town's own base, on the rhythm its draw gives it.
    pub fn household_of(&self, card_name: &str) -> Household {
        self.households.get(card_name).copied().unwrap_or_default()
    }
}

/// A persona card as `personas/cards.json` holds it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Card {
    pub name: String,
    pub card: String,
}

pub fn load_cards(cfg: &RunConfig) -> Result<Vec<Card>, String> {
    let path = cfg.path(&cfg.cards_path);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let cards: Vec<Card> = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    if cards.is_empty() {
        return Err(format!("{}: no cards", path.display()));
    }
    Ok(cards)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_price_is_four_rates_and_prices_a_usage() {
        let p = Price::parse("2.50, 10, 0.25, 2.50").expect("four rates");
        assert_eq!(p, Price { input: 2.5, output: 10.0, cache_read: 0.25, cache_write: 2.5 });
        let u = crate::event::Usage {
            calls: 1,
            input_tokens: 1_000_000,
            output_tokens: 100_000,
            cache_read_input_tokens: 4_000_000,
            cache_creation_input_tokens: 0,
        };
        assert!((p.cost(&u) - 4.5).abs() < 1e-9);
        assert!(Price::parse("2.5,10,0.25").is_err());
        assert!(Price::parse("2.5,ten,0.25,2.5").is_err());
        assert!(Price::parse("2.5,10,-1,2.5").is_err());
    }

    #[test]
    fn a_cap_without_a_price_is_refused() {
        let mut cfg = RunConfig::default();
        cfg.model.model = "m".into();
        cfg.cap_dollars = 50.0;
        assert!(cfg.check().is_err());
        cfg.model.price = Some(Price::parse("1,1,1,1").unwrap());
        assert!(cfg.check().is_ok());
        cfg.cap_dollars = -1.0;
        assert!(cfg.check().is_err());
    }
}
