// NOTE: checkpoint.rs in transaction_integrity uses the EV800000-EV800009 range.
// Support breakdown errors are in the EV810xxx range to avoid collisions.
pub mod support_breakdown_validation_error {
    pub const OWNER_ASSOCIATION_EXISTS: &str = "EV810000";
    pub const COEFFICIENTS_NOT_SUMMING_UP: &str = "EV810001";
    pub const COEFFICIENT_NOT_IN_RANGE: &str = "EV810002";
    pub const ACTION_AUTHOR_NOT_OWNER: &str = "EV810003";
    pub const ADDRESSES_MUST_CONTAIN_OWNER: &str = "EV810004";
    pub const ADDRESSES_CONTAIN_DUPLICATE: &str = "EV810005";
    pub const ADDRESS_REMOVED_IN_UPDATE: &str = "EV810006";
    pub const SUPPORT_BREAKDOWNS_NOT_DELETABLE: &str = "EV810007";
    pub const OWNER_TO_SUPPORT_BREAKDOWN_LINK_NOT_DELETABLE: &str = "EV810008";
    pub const SUPPORT_BREAKDOWN_UPDATES_LINK_NOT_DELETABLE: &str = "EV810009";
    pub const ADDRESS_TO_SUPPORT_BREAKDOWNS_LINK_NOT_DELETABLE: &str = "EV810010";
    pub const UPDATE_AUTHOR_NOT_OWNER: &str = "EV810011";
    pub const TARGET_OWNER_MISMATCH: &str = "EV810012";
    pub const BASE_ADDRESS_NOT_BENEFICIARY: &str = "EV810013";
    pub const EXPECTED_ENTRY_CREATION_ACTION: &str = "EV810014";
    pub const EXPECTED_ENTRY_TYPE: &str = "EV810015";
    pub const ORIGINAL_RECORD_NO_ENTRY: &str = "EV810016";
    pub const ORIGINAL_APP_ENTRY_NOT_DEFINED: &str = "EV810017";
    pub const UPDATE_ORIGINAL_NOT_CREATE: &str = "EV810018";
    pub const UPDATE_TYPE_MISMATCH: &str = "EV810019";
    pub const DELETE_ORIGINAL_NOT_CREATE: &str = "EV810020";
    pub const DELETE_ACTION_NOT_CREATE: &str = "EV810021";
    pub const PREVIOUS_ACTION_NOT_AVP: &str = "EV810022";
}

pub mod owner_to_support_breakdown_validation_error {
    pub const TARGET_NOT_ON_CREATE_WALLET_ACTION: &str = "EV811000";
    pub const AUTHOR_NOT_WALLET_OWNER: &str = "EV811001";
}
