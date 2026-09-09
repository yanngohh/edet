//! Emit inclusion-proof fixtures in the node's wire shape, for
//! `ui/src/lib/__tests__/proof.test.ts` to pin its reimplementation against.
//!
//! `cargo run -p edet-state --example proof_fixture`
//!
//! The client verifies proofs by reimplementing `root.rs` in TypeScript, and a
//! reimplementation cross-pinned against a HAND-COPIED fixture is pinned to
//! whatever the fixture said when somebody last pasted it. When the section
//! list changed under both of them, the test kept passing — implementation and
//! oracle had drifted together, which is the one failure a cross-pin is
//! supposed to be immune to. Generating the fixture from the normative
//! implementation is what makes it an oracle again.
use edet_state::root::{prove, prove_member, state_root, InclusionProof, Section};
use edet_state::state::State;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn emit(name: &str, p: &InclusionProof) {
    println!("const {name} = {{");
    println!("    section: '{}',", format!("{:?}", p.section).to_lowercase());
    println!("    index: {},", p.index);
    println!("    leaf_count: {},", p.leaf_count);
    println!("    key: '{}',", hex(&p.key));
    println!("    value: '{}',", hex(&p.value));
    println!("    leaf_salt: '{}',", hex(&p.leaf_salt));
    println!("    path: [");
    for h in &p.path {
        println!("        '{}',", hex(h));
    }
    println!("    ],");
    println!("    section_path: [");
    for h in &p.section_path {
        println!("        '{}',", hex(h));
    }
    println!("    ],");
    println!("}};");
}

fn main() {
    // Exactly the fixture `root.rs`'s own tests build: five founding
    // underwriters, nothing else.
    let mut st = State::default();
    for i in 0..5u8 {
        st.add_underwriter(vec![[i + 1; 32]], 2500.0).expect("founding underwriter");
    }
    println!("const RUST_ROOT = '{}';\n", hex(&state_root(&st).expect("root")));
    // An even index (consumes a right sibling at level 0) and the promoted
    // last leaf of an odd level (one sibling across three levels).
    emit("RUST_MEMBER_2_OF_5", &prove_member(&st, 2).expect("prove").expect("present"));
    println!();
    emit("RUST_MEMBER_4_OF_5", &prove_member(&st, 4).expect("prove").expect("present"));
    println!();
    // The two sections whose TAG and top-tree POSITION differ, which is the
    // deviation a verifier that derives one from the other cannot survive: it
    // passes every members-section fixture and fails only here. `Stakes` also
    // pins a section whose leaves are built from the graph rather than from a
    // record map, and whose value is the empty row an account with no
    // out-edge still holds a leaf for.
    emit(
        "RUST_STAKES_2_OF_5",
        &prove(&st, Section::Stakes, &2u64.to_be_bytes())
            .expect("prove")
            .expect("present"),
    );
    println!();
    emit("RUST_LEDGER", &prove(&st, Section::Ledger, &[]).expect("prove").expect("present"));
}
