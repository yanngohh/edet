# The edet paper, in brief

What a person who has read the paper retains of it. Written from
`paper/sections/*.tex` — position, model, medium, standing, stability, recourse,
governance, security, economics, adoption, provision — and carrying their claims, not
new ones. It stands in for those sections in what a paper-reading person is told,
because the sections themselves are tens of thousands of tokens on every turn of a
life. Where this and the paper disagree, the paper is right.

## What it is

A clearing medium for a community of mutual producers: people and small firms who buy
from one another often enough to clear obligations against each other instead of
settling each one in national currency. The unit is a debt contract, created by a sale
— the buyer becomes debtor, the seller creditor — and discharged when the buyer
delivers value onward. There is no token, no balance, no bearer instrument and nothing
to hoard. What a member accumulates is **capacity**: a ceiling on how much the community
will underwrite them for, non-transferable, not for sale, held in the stakes others have
placed on them, and falling when those lapse.

## Capacity is a minimum cut

A member's capacity is the maximum flow into their account from the community's
underwriters, across directed stakes recording what creditors have placed. Gross
capacity is what the community stands behind altogether; residual capacity — what a
transaction consults and a wallet shows — is the same maximum over what outstanding
credit has not already reserved. Underwriters *inside* the set being measured supply it
nothing, so a coalition cannot underwrite itself.

One equation settles four questions: an account with no stakes has capacity zero, so
creating one grants nothing; the stakes are the reputation; the capacity of any set is
bounded by the stakes crossing into it, so identities are free and worthless together;
and volume internal to a set never crosses that set's own boundary, so fabricated trade
is arithmetically void. An adversary who acquires genuine backing worth B obtains at
most B of capacity across every account they control.

## Underwriters, and the seed

An underwriter is a member who has declared a **supply**: an accepted liability, not a
rank — if those the stakes reach through them fail, that much of the loss is theirs.
Every unit of supply arrives through a ceremony: genesis, or a seed amendment the
community endorsed. A supply may be lowered at any time but never below the flow already
committed through it, and it may not be declared against capacity the community itself
conferred — that inflates the declared total while the community can carry no more.
Zero is absorbing: a community that underwrote nothing at genesis can enact nothing and
insure nothing.

## Stakes

A stake is directed, creditor to debtor, and written **only by discharge**. When a
debtor has repaid R of an obligation, the creditor's stake on them becomes the greater
of what it was and the smaller of R and what that creditor may confer — their supply if
they underwrite, otherwise their own capacity. So it is a peak and not a sum: running
the same loop a thousand times raises it exactly as much as running it once. Selling
earns none of it — capacity asks whether the community will carry your debt, and the
evidence for that is a debt you carried. Stakes decay each epoch unless renewed, but an
edge never decays below what a live obligation holds on it.

## Insured and uninsured

Within the debtor's capacity an obligation is **insured**: accepting it reserves a flow
of exactly that amount, and the community's recourse stands behind it. Beyond capacity a
creditor may still lend, and the obligation is **uninsured**: it reserves nothing, no
recourse applies, and the creditor bears the loss alone. This is how a community starts
— first trades are uninsured, they settle, they write stakes, and capacity is their
residue. A wallet says which it is before anybody signs.

## What a sale does, in order

A sale discharges before it creates. First **netting**: whatever the seller already owes
the buyer is extinguished, oldest first. Then the **cascade**: what is left clears the
seller's own obligations, and those of beneficiaries they have listed, by moving them
onto the buyer — a debtor swap, which writes no stake for anybody. Only the **residue**
becomes a fresh obligation from buyer to seller. So a member who owes is paid by ceasing
to owe, and the ordinary way to pay a debt is to sell to your creditor: the debt goes
and the stake is written in one act.

The cascade is opt-in on both sides — the supporter lists beneficiaries, the beneficiary
approves supporters — and each edge is capped at a multiple of what that pair has staked
in one another, which is zero for a pair that has never settled anything. A claim moves
only as far as the buyer can carry it insured, and keeps the earlier of its own date and
the sale's. Being carried this way costs the beneficiary the standing the obligation
would have conferred: relief now against standing later.

## When something fails

An obligation past maturity with an amount outstanding is in default; the epoch sweep
marks it, so no default waits on anybody noticing. For an **insured** obligation,
**substitution**: the underwriters whose supply carried it become the creditor's
debtors, split by exactly what each arc carried, and the defaulter's debt runs to them
instead of to the creditor. The substituted leg is itself uninsured — the loss does not
walk a hop further out — and the underwriter's supply stays drawn until the defaulter
cures. For an **uninsured** one, nothing: the creditor bears it. There is no loss pool,
because a pledge funded by a signature is funded by nothing, and making one sound turns
it back into the insured tier.

Parties may attach **arbitration** at acceptance: named arbiters, a quorum, a window, an
award ceiling. The award is the median of what the arbiters attest, minted once as an
ordinary uninsured obligation from creditor back to debtor, bounded by the original
amount — the buyer's remedy for non-delivery. A minority of the panel cannot move a
median.

The sweep also **nets rings** of defaults by the smallest hop, with no signature: every
party is already due, so nobody is paid early. Late payment — **cure** — clears a
default and stakes like any settlement. Default is a state, not a verdict.

## What writing costs

Writes are feeless: no gas, no fee token, no revenue to anybody. Spam is priced by
**operation bonds**: past a small free allowance each epoch, a transition encumbers
headroom measured against what the community's external seed reaches you for, returned
on schedule. Sustained exhaustion forfeits bonds — recorded against the member, paid to
nobody. Discharging, curing, transferring, exiting and lowering a supply are free, so a
member without headroom can still pay what they owe.

A **row is seated by its first bonded trade**, not by registration, and it costs its
sponsor a bond unit of their reach held for the life of the row. So a community seats
about one row per twenty units of seed and then nobody until a ceremony, and the member
best placed to bring in a newcomer can seat nobody until they have themselves borrowed
and repaid. An empty row that owes nothing and is named by nothing is retired a year
later, and the seat comes back.

## Governance

The electorate is the external seed: an assenting coalition's weight is its share of the
supply that arrived through ceremonies, because that is the one quantity a signature
cannot manufacture. Ordinary members have no vote, which is the decision rather than an
oversight. Six constants are governed within fixed safe ranges — the client's risk
midpoint, the visibility policy, the bond fraction, the decay ratio, the seed-amendment
rate, and the insured horizon. Changing who orders the ledger takes two thirds; anything
else a simple threshold, which itself can never be amended. A suspended member may still
pay, be paid, lower a supply and vote on their own reinstatement.

A **seed amendment** is a proposal whose author adds to their own declared supply; its
author cannot assent it, and amendments in one epoch may add at most a fixed fraction of
the seed the epoch opened with. **Re-denomination** changes the unit by a bounded factor,
scaling amounts, stakes and reservations together, so every capacity and every ratio is
preserved; rounding is always down, and a claim that rounds to nothing loses its
insurance rather than its debt.

## What it refuses, and what it costs

No rent on the medium, no hoardable balance, no lender of last resort, no
macroprudential brake, and no bridge to another ledger: a separate deployment is a
separate system with no credit path to it. The thesis is conditional — clearing beats
settling in currency only where enough of what a community buys is available from within
it — and a community that fails that test cannot be persuaded into this by any
mechanism. A seed is sized on peak simultaneous insured credit, never annual volume; err
low, because understating it costs friction while overstating it cannot be corrected.
Provision to a date is a claim, whose insurance lasts only to a governed horizon from
acceptance; provision across seasons is capacity, which decays; provision across decades
belongs to institutions, which are members like any other.
