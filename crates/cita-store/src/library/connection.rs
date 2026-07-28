use super::{LibraryError, schema::shelves};
use crate::DEFAULT_SHELF;
use diesel::{
    Connection, QueryableByName, RunQueryDsl, connection::SimpleConnection, insert_or_ignore_into,
    prelude::*, sql_query, sql_types::BigInt, sqlite::SqliteConnection,
};
use std::{fs, path::Path};
use url::Url;

const DATABASE_SCHEMA: i64 = 1;
const BOOTSTRAP_SQL: &str = include_str!("bootstrap.sql");

#[derive(QueryableByName)]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
struct UserVersion {
    #[diesel(sql_type = BigInt)]
    user_version: i64,
}

pub(super) fn create_directory(path: &Path) -> Result<(), LibraryError> {
    fs::create_dir_all(path).map_err(|source| LibraryError::Filesystem {
        path: path.into(),
        source,
    })
}

pub(super) fn validate_database_file(path: &Path) -> Result<(), LibraryError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| LibraryError::Filesystem {
        path: path.into(),
        source,
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(LibraryError::InvalidDatabasePath(path.into()));
    }
    Ok(())
}

pub(super) fn establish(path: &Path) -> Result<SqliteConnection, LibraryError> {
    if path.exists() {
        validate_database_file(path)?;
    }
    let url = database_url(path)?;
    let mut connection = SqliteConnection::establish(url.as_str())?;
    configure(&mut connection)?;
    Ok(connection)
}

pub(super) fn database_url(path: &Path) -> Result<Url, LibraryError> {
    Url::from_file_path(path).map_err(|()| LibraryError::InvalidDatabaseUrl(path.to_path_buf()))
}

fn configure(connection: &mut SqliteConnection) -> Result<(), LibraryError> {
    connection.batch_execute("PRAGMA busy_timeout = 5000;")?;
    connection.batch_execute("PRAGMA foreign_keys = ON;")?;
    connection.batch_execute("PRAGMA journal_mode = WAL;")?;
    connection.batch_execute("PRAGMA synchronous = FULL;")?;
    Ok(())
}

pub(super) fn initialize_schema(connection: &mut SqliteConnection) -> Result<(), LibraryError> {
    match user_version(connection)? {
        0 => connection.immediate_transaction(|connection| {
            connection.batch_execute(BOOTSTRAP_SQL)?;
            Ok(())
        }),
        DATABASE_SCHEMA => Ok(()),
        found => Err(LibraryError::UnsupportedSchema { found }),
    }
}

pub(super) fn validate_schema(connection: &mut SqliteConnection) -> Result<(), LibraryError> {
    let found = user_version(connection)?;
    if found == DATABASE_SCHEMA {
        Ok(())
    } else {
        Err(LibraryError::UnsupportedSchema { found })
    }
}

fn user_version(connection: &mut SqliteConnection) -> Result<i64, LibraryError> {
    Ok(sql_query("PRAGMA user_version")
        .get_result::<UserVersion>(connection)?
        .user_version)
}

pub(super) fn ensure_main(connection: &mut SqliteConnection) -> Result<(), LibraryError> {
    connection.immediate_transaction(|connection| {
        insert_or_ignore_into(shelves::table)
            .values(shelves::name.eq(DEFAULT_SHELF))
            .execute(connection)?;
        Ok(())
    })
}
