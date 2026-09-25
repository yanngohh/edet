//! **The shadow economy: money beside the ledger.**
//!
//! The ledger holds no money and cannot: every amount in `State` is a unit of
//! the community's own credit, and none of its transitions is a cash payment.
//! So the money every person also holds lives here — a balance per household,
//! income that arrives on a schedule, bills that fall due, and one price index
//! that scales both. Nothing here is audited by `invariants::audit`; the ledger
//! does not know it exists.
//!
//! **The weather is seeded and exogenous.** A hidden regime — calm, boom,
//! crisis — moves by a Markov draw each day, sets how fast prices rise, and in
//! a crisis cuts many households' income at once. Every draw comes from a
//! stream derived from the world seed and shared with nothing the people do,
//! so two runs of one world have the same weather whatever their people did.
//!
//! Integers throughout: minor units of money, parts per million for rates.

use std::collections::VecDeque;

use serde::Serialize;

use edet_swarm::rng::{mix, Rng};

use crate::config::{EconomyConfig, Household, SaleBand, SocialConfig};
use crate::social::Social;
use crate::streams;

pub const PPM: u64 = 1_000_000;
/// Days a month is, for income and bills.
pub const MONTH: u64 = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Regime {
    Calm,
    Boom,
    Crisis,
}

/// One household's money.
#[derive(Clone, Debug)]
pub struct Purse {
    pub cash: u64,
    /// Bills that fell due and were not covered. Paid first out of income.
    pub arrears: u64,
    pub income_base: u64,
    pub income_period: u64,
    income_phase: u64,
    pub bill_base: u64,
    bill_phase: u64,
    /// Whether this income is scaled by the price index, as bills always are.
    pub income_indexed: bool,
    /// Percent of the town's trade band this household buys in.
    pub trade_pct: u64,
    /// The last day of a cut to this household's income, if one is running.
    pub cut_until: Option<u64>,
    rng: Rng,
}

/// What happened to a household's money on one day, for the person's news.
#[derive(Clone, Debug, Default, Serialize)]
pub struct CashNews {
    pub income: u64,
    pub income_cut: bool,
    pub bill: u64,
    pub bill_short: u64,
    pub arrears_paid: u64,
    pub cut_began: bool,
    /// Somebody to buy from today, and what it comes to: the person's own
    /// trade, which is the only reason anybody here has to give or take
    /// credit. Whether they settle it in money or on the ledger is theirs.
    pub buy_from: Option<(usize, u64)>,
    /// Somebody who wants to buy from them, and for how much.
    pub sell_to: Option<(usize, u64)>,
}

#[derive(Clone, Debug)]
pub struct Economy {
    cfg: EconomyConfig,
    /// The price level, `PPM` at genesis.
    pub index_ppm: u64,
    pub regime: Regime,
    macro_rng: Rng,
    /// The index at the close of each of the last `MONTH + 1` days.
    history: VecDeque<u64>,
    pub purses: Vec<Purse>,
    world_seed: u64,
}

impl Economy {
    pub fn new(cfg: &EconomyConfig, world_seed: u64) -> Self {
        Economy {
            cfg: cfg.clone(),
            index_ppm: PPM,
            regime: Regime::Calm,
            macro_rng: Rng::seeded(mix(world_seed, streams::MACRO)),
            history: VecDeque::from(vec![PPM]),
            purses: Vec::new(),
            world_seed,
        }
    }

    /// Open a household for person `index`, drawn from that person's own
    /// stream: a skewed starting balance, an income on a weekly, fortnightly or
    /// monthly rhythm, and a monthly bill, each scaled by what `h` says a
    /// person of this card lives at.
    ///
    /// **Every draw happens whatever `h` is**, including the rhythm's when `h`
    /// names one, so the stream a household is drawn from does not depend on
    /// who the person turned out to be: one seed's weather and one seed's
    /// trades are the same against any table of households.
    pub fn open_purse(&mut self, index: usize, h: &Household) {
        debug_assert_eq!(index, self.purses.len(), "a purse per person, in person order");
        let mut rng = Rng::seeded(mix(self.world_seed, streams::HOUSEHOLD + index as u64));
        let pct = |amount: u64, pct: u64| (amount as u128 * pct as u128 / 100) as u64;
        let quarter = (self.cfg.wealth_base_minor / 4).max(1);
        let doublings = rng.next().trailing_zeros().min(h.wealth_doublings_max);
        let cash = pct(quarter.saturating_mul(1u64 << doublings) + rng.below(quarter), h.wealth_pct);
        let drawn_period = [7u64, 14, 30][rng.below(3) as usize];
        let income_period = h.income_period.unwrap_or(drawn_period);
        let income_phase = rng.below(income_period);
        let spread = |rng: &mut Rng, lo_pct: u64, hi_pct: u64| lo_pct + rng.below(hi_pct - lo_pct + 1);
        let income_pct = spread(&mut rng, 50, 150);
        let income_base = pct(self.cfg.income_monthly_minor * income_period / MONTH * income_pct / 100, h.income_pct);
        let bill_pct = spread(&mut rng, 70, 130);
        let mut bill_base = pct(self.cfg.bills_monthly_minor * bill_pct / 100, h.bills_pct);
        // What this household earns in a month, whatever rhythm it earns it on.
        let monthly_income = income_base * MONTH / income_period.max(1);
        if self.cfg.bills_max_of_income_ppm > 0 {
            let ceiling = (monthly_income as u128 * self.cfg.bills_max_of_income_ppm as u128 / PPM as u128) as u64;
            bill_base = bill_base.min(ceiling);
        }
        // A household opens with a cushion against its own bill, not against
        // the town's: the two are drawn apart and only this relates them.
        let months = h.min_months_of_bills_ppm.unwrap_or(self.cfg.wealth_min_months_of_bills_ppm);
        let cash = cash.max((bill_base as u128 * months as u128 / PPM as u128) as u64);
        let bill_phase = rng.below(MONTH);
        self.purses.push(Purse {
            cash,
            arrears: 0,
            income_base,
            income_period,
            income_phase,
            bill_base,
            bill_phase,
            income_indexed: h.income_indexed,
            trade_pct: h.trade_pct,
            cut_until: None,
            rng,
        });
    }

    fn scale(&self, amount: u64) -> u64 {
        (amount as u128 * self.index_ppm as u128 / PPM as u128) as u64
    }

    /// One day of weather and money. Draws the same number of times whatever
    /// happened, so a household's stream does not depend on what anybody did.
    ///
    /// Who a day brings is the town's own graph's to say: with none, anybody
    /// but themselves, drawn evenly.
    pub fn open_day(&mut self, tick: u64, social: &Social, social_cfg: &SocialConfig) -> Vec<CashNews> {
        let c = &self.cfg;
        let roll = self.macro_rng.below(PPM);
        self.regime = match self.regime {
            Regime::Calm if roll < c.calm_to_crisis_ppm => Regime::Crisis,
            Regime::Calm if roll < c.calm_to_crisis_ppm + c.calm_to_boom_ppm => Regime::Boom,
            Regime::Boom if roll < c.boom_to_crisis_ppm => Regime::Crisis,
            Regime::Boom if roll < c.boom_to_crisis_ppm + c.boom_to_calm_ppm => Regime::Calm,
            Regime::Crisis if roll < c.crisis_to_calm_ppm => Regime::Calm,
            r => r,
        };
        let mut rate = match self.regime {
            Regime::Calm => c.calm_inflation_ppm,
            Regime::Boom => c.boom_inflation_ppm,
            Regime::Crisis => c.crisis_inflation_ppm,
        };
        let jump_roll = self.macro_rng.below(PPM);
        let jump_size = c.jump_min_ppm + self.macro_rng.below(c.jump_max_ppm - c.jump_min_ppm + 1);
        if jump_roll < c.jump_ppm {
            rate += jump_size;
        }
        self.index_ppm = (self.index_ppm as u128 * (PPM + rate) as u128 / PPM as u128) as u64;
        self.history.push_back(self.index_ppm);
        while self.history.len() > MONTH as usize + 1 {
            self.history.pop_front();
        }

        let cut_ppm = match self.regime {
            Regime::Calm => c.calm_cut_ppm,
            Regime::Boom => c.calm_cut_ppm / 2,
            Regime::Crisis => c.crisis_cut_ppm,
        };
        let (keeps, cut_days) = (c.cut_keeps_ppm, c.cut_days);
        let mut news = Vec::with_capacity(self.purses.len());
        for i in 0..self.purses.len() {
            // A wage follows prices and a pension does not; a bill always does.
            let income = match self.purses[i].income_indexed {
                true => self.scale(self.purses[i].income_base),
                false => self.purses[i].income_base,
            };
            let bill = self.scale(self.purses[i].bill_base);
            let p = &mut self.purses[i];
            let mut n = CashNews::default();
            if p.cut_until.is_some_and(|until| until < tick) {
                p.cut_until = None;
            }
            if p.rng.below(PPM) < cut_ppm && p.cut_until.is_none() {
                p.cut_until = Some(tick + cut_days);
                n.cut_began = true;
            }
            if (tick + p.income_phase).is_multiple_of(p.income_period) {
                let mut amount = income;
                if p.cut_until.is_some() {
                    amount = (amount as u128 * keeps as u128 / PPM as u128) as u64;
                    n.income_cut = true;
                }
                let to_arrears = amount.min(p.arrears);
                p.arrears -= to_arrears;
                p.cash += amount - to_arrears;
                n.income = amount;
                n.arrears_paid = to_arrears;
            }
            if (tick + p.bill_phase).is_multiple_of(MONTH) {
                let paid = bill.min(p.cash);
                p.cash -= paid;
                p.arrears += bill - paid;
                n.bill = bill;
                n.bill_short = bill - paid;
            }
            news.push(n);
        }
        // Who trades with whom today. Drawn after the money, from each
        // person's own stream, so a control run of the same seed meets the
        // same suppliers and the same customers on the same days.
        let people = self.purses.len();
        if people > 1 {
            let span = c.trade_max_minor.saturating_sub(c.trade_min_minor) + 1;
            let index = self.index_ppm;
            for (i, n) in news.iter_mut().enumerate() {
                let scale = |minor: u64| (minor as u128 * index as u128 / PPM as u128) as u64;
                // Every draw first, from this person's own stream, and the
                // band applied after: what a trade comes to is the buyer's
                // size, and for a sale the buyer is the other person.
                let (buy, sell) = {
                    let p = &mut self.purses[i];
                    let buy = (p.rng.below(PPM) < c.buys_ppm).then(|| {
                        let raw = c.trade_min_minor + p.rng.below(span);
                        social.partner(social_cfg, &mut p.rng, i, people).map(|j| (j, raw))
                    });
                    let sell = (p.rng.below(PPM) < c.sells_ppm).then(|| {
                        let raw = c.trade_min_minor + p.rng.below(span);
                        social.partner(social_cfg, &mut p.rng, i, people).map(|j| (j, raw))
                    });
                    (buy.flatten(), sell.flatten())
                };
                // Whose band a trade is drawn in: the buyer's, or the smaller
                // of the two. Under the buyer's alone a teenager at 35 sold at
                // a manufacturer's 600 and was the creditor on a quarter of
                // pilot-6's contracts, while the manufacturer bought at six
                // times every supplier's scale and was cashless by day 35.
                let pct = |buyer: usize, seller: usize| match c.sale_band {
                    SaleBand::Buyer => self.purses[buyer].trade_pct,
                    SaleBand::Smaller => self.purses[buyer].trade_pct.min(self.purses[seller].trade_pct),
                };
                let band = |raw: u64, pct: u64| scale((raw as u128 * pct as u128 / 100) as u64);
                n.buy_from = buy.map(|(j, raw)| (j, band(raw, pct(i, j))));
                n.sell_to = sell.map(|(j, raw)| (j, band(raw, pct(j, i))));
            }
        }
        news
    }

    /// Move money between two households. Refused, with nothing moved, when
    /// the payer does not have it.
    pub fn pay(&mut self, from: usize, to: usize, amount: u64) -> Result<(), &'static str> {
        if amount == 0 {
            return Err("a payment of nothing");
        }
        if from == to {
            return Err("a payment to yourself");
        }
        let Some(payer) = self.purses.get(from) else { return Err("no such payer") };
        if to >= self.purses.len() {
            return Err("no such payee");
        }
        if payer.cash < amount {
            return Err("not enough cash");
        }
        self.purses[from].cash -= amount;
        self.purses[to].cash += amount;
        Ok(())
    }

    /// The published climate: a fact the economy states, never a forecast.
    /// The regime itself is not in it.
    pub fn indicator(&self) -> Indicator {
        let month_ago = self.history.front().copied().unwrap_or(PPM).max(1);
        let change_ppm = self.index_ppm as i128 * PPM as i128 / month_ago as i128 - PPM as i128;
        Indicator {
            price_index: fmt_ppm(self.index_ppm),
            change_over_last_30_days: fmt_signed_percent(change_ppm),
            households_with_income_cut: self.purses.iter().filter(|p| p.cut_until.is_some()).count(),
            households: self.purses.len(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Indicator {
    pub price_index: String,
    pub change_over_last_30_days: String,
    pub households_with_income_cut: usize,
    pub households: usize,
}

/// `1234567` ppm as `"1.234567"`.
pub fn fmt_ppm(ppm: u64) -> String {
    format!("{}.{:06}", ppm / PPM, ppm % PPM)
}

/// Signed parts per million as a percentage with two places.
fn fmt_signed_percent(ppm: i128) -> String {
    let sign = if ppm < 0 { "-" } else { "+" };
    let bp = ppm.unsigned_abs() / 100; // hundredths of a percent
    format!("{sign}{}.{:02}%", bp / 100, bp % 100)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{RunConfig, SaleBand};

    /// Two households, one at 35 and one at 600, every day a trade, and one
    /// fixed size: whose band a trade is drawn in is what the amounts say.
    fn town(band: SaleBand) -> Vec<CashNews> {
        let mut cfg = RunConfig::default().economy;
        cfg.buys_ppm = PPM;
        cfg.sells_ppm = PPM;
        cfg.trade_min_minor = 10_000;
        cfg.trade_max_minor = 10_000;
        cfg.calm_inflation_ppm = 0;
        cfg.jump_ppm = 0;
        cfg.calm_cut_ppm = 0;
        cfg.sale_band = band;
        let mut e = Economy::new(&cfg, 7);
        e.open_purse(0, &Household { trade_pct: 35, ..Default::default() });
        e.open_purse(1, &Household { trade_pct: 600, ..Default::default() });
        e.open_day(1, &Social::none(), &crate::config::SocialConfig::default())
    }

    #[test]
    fn under_the_buyer_s_band_a_small_household_sells_at_a_large_one_s_size() {
        let news = town(SaleBand::Buyer);
        assert_eq!(news[0].buy_from, Some((1, 3_500)), "the teenager buys at 35");
        assert_eq!(news[0].sell_to, Some((1, 60_000)), "and sells at the manufacturer's 600");
        assert_eq!(news[1].buy_from, Some((0, 60_000)));
        assert_eq!(news[1].sell_to, Some((0, 3_500)));
    }

    #[test]
    fn under_the_smaller_band_nobody_trades_beyond_the_smaller_of_the_two() {
        let news = town(SaleBand::Smaller);
        for n in &news {
            assert_eq!(n.buy_from.map(|(_, a)| a), Some(3_500), "{n:?}");
            assert_eq!(n.sell_to.map(|(_, a)| a), Some(3_500), "{n:?}");
        }
    }
}
