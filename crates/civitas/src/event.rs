//! **The events a tape holds.** Every change to a world is one of these, and
//! [`crate::world::World::apply`] is the only thing that makes one, so a world
//! rebuilt from a tape is the world the tape was written from.

use serde::{Deserialize, Serialize};

use edet_state::types::MemberId;

use crate::acts::{Act, ActResult};
use crate::config::Tier;

/// A person as the tape records them joining.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PersonRecord {
    pub index: usize,
    pub key: String,
    pub address: String,
    pub founder: bool,
    pub member: Option<MemberId>,
    /// The card's name, and its text. Both empty for a newcomer until their
    /// card is written.
    pub card_name: String,
    pub card: Option<String>,
    pub tier: Option<Tier>,
    pub joined_tick: u64,
    pub introduced_by: Option<usize>,
}

/// What one model conversation step cost.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Usage {
    pub calls: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
}

impl Usage {
    pub fn add(&mut self, other: &Usage) {
        self.calls += other.calls;
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_read_input_tokens += other.cache_read_input_tokens;
        self.cache_creation_input_tokens += other.cache_creation_input_tokens;
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// The founding: the ledger's genesis and everybody present on the first
    /// day.
    Genesis {
        chain_id: String,
        persons: Vec<PersonRecord>,
    },
    /// A day begins: the ledger crosses into `epoch`, the weather moves, money
    /// arrives and bills fall due, and these people will have a turn, in order.
    TickOpened {
        tick: u64,
        epoch: u64,
        schedule: Vec<usize>,
    },
    /// A newcomer's card, written for them before their first day.
    CardWritten {
        tick: u64,
        person: usize,
        name: String,
        card: String,
        tier: crate::config::Tier,
        exchange: Option<u64>,
    },
    /// A person's day, applied: what they did and what each act came to.
    Day {
        tick: u64,
        person: usize,
        exchange: u64,
        acts: Vec<Act>,
        results: Vec<ActResult>,
        /// Acts the day's checks refused. Never applied; kept because what a
        /// person tried and could not do is part of what they did.
        #[serde(default)]
        refused: Vec<(Act, ActResult)>,
        /// What the person had READ when this day was written: the square and
        /// the mail up to a sequence, and how many lines of news their note
        /// carried. The day spends these and nothing that arrived after them,
        /// which only a day lived beside its neighbours' can.
        ///
        /// Absent from a tape written before days were lived together, where
        /// nothing could arrive in between: the world as the day applies IS
        /// what the person read.
        #[serde(default)]
        seen: Option<crate::world::Seen>,
        usage: Usage,
    },
    /// A day that did not happen: the call failed, timed out or could not be
    /// understood. Nothing the person did in it is applied.
    Silent {
        tick: u64,
        person: usize,
        reason: String,
        usage: Usage,
    },
    /// What standing instructions did at the end of the day, per person.
    Instructions {
        tick: u64,
        fired: Vec<(usize, Act)>,
        results: Vec<ActResult>,
    },
    /// `invariants::audit` failed. The run ends here.
    Violation {
        tick: u64,
        person: Option<usize>,
        act: String,
        tx: String,
        signers: Vec<MemberId>,
        invariant: String,
    },
    /// The day ends. `digest` folds every transaction submitted so far and
    /// what the ledger answered; a replay that reaches a different one is not
    /// the same run.
    TickClosed {
        tick: u64,
        epoch: u64,
        digest: String,
        state_root: String,
        turns_used: u64,
    },
    /// A resume under a different model than the run was started with.
    ModelChanged {
        tick: u64,
        from: String,
        to: String,
    },
    Ended {
        tick: u64,
        reason: String,
    },
    /// A run that ended at its cap, continued under a higher one. The cap was
    /// money and not a finding, so raising it is a decision the tape records
    /// rather than a different run; nothing else that ends a run can be
    /// reopened.
    Reopened {
        tick: u64,
        cap_dollars: f64,
        spent: f64,
    },
}
