diesel::table! {
    bibliography_references (id) {
        id -> BigInt,
        source_kind -> Text,
        bibtex -> Text,
        title -> Text,
        year -> Nullable<Integer>,
    }
}

diesel::table! {
    contributors (reference_id, kind, position) {
        reference_id -> BigInt,
        kind -> Text,
        position -> BigInt,
        name -> Text,
    }
}

diesel::table! {
    identities (reference_id, kind, value) {
        reference_id -> BigInt,
        kind -> Text,
        value -> Text,
        canonical -> Bool,
    }
}

diesel::table! {
    inspire_records (reference_id) {
        reference_id -> BigInt,
        record_id -> BigInt,
        updated -> Text,
    }
}

diesel::table! {
    shelves (id) {
        id -> BigInt,
        name -> Text,
    }
}

diesel::table! {
    shelf_references (shelf_id, reference_id) {
        shelf_id -> BigInt,
        reference_id -> BigInt,
        citation_key -> Text,
    }
}

diesel::joinable!(contributors -> bibliography_references (reference_id));
diesel::joinable!(identities -> bibliography_references (reference_id));
diesel::joinable!(inspire_records -> bibliography_references (reference_id));
diesel::joinable!(shelf_references -> bibliography_references (reference_id));
diesel::joinable!(shelf_references -> shelves (shelf_id));

diesel::allow_tables_to_appear_in_same_query!(
    bibliography_references,
    contributors,
    identities,
    inspire_records,
    shelves,
    shelf_references,
);
