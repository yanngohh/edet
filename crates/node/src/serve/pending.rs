//! The pending-signature pool, run under the node's signing scheme.
//!
//! The pool itself — entries keyed by digest, the fair share, the tombstones,
//! invitations, declines — is `edet_view::pending`, one implementation that a
//! simulation calls too. What the node adds is the scheme it runs under: an
//! envelope signs `block::tx_digest`, and a signature is Ed25519. The functions
//! below are the pool's checks with that scheme passed, so the door here and
//! `PendingPool::sign` inside still call one function and enforce one rule.

use edet_state::types::{Key, MemberId, Party};
use edet_state::{State, Tx};

pub use edet_view::pending::{
    decline_message, invite_message, Completed, Invite, PendingDeclineReq, PendingEntry, PendingList, PendingPool,
    PendingSignReq, SignOutcome, MAX_INVITED_PER_MEMBER,
};

use crate::block::{tx_digest, SignedTx};

/// The node's scheme: the consensus envelope digest, verified as Ed25519.
pub struct Ed25519;

impl edet_view::pending::Scheme for Ed25519 {
    fn digest(&self, chain_id: &str, tx: &Tx, nonce: &[u8; 16], not_after_epoch: u64) -> Option<[u8; 32]> {
        tx_digest(chain_id, tx, nonce, not_after_epoch).ok()
    }

    fn verify(&self, key: &Key, message: &[u8], signature: &[u8]) -> bool {
        use ed25519_dalek::{Signature, Verifier, VerifyingKey};
        let Ok(vk) = VerifyingKey::from_bytes(key) else { return false };
        let Ok(sig) = Signature::from_slice(signature) else { return false };
        vk.verify(message, &sig).is_ok()
    }
}

/// `edet_view::pending::invited_by` under [`Ed25519`].
pub fn invited_by(state: &State, req: &PendingSignReq, now_secs: u64) -> Option<(MemberId, Key)> {
    edet_view::pending::invited_by(&Ed25519, state, req, now_secs)
}

/// `edet_view::pending::verified_signer` under [`Ed25519`].
pub fn verified_signer(state: &State, req: &PendingSignReq, digest: &[u8; 32]) -> Option<Party> {
    edet_view::pending::verified_signer(&Ed25519, state, req, digest)
}

/// `edet_view::pending::request_digest` under [`Ed25519`].
pub fn request_digest(state: &State, req: &PendingSignReq) -> Option<[u8; 32]> {
    edet_view::pending::request_digest(&Ed25519, state, req)
}

impl From<Completed> for SignedTx {
    fn from(c: Completed) -> Self {
        SignedTx {
            tx: c.tx,
            nonce: c.nonce,
            not_after_epoch: c.not_after_epoch,
            signers: c.signers,
            signatures: c.signatures,
        }
    }
}
