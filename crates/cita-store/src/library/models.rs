use super::schema::{
    bibliography_references, contributors, identities, inspire_records, shelf_references, shelves,
};
use diesel::{Insertable, Queryable, Selectable, sqlite::Sqlite};

#[derive(Clone, Debug, Queryable, Selectable)]
#[diesel(table_name = bibliography_references)]
#[diesel(check_for_backend(Sqlite))]
pub(super) struct ReferenceRow {
    pub(super) id: i64,
    pub(super) source_kind: String,
    pub(super) bibtex: String,
    pub(super) title: String,
    pub(super) year: Option<i32>,
}

#[derive(Insertable)]
#[diesel(table_name = bibliography_references)]
pub(super) struct NewReference<'a> {
    pub(super) source_kind: &'a str,
    pub(super) bibtex: &'a str,
    pub(super) title: &'a str,
    pub(super) year: Option<i32>,
}

#[derive(Clone, Debug, Queryable, Selectable)]
#[diesel(table_name = contributors)]
#[diesel(check_for_backend(Sqlite))]
pub(super) struct ContributorRow {
    pub(super) reference_id: i64,
    pub(super) kind: String,
    pub(super) position: i64,
    pub(super) name: String,
}

#[derive(Insertable)]
#[diesel(table_name = contributors)]
pub(super) struct NewContributor<'a> {
    pub(super) reference_id: i64,
    pub(super) kind: &'a str,
    pub(super) position: i64,
    pub(super) name: &'a str,
}

#[derive(Clone, Debug, Queryable, Selectable)]
#[diesel(table_name = identities)]
#[diesel(check_for_backend(Sqlite))]
pub(super) struct IdentityRow {
    pub(super) reference_id: i64,
    pub(super) kind: String,
    pub(super) value: String,
    pub(super) canonical: bool,
}

#[derive(Insertable)]
#[diesel(table_name = identities)]
pub(super) struct NewIdentity<'a> {
    pub(super) reference_id: i64,
    pub(super) kind: &'a str,
    pub(super) value: &'a str,
    pub(super) canonical: bool,
}

#[derive(Clone, Debug, Insertable, Queryable, Selectable)]
#[diesel(table_name = inspire_records)]
#[diesel(check_for_backend(Sqlite))]
pub(super) struct InspireRow {
    pub(super) reference_id: i64,
    pub(super) record_id: i64,
    pub(super) updated: String,
}

#[derive(Insertable)]
#[diesel(table_name = shelves)]
pub(super) struct NewShelf<'a> {
    pub(super) name: &'a str,
}

#[derive(Clone, Debug, Insertable, Queryable, Selectable)]
#[diesel(table_name = shelf_references)]
#[diesel(check_for_backend(Sqlite))]
pub(super) struct MembershipRow {
    pub(super) shelf_id: i64,
    pub(super) reference_id: i64,
    pub(super) citation_key: String,
}
